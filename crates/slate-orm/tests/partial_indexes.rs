//! A partial index declared through `#[derive(Record)]`.
//!
//! The derive's job here is narrow — turn `only_where(...)` into the same
//! `IndexDef` a hand-written schema would build — but the ways it can go wrong
//! are all quiet ones, so the tests are arranged around them rather than around
//! the feature.
//!
//! Equality is the weakest of them and comes first only because it is the
//! obvious test to write: [`IndexDef`]'s `PartialEq` deliberately does not
//! compare predicates, so two tables comparing equal says nothing about whether
//! the predicate survived, or which index it landed on. What says something is
//! `admits`, on rows either side of the predicate, and the shape of the
//! predicate the planner will read back.
//!
//! The other two halves are the ones a mistake actually costs something in:
//! the record store has to *maintain* the index the derive declared, and the
//! planner has to be able to *read* the predicate it emitted. Neither is
//! checked by asking a query for its rows — see the module docs of
//! `slate-kernel/tests/partial_indexes.rs`: a spurious index entry is invisible
//! through a query, because the residual predicate throws the row away and the
//! answer looks right. So maintenance is checked against raw index keys, and
//! planning against the plan.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::keys;
use slate_orm::{
    AccessSummary, Action, Catalog, ColumnStats, Expr, Grant, IndexDef, IndexId, Ordinal, Query,
    Record, RecordStore, Records, Row, SecurityCatalog, SecurityContext, Statistics, TableDef,
    TableId, TableStats, Value, ValueType, memory::MemoryStore,
};
use std::collections::BTreeSet;

const DOCS: TableId = TableId(1);
const BY_TITLE: IndexId = IndexId(10);
const LIVE_BY_AUTHOR: IndexId = IndexId(11);

/// A document table with the commonest partial index there is: an index over
/// the rows that have not been soft-deleted.
///
/// The predicate names `deleted_at` — the *field*, which the macro turns into
/// an ordinal at expansion. Resolving the name at runtime is what a hand-built
/// schema cannot do here, since building the table is what evaluates the
/// argument.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "docs", id = 1)]
#[record(index(
    name = "live_by_author",
    id = 11,
    columns("author"),
    only_where(Expr::is_null(deleted_at))
))]
struct Doc {
    #[record(pk)]
    id: u64,
    author: u64,
    // Not partial, and here as the control: whatever the predicate restricts,
    // it has to be one index rather than the table or every index on it.
    #[record(index(name = "by_title", id = 10))]
    title: String,
    deleted_at: Option<i64>,
}

fn doc(id: u64, author: u64, deleted_at: Option<i64>) -> Doc {
    Doc {
        id,
        author,
        title: format!("doc {id}"),
        deleted_at,
    }
}

/// The table a person would have written by hand.
///
/// The predicate is written by position, because [`IndexBuilder::only_where`]
/// gives no other way: a name resolved by building the table recurses. That
/// asymmetry is the whole reason the attribute exists, so pinning the position
/// here is also what pins the macro's arithmetic.
fn hand_written() -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("author", ValueType::U64)
        .column("title", ValueType::Str)
        .nullable_column("deleted_at", ValueType::I64)
        .primary_key(["id"])
        // Struct-level attributes are emitted before field-level ones, and
        // `TableDef` equality compares the index list in order.
        .index(
            IndexDef::builder("live_by_author", LIVE_BY_AUTHOR)
                .column("author")
                .only_where(Expr::is_null(Ordinal(3))),
        )
        .index(IndexDef::builder("by_title", BY_TITLE).column("title"))
        .build()
        .expect("hand-written schema is valid")
}

#[test]
fn deleted_at_is_the_fourth_column() {
    assert_eq!(Doc::COLUMNS.deleted_at, Ordinal(3));
    assert_eq!(
        Doc::COLUMNS.deleted_at,
        Doc::table().ordinal_of("deleted_at").unwrap()
    );
}

/// Equal as values, and — the part equality does not cover — the same index
/// holds the same rows.
#[test]
fn the_derived_partial_index_behaves_like_a_hand_written_one() {
    let derived = Doc::table();
    let expected = hand_written();
    assert_eq!(*derived, expected);

    let live = doc(1, 7, None).to_row();
    let deleted = doc(2, 7, Some(1_700_000_000)).to_row();

    for (name, id) in [("by_title", BY_TITLE), ("live_by_author", LIVE_BY_AUTHOR)] {
        let derived = derived.index(id).expect("the derived index exists");
        let hand = expected.index(id).expect("the hand-written index exists");
        for row in [&live, &deleted] {
            assert_eq!(
                derived.admits(row),
                hand.admits(row),
                "`{name}` disagrees with the hand-written index about {row:?}"
            );
        }
    }

    // And what they agree on is the predicate rather than "everything": the
    // partial index drops the deleted row, the ordinary one keeps it.
    let live_by_author = derived.index(LIVE_BY_AUTHOR).unwrap();
    assert!(live_by_author.admits(&live));
    assert!(!live_by_author.admits(&deleted));
    let by_title = derived.index(BY_TITLE).unwrap();
    assert!(by_title.admits(&live) && by_title.admits(&deleted));
}

/// The predicate has to come back out as an `Expr`.
///
/// `only_where` accepts any `Predicate`, and the planner reads one only by
/// downcasting it — so a predicate of any other type is not a broken index but
/// an index no plan ever chooses. The macro pins the type; this checks that the
/// thing arriving at the planner is the expression that was written.
#[test]
fn the_derived_predicate_is_an_expr_the_planner_can_read() {
    let index = Doc::table().index(LIVE_BY_AUTHOR).unwrap();
    let predicate = index.predicate().expect("the index is partial");
    let expr = predicate
        .as_any()
        .and_then(|any| any.downcast_ref::<Expr>())
        .expect("the planner reads the predicate by downcasting it to `Expr`");
    assert_eq!(*expr, Expr::is_null(Doc::COLUMNS.deleted_at));
}

/// The same declaration written on the field the index keys on.
///
/// Both forms go through one parser, so this is here to keep them from
/// drifting rather than to test the predicate twice — and to pin the one thing
/// the field-level form could plausibly get wrong, which is assuming the field
/// it is written on is the only column in play. The predicate names another.
#[derive(Record, Debug)]
#[record(table = "notes", id = 2)]
struct Note {
    #[record(pk)]
    id: u64,
    #[record(index(
        name = "live_notes_by_owner",
        id = 20,
        only_where(Expr::is_null(archived_at))
    ))]
    owner: u64,
    archived_at: Option<i64>,
}

#[test]
fn a_partial_index_can_be_declared_on_a_field() {
    let index = Note::table().index(IndexId(20)).unwrap();
    assert_eq!(index.columns()[0].ordinal, Note::COLUMNS.owner);
    let predicate = index.predicate().expect("the index is partial");
    let expr = predicate
        .as_any()
        .and_then(|any| any.downcast_ref::<Expr>())
        .expect("the predicate is an `Expr`");
    assert_eq!(*expr, Expr::is_null(Note::COLUMNS.archived_at));
}

fn store() -> (RecordStore<MemoryStore>, MemoryStore) {
    let kv = MemoryStore::new();
    let catalog = Catalog::from_tables([Doc::table().clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", DOCS, Action::ALL));
    (
        // The same handle, so the assertions read the keys the record layer is
        // writing rather than a copy of them.
        RecordStore::new(kv.clone(), catalog, security).with_statistics(statistics()),
        kv,
    )
}

/// A table big enough for an index to be worth its point reads, with `author`
/// selective and nine rows in ten live.
///
/// Without this the planner is working from `TableStats::assumed`, and what
/// these tests want to know is whether it *may* read the partial index, not
/// whether a million-row guess makes it want to.
fn statistics() -> Statistics {
    let spread = ColumnStats {
        distinct: 1_000_000,
        null_fraction: 0.0,
    };
    Statistics::new().with(
        DOCS,
        TableStats::with_row_count(1_000_000)
            .with_column(Doc::COLUMNS.author, spread)
            .with_column(Doc::COLUMNS.title, spread)
            .with_column(
                Doc::COLUMNS.deleted_at,
                ColumnStats {
                    distinct: 100,
                    null_fraction: 0.9,
                },
            ),
    )
}

/// Every committed key under the index's prefix.
fn index_keys(kv: &MemoryStore, index: IndexId) -> BTreeSet<Vec<u8>> {
    let table = Doc::table();
    let index = table.index(index).expect("the index exists");
    let prefix = keys::index_prefix(table, index, None);
    kv.keys()
        .into_iter()
        .filter(|key| key.starts_with(&prefix))
        .collect()
}

/// The keys the index *should* hold, given the rows that are in the table.
fn wanted_keys(index: IndexId, rows: &[Row]) -> BTreeSet<Vec<u8>> {
    let table = Doc::table();
    let index = table.index(index).expect("the index exists");
    rows.iter()
        .filter(|row| index.admits(row))
        .map(|row| {
            keys::index_entry(
                table,
                index,
                &row.index_values(index),
                &row.primary_key_values(table),
            )
            .key
        })
        .collect()
}

/// Declaring the index through the derive has to make the write path maintain
/// it, which is the whole point of declaring it on the schema at all.
///
/// Checked against the stored keys: a query would not notice a stray entry,
/// because the residual predicate discards the row it points at.
#[tokio::test]
async fn the_record_store_maintains_the_derived_index() {
    let (store, kv) = store();
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    txn.insert_record(&root, &doc(1, 7, None)).await.unwrap();
    txn.insert_record(&root, &doc(2, 7, Some(10)))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(
        index_keys(&kv, LIVE_BY_AUTHOR),
        wanted_keys(LIVE_BY_AUTHOR, &[doc(1, 7, None).to_row()]),
        "the deleted document is in the index, or the live one is not"
    );
    // The ordinary index holds both, so an empty predicate has not leaked onto
    // the whole table.
    assert_eq!(index_keys(&kv, BY_TITLE).len(), 2);

    // Deleting the live one takes its entry with it.
    let txn = store.begin().await.unwrap();
    txn.update_record(&root, &doc(1, 7, Some(20)))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert!(
        index_keys(&kv, LIVE_BY_AUTHOR).is_empty(),
        "soft-deleting a document left its entry behind"
    );

    // And undeleting it puts the entry back, even though `author` — the only
    // column the index keys on — never changed.
    let txn = store.begin().await.unwrap();
    txn.update_record(&root, &doc(1, 7, None)).await.unwrap();
    txn.commit().await.unwrap();
    assert_eq!(
        index_keys(&kv, LIVE_BY_AUTHOR),
        wanted_keys(LIVE_BY_AUTHOR, &[doc(1, 7, None).to_row()]),
        "restoring a document did not put it back in the index"
    );

    // Both documents are still readable: a partial index restricts the index,
    // not the table.
    let txn = store.begin().await.unwrap();
    let all: Vec<Doc> = txn.query_records(&root, &Query::all()).await.unwrap();
    assert_eq!(all.len(), 2);
}

async fn access_of(store: &RecordStore<MemoryStore>, filter: Expr) -> AccessSummary {
    let txn = store.begin().await.unwrap();
    txn.explain_records::<Doc>(&SecurityContext::superuser(), &Query::all().filter(filter))
        .unwrap()
        .access
}

/// A query the planner can prove lands inside the predicate may read the index;
/// one it cannot may not.
///
/// This is the pair that says the derived predicate reached the planner in a
/// form it can reason about. Emitting a predicate the planner cannot read
/// passes every test above and fails this one — as a missed index rather than
/// a wrong answer, which is why it needs a test of its own.
#[tokio::test]
async fn the_planner_reads_the_derived_index_only_inside_its_predicate() {
    let (store, _kv) = store();
    let author = Expr::eq(Doc::COLUMNS.author, Value::U64(7));

    let inside = access_of(
        &store,
        Expr::is_null(Doc::COLUMNS.deleted_at).and(author.clone()),
    )
    .await;
    assert_eq!(
        inside,
        AccessSummary::IndexScan {
            index: "live_by_author".to_owned()
        },
        "a live-only query did not reach for the live-only index"
    );

    // `author = 7` selects soft-deleted documents too, and the index does not
    // hold them. There is no residual that puts a missing entry back.
    let outside = access_of(&store, author).await;
    assert_eq!(
        outside,
        AccessSummary::TableScan,
        "a partial index answered a query it does not cover"
    );
}
