//! Adversarial probes on grouping over a join, and on grouping by a value the
//! query computes.
//!
//! `grouped_join_oracle.rs` already requires the three join algorithms to agree
//! and the groups to match a hand-written fold. It builds every join it tests
//! through one helper that never sets `Join::limit` or `Join::offset`, so the
//! interaction between the join's own window and the grouping above it is
//! outside its corpus. That interaction is what the first test here is about.
//!
//! The second pair is the other thing the grouped oracles never draw: a group
//! key that is not a column but a value the query computes. It is in the
//! ordinal space every other computed value lives in, so the grouped read has
//! to narrow a projection around an ordinal the table does not have.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, Grant, Group, Grouping, Join, JoinAlgorithm, Query, RecordStore, Scalar,
    SecurityCatalog, SecurityContext, Side,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);
const BY_AUTHOR: IndexId = IndexId(20);
const BY_SHELF: IndexId = IndexId(21);

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("id", ValueType::U64)
        .column("region", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("id", ValueType::U64)
        .column("author_id", ValueType::U64)
        .column("pages", ValueType::I64)
        .column("shelf", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_author", BY_AUTHOR).column("author_id"))
        .index(IndexDef::builder("by_shelf", BY_SHELF).column("shelf"))
        .build()
        .expect("valid schema")
}

fn author_col(name: &str) -> Ordinal {
    authors().ordinal_of(name).expect("column exists")
}

fn book_col(name: &str) -> Ordinal {
    books().ordinal_of(name).expect("column exists")
}

fn author(id: u64) -> Row {
    Row::new(vec![Value::U64(id), Value::Str(format!("r{}", id % 3))])
}

fn book(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::U64(id % 6),
        Value::I64((id % 7) as i64 * 10),
        Value::Str(format!("s{}", id % 4)),
    ])
}

const AUTHOR_ROWS: u64 = 6;
const BOOK_ROWS: u64 = 24;

async fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([authors(), books()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::ALL))
        .grant(Grant::new("r", BOOKS, Action::ALL));
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let root = SecurityContext::superuser();
    let txn = loader.begin().await.unwrap();
    for id in 0..AUTHOR_ROWS {
        txn.insert(&root, &authors(), &author(id)).await.unwrap();
    }
    for id in 0..BOOK_ROWS {
        txn.insert(&root, &books(), &book(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    RecordStore::new(backing, catalog, security)
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// A grouped read of a join whose own `limit` narrows the rows going in.
///
/// The single-table grouped read drops a query's `limit` and `offset` before it
/// runs — `read::narrowed` says so in as many words, because "aggregating a
/// windowed subset of an unordered result is not a meaningful request".
/// `read::narrowed_join` used to clone the join and replace only the two
/// projections, so a join's `limit` survived into a grouped join and windowed
/// the row stream before the grouper ever saw it.
///
/// A join has no order. Which rows the window kept was therefore whichever ones
/// the chosen algorithm happened to produce first, and the answer was a table
/// of plausible numbers either way: this fixture came back as counts of
/// 3/3/3/3 building the left side and 4/2/4/2 building the right.
#[tokio::test]
async fn a_grouped_join_does_not_depend_on_which_algorithm_served_it() {
    let store = store().await;
    // `shelf` in the joined ordinal space: the right table's columns sit past
    // the left table's width.
    let shelf = Ordinal(authors().columns().len() + book_col("shelf").0);
    let grouping = Grouping::by([shelf], &[Aggregate::Count]);

    let mut answers: Vec<(JoinAlgorithm, Vec<Group>)> = Vec::new();
    for algorithm in [
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::NestedLoop,
    ] {
        let mut join = Join::equating(author_col("id"), book_col("author_id"))
            .left(Query::all())
            .right(Query::all())
            // Half the joined rows. Any window at all is enough; this is the
            // shape a caller writes when they want "the first page".
            .limit(12);
        join.force = Some(algorithm);
        let txn = store.begin().await.unwrap();
        let groups = txn
            .group_by_join(&root(), &authors(), &books(), &join, &grouping)
            .await
            .unwrap();
        answers.push((algorithm, groups));
    }

    let (first_algorithm, first) = &answers[0];
    for (algorithm, groups) in &answers[1..] {
        assert_eq!(
            groups, first,
            "{algorithm:?} grouped a limited join into {groups:?}, where \
             {first_algorithm:?} produced {first:?}"
        );
    }
}

// --- grouping by a computed value -----------------------------------------

/// `GROUP BY length(shelf) ...` — well, `pages / 10`, which is an integer and
/// gives seven groups over the fixture.
fn bucketed() -> Vec<Scalar> {
    vec![Scalar::Div(
        Box::new(Scalar::Column(book_col("pages"))),
        Box::new(Scalar::Literal(Value::I64(10))),
    )]
}

fn computed_ordinal() -> Ordinal {
    Ordinal(books().columns().len())
}

/// A fold written out here: the groups `GROUP BY pages/10` must produce.
fn expected() -> Vec<Group> {
    let mut counts: std::collections::BTreeMap<i64, u64> = std::collections::BTreeMap::new();
    for id in 0..BOOK_ROWS {
        let pages = (id % 7) as i64 * 10;
        *counts.entry(pages / 10).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(bucket, count)| Group {
            key: vec![Value::I64(bucket)],
            values: vec![Value::U64(count)],
        })
        .collect()
}

/// Grouping by a computed value agrees with the fold, on the planner's choice.
#[tokio::test]
async fn grouping_by_a_computed_value_agrees_with_a_fold() {
    let store = store().await;
    let txn = store.begin().await.unwrap();
    let query = Query::all().computing(bucketed());
    let grouping = Grouping::by([computed_ordinal()], &[Aggregate::Count]);
    let got = txn
        .grouped(&root(), &books(), &query, &grouping)
        .await
        .unwrap();
    assert_eq!(got, expected());
}

/// And on every access path, which is the property an index that leaves the
/// computed value's input undecoded would break: the group key would be null
/// for every row and the whole table would come back as one group.
#[tokio::test]
async fn grouping_by_a_computed_value_agrees_down_every_access_path() {
    let store = store().await;
    let grouping = Grouping::by([computed_ordinal()], &[Aggregate::Count]);

    for (name, query) in [
        (
            "table scan",
            Query::all().computing(bucketed()).using_table_scan(),
        ),
        (
            "by_author",
            Query::all().computing(bucketed()).using_index(BY_AUTHOR),
        ),
        (
            "by_shelf",
            Query::all().computing(bucketed()).using_index(BY_SHELF),
        ),
    ] {
        let txn = store.begin().await.unwrap();
        let got = txn
            .grouped(&root(), &books(), &query, &grouping)
            .await
            .unwrap();
        assert_eq!(got, expected(), "GROUP BY pages/10 differed on {name}");
    }
}

// --- grouping ordinals a join does not validate ---------------------------

/// `Join::validate` refuses a `having` ordinal outside the joined space, in as
/// many words: "a `having` ordinal past the joined width names no column at
/// all, and would silently read as null — that is, as a condition nobody
/// wrote." Nothing does the same for a `Grouping`'s ordinals, and the two
/// arrive through the same call.
///
/// Two shapes follow, and both are recorded here as the current behaviour
/// rather than asserted to be wrong — the fix is a refusal, which is a
/// decision rather than an arithmetic error.
///
/// **Past the joined width**: every row groups under null, so a table of
/// numbers comes back with one row in it.
#[tokio::test]
async fn a_grouping_ordinal_past_the_joined_width_groups_everything_under_null() {
    let store = store().await;
    let past = Ordinal(authors().columns().len() + books().columns().len() + 3);
    let join = Join::equating(author_col("id"), book_col("author_id"));
    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([past], &[Aggregate::Count]),
        )
        .await
        .unwrap();
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].key, vec![Value::Null]);
}

/// **A left-side computed value's ordinal** is worse, because it is *in* range.
///
/// `JoinSchema` packs by declared table width, so the ordinal a left-side
/// query gives its first computed value — `left_width + 0` — is the right
/// table's column 0 in the joined space. `JoinedRow::flatten` truncates the
/// left row to the table's width, so the value never reaches the grouper; the
/// group key is the right table's first column instead, with no error
/// anywhere. `records.proto` refuses exactly this reference on `having`
/// ("a computed value has no slot in it; the reference is refused rather than
/// landing on whatever column happens to sit at that offset"); the kernel's
/// `Grouping` accepts it.
#[tokio::test]
async fn a_left_side_computed_ordinal_lands_on_the_right_tables_first_column() {
    let store = store().await;
    let left_width = authors().columns().len();
    let join =
        Join::equating(author_col("id"), book_col("author_id")).left(Query::all().computing([
            Scalar::Upper(Box::new(Scalar::Column(author_col("region")))),
        ]));
    let txn = store.begin().await.unwrap();
    let by_computed = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by([Ordinal(left_width)], &[Aggregate::Count]),
        )
        .await
        .unwrap();

    // Which is exactly grouping by `books.id`, the right table's column 0.
    let txn = store.begin().await.unwrap();
    let by_book_id = txn
        .group_by_join(
            &root(),
            &authors(),
            &books(),
            &join,
            &Grouping::by(
                [Ordinal(left_width + book_col("id").0)],
                &[Aggregate::Count],
            ),
        )
        .await
        .unwrap();

    // Pinned as it stands today, not asserted to be right: the two are the
    // same answer, so the caller's ordinal named a column it did not write.
    assert_eq!(
        by_computed, by_book_id,
        "if this ever stops holding, the kernel has learned to say something \
         about a left-side computed ordinal — check it is a refusal and not a \
         second silent meaning"
    );
    // And it is emphatically not the upper-cased region it names: that has
    // three values, this has one group per book.
    assert!(
        by_computed.len() > 3,
        "grouping by `upper(region)` would give three groups; this gave {}",
        by_computed.len()
    );
}
