//! An inverted index: one entry per term, which is a cardinality this layer
//! assumed away everywhere until F6.
//!
//! Every other index writes exactly one entry per row. The write path compared
//! one old key against one new one, `erase_row` deleted one key, and the
//! planner matched constraints against the column's *value*. A full-text index
//! breaks all three: a row writes as many entries as it has distinct words, an
//! update rewrites only the words that changed, and no constraint on the
//! column says anything about where its terms sort.
//!
//! What is asserted here, in order of how badly it fails when wrong:
//!
//! 1. **The index and a scan agree.** The strongest test in the file, and the
//!    one that catches a tokenizer drifting between the write path and the
//!    predicate: the same `contains` is answered with the index and with a
//!    forced table scan, over generated text, and the two row sets must match.
//!    An index that quietly matches fewer rows is otherwise invisible.
//! 2. **An update rewrites the terms that changed and only those.** Getting
//!    this wrong leaves an entry for a word the row no longer has, which the
//!    index returns and the residual then rejects — a *slower* correct answer,
//!    and so a bug that never shows up as a wrong result.
//! 3. **A delete removes every entry.** Getting this wrong leaves an entry
//!    pointing at a row that is gone.
//! 4. The refusals at declaration, the three-valued treatment of null, and
//!    that the planner uses the index when it can and does not when it cannot.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::plan::{Access, plan_full, plan_hinted};
use slate_kernel::stats::{ColumnStats, TableStats};
use slate_kernel::{
    AccessHint, Action, Expr, Grant, Projection, Query, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext, SortKey,
};
use slate_schema::{
    Catalog, IndexBuilder, IndexDef, IndexId, Ordinal, Row, SchemaError, TableDef, TableId,
    tokenize,
};
use slate_tuple::{Value, ValueType};

const T: TableId = TableId(1);
const BY_TITLE: IndexId = IndexId(10);

fn table() -> TableDef {
    TableDef::builder("docs", T)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .nullable_column("body", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_title", BY_TITLE)
                .column("title")
                .text(),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

fn row(id: u64, title: &str, body: Option<&str>) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(title.to_owned()),
        body.map_or(Value::Null, |b| Value::Str(b.to_owned())),
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn rows() -> Vec<Row> {
    vec![
        row(1, "A Wizard of Earthsea", Some("the first")),
        row(2, "The Left Hand of Darkness", Some("the second")),
        row(3, "The Dispossessed", None),
        // Shares every term with row 1 but one, so a two-term search separates
        // them and a one-term search does not.
        row(4, "A Wizard of Oz", Some("the fourth")),
        // No term of `wizard` anywhere, and a title whose punctuation is the
        // only thing separating its words.
        row(5, "Solaris--Second,Edition", Some("the fifth")),
    ]
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows()).await.unwrap();
    txn.commit().await.unwrap();
    store
}

/// Ids a search returns **through the inverted index**, sorted.
///
/// Hinted, and that is not a detail. The planner's own choice for a `contains`
/// on a fixture this size is a table scan — see
/// `the_planner_costs_a_text_index_like_any_other`, which is why — so every
/// assertion in this file would otherwise be about the predicate and none of
/// them about the index. The first version of this file did exactly that: it
/// would have passed against an index that wrote no entries at all.
///
/// A hint is advice, so a query whose `contains` is on a column this index
/// does not hold falls back rather than failing; that is what the `body` cases
/// below are reading through.
async fn ids(store: &RecordStore<MemoryStore>, filter: Expr) -> Vec<u64> {
    ids_via(store, Query::all().filter(filter).using_index(BY_TITLE)).await
}

/// The same, for a query the caller has already shaped.
async fn ids_via(store: &RecordStore<MemoryStore>, query: Query) -> Vec<u64> {
    let table = table();
    let txn = store.begin().await.unwrap();
    let mut out: Vec<u64> = txn
        .execute(&root(), &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .iter()
        .map(|r| match r.get(col("id")) {
            Some(Value::U64(id)) => *id,
            other => panic!("id came back as {other:?}"),
        })
        .collect();
    out.sort_unstable();
    out
}

#[tokio::test]
async fn a_search_finds_the_rows_whose_text_holds_every_term() {
    let store = seeded().await;

    // One term: both wizards.
    assert_eq!(
        ids(&store, Expr::contains(col("title"), "wizard")).await,
        [1, 4]
    );
    // Two terms, and the second separates them — which is the whole of what
    // "conjunctive" means here.
    assert_eq!(
        ids(&store, Expr::contains(col("title"), "wizard earthsea")).await,
        [1]
    );
    // Case and punctuation are the tokenizer's business, not the caller's.
    assert_eq!(
        ids(&store, Expr::contains(col("title"), "WIZARD, of!")).await,
        [1, 4]
    );
    // A term nothing holds.
    assert!(
        ids(&store, Expr::contains(col("title"), "dragons"))
            .await
            .is_empty()
    );
    // And a word that is a substring of a term is not that term: `LIKE
    // '%wiz%'` would match and this must not, which is the difference the
    // index exists to make.
    assert!(
        ids(&store, Expr::contains(col("title"), "wiz"))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn punctuation_inside_a_title_separates_its_terms() {
    let store = seeded().await;
    // `Solaris--Second,Edition` has no spaces in it at all. A tokenizer that
    // split on whitespace would hold it as one enormous term and none of these
    // would find it.
    for term in ["solaris", "second", "edition", "second edition"] {
        assert_eq!(
            ids(&store, Expr::contains(col("title"), term)).await,
            [5],
            "searching for {term:?}"
        );
    }
}

#[tokio::test]
async fn a_null_text_is_unknown_rather_than_absent() {
    let store = seeded().await;
    // Row 3's body is null. `contains` on it is Unknown for every term, so it
    // is in neither the positive answer nor the negated one — SQL's own rule,
    // and the same one `LIKE` follows.
    assert!(
        ids(&store, Expr::contains(col("body"), "the"))
            .await
            .contains(&1)
    );
    let negated = ids(
        &store,
        Expr::Not(Box::new(Expr::contains(col("body"), "first"))),
    )
    .await;
    assert!(
        !negated.contains(&3),
        "a null body came back from a NOT: {negated:?}"
    );
    assert!(negated.contains(&2), "{negated:?}");
}

#[tokio::test]
async fn a_search_with_no_terms_matches_nothing() {
    let store = seeded().await;
    // `'???'` tokenizes to nothing. Matching everything would hand back the
    // whole table for a query the caller thought was narrow.
    assert!(
        ids(&store, Expr::contains(col("title"), "???"))
            .await
            .is_empty()
    );
    assert!(tokenize("???").is_empty(), "the premise of the case above");
}

#[tokio::test]
async fn an_update_rewrites_the_terms_that_changed_and_keeps_the_rest() {
    let store = seeded().await;
    let table = table();

    let txn = store.begin().await.unwrap();
    txn.update(
        &root(),
        &table,
        &row(1, "A Wizard of Roke", Some("the first")),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // The term that went is gone from the index, and the one that arrived is
    // in it. A write path that only ever added entries would still pass the
    // second of these and fail the first.
    assert_eq!(
        ids(&store, Expr::contains(col("title"), "earthsea")).await,
        Vec::<u64>::new()
    );
    assert_eq!(ids(&store, Expr::contains(col("title"), "roke")).await, [1]);
    // And the terms that did not change still find it, which is what makes
    // this an update rather than a delete and an insert.
    assert_eq!(
        ids(&store, Expr::contains(col("title"), "wizard")).await,
        [1, 4]
    );
}

#[tokio::test]
async fn a_delete_removes_every_one_of_a_rows_entries() {
    let store = seeded().await;
    let table = table();

    let txn = store.begin().await.unwrap();
    txn.delete(&root(), &table, &[Value::U64(1)]).await.unwrap();
    txn.commit().await.unwrap();

    // Every term of the deleted title, not just the first: an `erase_row` that
    // deleted one entry would leave three behind, and each of them points at a
    // row that is not there.
    for term in ["a", "wizard", "of", "earthsea"] {
        let found = ids(&store, Expr::contains(col("title"), term)).await;
        assert!(
            !found.contains(&1),
            "{term:?} still finds the deleted row: {found:?}"
        );
    }
    assert_eq!(
        ids(&store, Expr::contains(col("title"), "wizard")).await,
        [4]
    );
}

#[test]
fn a_search_makes_the_index_a_candidate_and_nothing_else_does() {
    let table = table();
    let hinted = |predicate: Expr| {
        plan_hinted(
            &table,
            std::sync::Arc::new(predicate),
            ScanOrder::Ascending,
            &Projection::All,
            &TableStats::assumed(),
            None,
            &[],
            Some(AccessHint::Index(BY_TITLE)),
            &[],
        )
    };

    match hinted(Expr::contains(col("title"), "wizard")).access {
        Access::IndexScan {
            index, covering, ..
        } => {
            assert_eq!(index, BY_TITLE);
            // An entry holds a term, so the row still has to be read — not
            // even the indexed column can be answered from one.
            assert!(!covering, "a term cannot answer for the column");
        }
        other => panic!("a search did not reach the index: {other:?}"),
    }

    // A predicate on the same column that is not a search says nothing about
    // where its terms sort, so the index is not a candidate *even hinted*: a
    // hint is advice about which correct plan to take, never permission to
    // read an index that cannot answer the question.
    let equality = hinted(Expr::eq(
        col("title"),
        Value::Str("A Wizard of Earthsea".into()),
    ));
    assert!(
        matches!(equality.access, Access::TableScan { .. }),
        "an equality on the column reached the inverted index: {:?}",
        equality.access
    );
}

/// A text index is costed exactly like any other non-covering index.
///
/// The differential, and the reason there is no assertion here that the
/// planner *chooses* it: on this cost model a non-covering index is chosen
/// only when it is expected to return about one row, because following an
/// entry to its row costs `POINT_READ_COST` — three requests, measured — and a
/// scan costs `SCAN_ROW_COST`, a eight-thousandth of one. That is true of
/// every secondary index here and has nothing to do with text.
///
/// So what is asserted is that a `contains` and an equality of the *same*
/// estimated selectivity reach the same verdict at every size. A text index
/// that were costed specially — cheaper, because its entries under one term
/// are in primary-key order, which is a real property and an unmeasured one —
/// would break this, and breaking it deliberately is the shape a future
/// measurement would take.
#[test]
fn the_planner_costs_a_text_index_like_any_other() {
    let text = table();
    let ordinary = TableDef::builder("docs", T)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .nullable_column("body", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("plain_title", IndexId(11)).column("title"))
        .build()
        .expect("valid schema");

    // An equality on a column with `1 / TERM_SELECTIVITY` distinct values is
    // the same estimate a one-term search gets.
    let distinct = (1.0 / slate_kernel::stats::TERM_SELECTIVITY).round() as u64;

    for rows in [1_000u64, 10_000, 100_000, 1_000_000] {
        let kind = |table: &TableDef, predicate: Expr, stats: TableStats| {
            let plan = plan_full(
                table,
                std::sync::Arc::new(predicate),
                ScanOrder::Ascending,
                &Projection::All,
                &stats,
                None,
                &[],
            );
            match plan.access {
                Access::IndexScan { .. } => "index",
                Access::TableScan { .. } => "scan",
                other => panic!("unexpected access: {other:?}"),
            }
        };
        let searched = kind(
            &text,
            Expr::contains(col("title"), "wizard"),
            TableStats::with_row_count(rows),
        );
        let equalled = kind(
            &ordinary,
            Expr::eq(col("title"), Value::Str("wizard".into())),
            TableStats::with_row_count(rows).with_column(
                col("title"),
                ColumnStats {
                    distinct,
                    null_fraction: 0.0,
                },
            ),
        );
        assert_eq!(
            searched, equalled,
            "at {rows} rows a search and an equality of the same selectivity \
             were costed differently"
        );
    }
}

#[test]
fn a_search_ordered_by_the_primary_key_does_not_sort() {
    let table = table();
    let ordered = |sort: &[SortKey]| {
        plan_hinted(
            &table,
            std::sync::Arc::new(Expr::contains(col("title"), "wizard")),
            ScanOrder::Ascending,
            &Projection::All,
            &TableStats::assumed(),
            Some(10),
            sort,
            Some(AccessHint::Index(BY_TITLE)),
            &[],
        )
    };

    // Under one term the entries differ only in the primary key, so the walk
    // is already in that order. Claiming otherwise would sort every match
    // before returning the first, which a `LIMIT` cannot then narrow.
    //
    // Hinted, because the plan has to be the *index* one for this to say
    // anything: a table scan is in primary-key order too, so the unhinted
    // version of this test passes whatever `match_text_index` claims.
    let by_id = ordered(&[SortKey::asc(col("id"))]);
    assert!(
        matches!(by_id.access, Access::IndexScan { .. }),
        "the premise: {:?}",
        by_id.access
    );
    assert!(by_id.sort.is_none(), "a search by id sorted: {by_id:?}");

    // But not by the indexed column: many titles hold one word, so their order
    // under a term is the key's and not the title's.
    let by_title = ordered(&[SortKey::asc(col("title"))]);
    assert!(
        by_title.sort.is_some(),
        "the plan claimed an order it does not produce: {by_title:?}"
    );
}

/// The index and a table scan answer the same question the same way.
///
/// The oracle, and the only test here that would catch the tokenizer drifting
/// between the write path and the predicate — which is the failure that does
/// not look like one: the index simply returns fewer rows, and every other
/// assertion in this file was written against what the index does.
///
/// Generated text rather than hand-written, because the cases worth catching
/// are the ones nobody thought of: a term that is a prefix of another, a word
/// repeated, a title that is only punctuation.
#[tokio::test]
async fn the_index_and_a_scan_agree_on_every_search() {
    const WORDS: [&str; 8] = ["alpha", "al", "beta", "beta", "gamma", "Δ", "9", "alpha9"];

    let table = table();
    let catalog = Catalog::from_tables([table.clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);

    // A deterministic spread: each row takes a different rotation of the word
    // list, so every word appears in some rows and not others.
    let mut seeded = Vec::new();
    for id in 0..40u64 {
        let title: String = (0..4)
            .map(|k| WORDS[((id as usize) + k * 3) % WORDS.len()])
            .collect::<Vec<_>>()
            .join(if id % 3 == 0 { ", " } else { " " });
        seeded.push(row(id + 1, &title, None));
    }
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table, &seeded).await.unwrap();
    txn.commit().await.unwrap();

    for search in [
        "alpha",
        "al",
        "beta",
        "gamma",
        "Δ",
        "9",
        "alpha9",
        "alpha beta",
        "al alpha",
        "gamma Δ 9",
        "nothing",
        "",
        "!!!",
    ] {
        let filter = Expr::contains(col("title"), search);

        // Both paths are *hinted*, and the plans are checked to be the two
        // different things before the rows are compared. Without that this
        // whole test compares a scan against a scan — which is what the first
        // version did, because the planner's own choice for a `contains` on a
        // table this size is a scan. It would have passed against an index
        // that wrote no entries at all.
        let by_index = Query::all().filter(filter.clone()).using_index(BY_TITLE);
        let by_scan = Query::all().filter(filter.clone()).using_table_scan();
        let planned = |query: &Query| {
            plan_hinted(
                &table,
                std::sync::Arc::new(filter.clone()),
                ScanOrder::Ascending,
                &Projection::All,
                &TableStats::assumed(),
                None,
                &[],
                query.hint,
                &[],
            )
            .access
        };
        // The empty searches have no term, so there is nothing to range on and
        // both hints land on a scan. They are still worth running — an empty
        // search must return nothing on both paths — and are exempt from the
        // premise rather than dropped.
        if !tokenize(search).is_empty() {
            assert!(
                matches!(planned(&by_index), Access::IndexScan { .. }),
                "searching for {search:?}: the hinted plan is not an index scan"
            );
            assert!(
                matches!(planned(&by_scan), Access::TableScan { .. }),
                "searching for {search:?}: the hinted plan is not a table scan"
            );
        }

        assert_eq!(
            ids_via(&store, by_index).await,
            ids_via(&store, by_scan).await,
            "searching for {search:?}: the index and a scan disagree"
        );
    }
}

/// The four ways to declare something an inverted index cannot hold.
///
/// One error carrying a reason rather than four variants, because all four are
/// the same mistake and the reader fixing one wants to read what an inverted
/// index *is*.
#[test]
fn a_text_index_that_cannot_hold_terms_is_refused() {
    let base = || {
        TableDef::builder("docs", T)
            .column("id", ValueType::U64)
            .column("title", ValueType::Str)
            .column("year", ValueType::I64)
            .primary_key(["id"])
    };
    let cases: [(&str, IndexBuilder); 4] = [
        (
            "is also unique",
            IndexDef::builder("by_title", BY_TITLE)
                .column("title")
                .text()
                .unique(),
        ),
        (
            "names more than one column",
            IndexDef::builder("by_title", BY_TITLE)
                .column("title")
                .column("year")
                .text(),
        ),
        (
            "keys on a column that is not a string",
            IndexDef::builder("by_year", BY_TITLE).column("year").text(),
        ),
        (
            "also keys on an expression",
            IndexDef::builder("by_title", BY_TITLE)
                .column("title")
                .expression(FirstChar, ValueType::Str)
                .text(),
        ),
    ];
    for (fragment, index) in cases {
        let built = base().index(index).build();
        match built {
            Err(SchemaError::UnindexableText { reason, .. }) => assert!(
                reason.contains(fragment),
                "expected a refusal mentioning {fragment:?}, got {reason:?}"
            ),
            other => panic!("expected a refusal about {fragment:?}, got {other:?}"),
        }
    }
}

/// An expression for the refusal case above; never evaluated.
#[derive(Debug, Clone)]
struct FirstChar;

impl slate_schema::Computed for FirstChar {
    fn value(&self, row: &Row) -> Value {
        match row.get(Ordinal(1)) {
            Some(Value::Str(s)) => Value::Str(s.chars().take(1).collect()),
            _ => Value::Null,
        }
    }
}

/// A search on a tenant-scoped table stays inside the tenant.
///
/// The tenant leads every index key, so the term is not at a fixed offset
/// until the tenant is pinned — which is why `match_text_index` refuses to be
/// a candidate without an equality on it. That refusal is the *safe* half; the
/// unsafe half would be ranging on the term alone, which reads every tenant's
/// entries under that word and hands them to a residual that has already been
/// told which tenant is asking.
///
/// Both halves are asserted: the rows are this tenant's, and the plan really
/// was the index scan rather than a scan that the row policy would have
/// filtered anyway.
#[tokio::test]
async fn a_search_on_a_tenant_scoped_table_stays_inside_the_tenant() {
    const TENANT_A: u128 = 1;
    const TENANT_B: u128 = 2;
    const NOTES: TableId = TableId(2);
    const BY_BODY: IndexId = IndexId(20);

    let notes = TableDef::builder("notes", NOTES)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("body", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_body", BY_BODY).column("body").text())
        .build()
        .expect("valid schema");

    let note = |tenant: u128, id: u64, body: &str| {
        Row::new(vec![
            Value::Uuid(uuid::Uuid::from_u128(tenant)),
            Value::U64(id),
            Value::Str(body.to_owned()),
        ])
    };

    let catalog = Catalog::from_tables([notes.clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new(
        "app",
        NOTES,
        [Action::Read, Action::Insert, Action::Explain],
    ));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    txn.insert_many(
        &root(),
        &notes,
        &[
            note(TENANT_A, 1, "a shared word here"),
            note(TENANT_B, 1, "a shared word there"),
        ],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let caller = SecurityContext::new(
        slate_kernel::Principal::new(Value::U64(7))
            .with_tenant(Value::Uuid(uuid::Uuid::from_u128(TENANT_A)))
            .with_role("app"),
    );
    let query = Query::all()
        .filter(Expr::contains(Ordinal(2), "shared"))
        .using_index(BY_BODY);

    let txn = store.begin().await.unwrap();
    let explained = txn.explain(&caller, &notes, &query).unwrap();
    assert!(
        explained.access.to_string().contains("Index"),
        "the premise: {}",
        explained.access
    );
    let rows = txn
        .execute(&caller, &notes, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "a search crossed tenants: {rows:?}");
    assert_eq!(
        rows[0].get(Ordinal(2)),
        Some(&Value::Str("a shared word here".to_owned()))
    );
}

/// A row with no text contributes no entry, which only the index can say.
///
/// Written because a mutation survived: making a null text write one entry
/// under the empty term broke nothing at all. It is invisible to every search
/// — a search for `''` has no terms and matches nothing, so no query can
/// distinguish the two — and it is still wrong: every row with a null title
/// would share one key, which is a hot spot holding rows that no term can
/// reach.
///
/// So the assertion is against `key_sets` rather than against an answer. That
/// is the one place the cardinality is decided, and looking at it directly is
/// the only way to see a difference the query layer cannot.
#[test]
fn a_row_with_no_text_writes_no_entry() {
    let table = table();
    let index = table.index(BY_TITLE).expect("the index");

    // A null in the indexed column: no entries, the same way a partial index
    // holds none for a row its predicate rejects.
    let missing = Row::new(vec![Value::U64(1), Value::Null, Value::Null]);
    assert!(
        index.key_sets(&missing).is_empty(),
        "a null title wrote an entry"
    );

    // And a title that is only punctuation, which is a string and tokenizes to
    // nothing. Same answer, different reason, and the second is the one a
    // caller can actually produce.
    let silent = Row::new(vec![
        Value::U64(2),
        Value::Str("!!! ---".into()),
        Value::Null,
    ]);
    assert!(
        index.key_sets(&silent).is_empty(),
        "a title with no terms wrote an entry"
    );

    // The premise: an ordinary title does write one per term, so the two
    // assertions above are about *no* entries rather than about a `key_sets`
    // that always returns nothing.
    let ordinary = Row::new(vec![
        Value::U64(3),
        Value::Str("A Wizard of Earthsea".into()),
        Value::Null,
    ]);
    assert_eq!(index.key_sets(&ordinary).len(), 4);
}
