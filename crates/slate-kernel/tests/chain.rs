//! Joining more than two tables.
//!
//! Three tables: authors, books, and the publisher each book came from. The
//! shape an ORM actually asks for — walk down an association, then down
//! another — and the shape where a mistake compounds, because step two runs
//! over whatever step one left behind.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Chain, ChainRow, CmpOp, Expr, Grant, Join, JoinAlgorithm, JoinSchema, JoinStep,
    KernelError, Policy, Principal, Query, RecordStore, SecurityCatalog, SecurityContext, Side,
    Statistics, TableStats,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);
const PUBLISHERS: TableId = TableId(3);

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .nullable_column("author_id", ValueType::U64)
        .nullable_column("publisher_id", ValueType::U64)
        .column("title", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_author", IndexId(20)).column("author_id"))
        .build()
        .expect("valid schema")
}

fn publishers() -> TableDef {
    TableDef::builder("publishers", PUBLISHERS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("house", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn a(name: &str) -> Ordinal {
    authors().ordinal_of(name).expect("column exists")
}
fn b(name: &str) -> Ordinal {
    books().ordinal_of(name).expect("column exists")
}
fn p(name: &str) -> Ordinal {
    publishers().ordinal_of(name).expect("column exists")
}

fn schema() -> JoinSchema {
    JoinSchema::over([&authors(), &books(), &publishers()])
}

fn tables() -> Vec<TableDef> {
    vec![authors(), books(), publishers()]
}

/// `authors -> books -> publishers`, keyed the obvious way.
fn walk() -> Chain {
    let at = schema();
    Chain::start()
        .join(JoinStep::equating(at.at(0, a("id")), b("author_id")))
        .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")))
}

fn open() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::EVERYTHING))
        .grant(Grant::new("r", BOOKS, Action::EVERYTHING))
        .grant(Grant::new("r", PUBLISHERS, Action::EVERYTHING))
}

fn reader(tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(tenant))
            .with_role("r"),
    )
}

/// Ursula has two books, one from each publisher. Iain has one, from a
/// publisher that does not exist. Nobody has none at all.
async fn store(
    security: SecurityCatalog,
) -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
    let catalog = Catalog::from_tables(tables()).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let root = SecurityContext::superuser();

    let author = |id: u64, name: &str| {
        Row::new(vec![
            Value::U64(1),
            Value::U64(id),
            Value::Str(name.to_owned()),
        ])
    };
    let book = |id: u64, author: Option<u64>, publisher: Option<u64>, title: &str| {
        Row::new(vec![
            Value::U64(1),
            Value::U64(id),
            author.map_or(Value::Null, Value::U64),
            publisher.map_or(Value::Null, Value::U64),
            Value::Str(title.to_owned()),
        ])
    };
    let publisher = |id: u64, house: &str| {
        Row::new(vec![
            Value::U64(1),
            Value::U64(id),
            Value::Str(house.to_owned()),
        ])
    };

    let txn = loader.begin().await.unwrap();
    for row in [author(1, "Ursula"), author(2, "Iain"), author(3, "Nobody")] {
        txn.insert(&root, &authors(), &row).await.unwrap();
    }
    for row in [
        book(10, Some(1), Some(100), "A Wizard of Earthsea"),
        book(11, Some(1), Some(101), "The Dispossessed"),
        book(12, Some(2), Some(999), "Consider Phlebas"),
        book(13, None, Some(100), "Anonymous"),
    ] {
        txn.insert(&root, &books(), &row).await.unwrap();
    }
    for row in [publisher(100, "Parnassus"), publisher(101, "Harper")] {
        txn.insert(&root, &publishers(), &row).await.unwrap();
    }
    txn.commit().await.unwrap();

    let slow = LatencyStore::new(backing, LatencyProfile::free());
    let counters = slow.counters();
    (RecordStore::new(slow, catalog, security), counters)
}

fn refs(tables: &[TableDef]) -> Vec<&TableDef> {
    tables.iter().collect()
}

fn houses(rows: &[ChainRow]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| match r.at(2).and_then(|row| row.get(p("house"))) {
            Some(Value::Str(s)) => s.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    out.sort();
    out
}

fn shape(rows: &[ChainRow]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| {
            format!(
                "{:?}|{:?}|{:?}",
                r.at(0).and_then(|row| row.get(a("name"))),
                r.at(1).and_then(|row| row.get(b("title"))),
                r.at(2).and_then(|row| row.get(p("house"))),
            )
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn three_tables_join_in_order() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();

    let rows = txn
        .chain(&reader(1), &refs(&defs), &walk())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // Ursula's two books have real publishers; Iain's does not; the
    // anonymous book has no author, so it never reaches step two.
    assert_eq!(houses(&rows), vec!["Harper", "Parnassus"]);
    assert!(rows.iter().all(ChainRow::is_complete));
    assert!(rows.iter().all(|r| r.len() == 3));
}

/// A chain must agree with the two-table joins it decomposes into. If it does
/// not, one of the two is wrong and there is no way to tell which from inside.
#[tokio::test]
async fn a_chain_agrees_with_the_joins_it_composes() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();

    let chained = txn
        .chain(&reader(1), &refs(&defs), &walk())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // The same thing as two separate joins: authors to books, then each
    // resulting book to its publisher.
    let pairs = txn
        .join(
            &reader(1),
            &authors(),
            &books(),
            &Join::equating(a("id"), b("author_id")),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let mut expected = Vec::new();
    for pair in &pairs {
        let book = pair.right.as_ref().unwrap();
        let publisher_id = book.get(b("publisher_id")).unwrap().clone();
        let found = txn
            .query(
                &reader(1),
                &publishers(),
                Expr::eq(p("id"), publisher_id),
                slate_kernel::ScanOrder::Ascending,
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        for publisher in found {
            expected.push(format!(
                "{:?}|{:?}|{:?}",
                pair.left.as_ref().and_then(|l| l.get(a("name"))),
                book.get(b("title")),
                publisher.get(p("house")),
            ));
        }
    }
    expected.sort();
    assert_eq!(shape(&chained), expected);
}

/// Both algorithms must produce the same chain, per step. The planner is free
/// to pick differently at each one, so a disagreement would be a bug that
/// appears only when the statistics change.
#[tokio::test]
async fn every_algorithm_agrees_at_every_step() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();
    let at = schema();

    let mut seen: Option<Vec<String>> = None;
    for first in [
        JoinAlgorithm::Hash { build: Side::Right },
        JoinAlgorithm::NestedLoop,
    ] {
        for second in [
            JoinAlgorithm::Hash { build: Side::Right },
            JoinAlgorithm::NestedLoop,
        ] {
            let chain = Chain::start()
                .join(JoinStep::equating(at.at(0, a("id")), b("author_id")).using(first))
                .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")).using(second));
            // The force must actually take, or this test compares one
            // algorithm with itself and proves nothing.
            let plan = txn.explain_chain(&reader(1), &refs(&defs), &chain).unwrap();
            assert!(
                core::mem::discriminant(&plan.steps[0].algorithm)
                    == core::mem::discriminant(&first)
                    && core::mem::discriminant(&plan.steps[1].algorithm)
                        == core::mem::discriminant(&second),
                "forcing was ignored: {:?} then {:?}",
                plan.steps[0].algorithm,
                plan.steps[1].algorithm
            );
            let rows = txn
                .chain(&reader(1), &refs(&defs), &chain)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            let got = shape(&rows);
            match &seen {
                None => seen = Some(got),
                Some(first_shape) => {
                    assert_eq!(&got, first_shape, "{first:?} then {second:?} disagreed");
                }
            }
        }
    }
}

/// An outer step preserves what the step before it accumulated, not just the
/// first table's rows — which is the thing that is easy to get wrong once
/// there is more than one step.
#[tokio::test]
async fn an_outer_step_preserves_the_accumulated_row() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();
    let at = schema();

    let chain = Chain::start()
        .join(JoinStep::equating(at.at(0, a("id")), b("author_id")).left_outer())
        .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")).left_outer());

    let rows = txn
        .chain(&reader(1), &refs(&defs), &chain)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // Ursula x2 with publishers, Iain with a book but no publisher, Nobody
    // with neither.
    assert_eq!(rows.len(), 4, "{:?}", shape(&rows));
    assert_eq!(rows.iter().filter(|r| r.is_complete()).count(), 2);

    // Nobody wrote nothing, so both later tables are absent — and the row is
    // still three wide, or the ordinal space would not line up.
    let nobody = rows
        .iter()
        .find(|r| r.at(0).and_then(|x| x.get(a("name"))) == Some(&Value::Str("Nobody".into())))
        .expect("the author with no books survived the first outer step");
    assert_eq!(nobody.len(), 3);
    assert!(nobody.at(1).is_none() && nobody.at(2).is_none());

    // Iain has a book whose publisher does not exist: kept by the second
    // outer step, with the book still attached.
    let iain = rows
        .iter()
        .find(|r| r.at(0).and_then(|x| x.get(a("name"))) == Some(&Value::Str("Iain".into())))
        .expect("Iain survived");
    assert!(iain.at(1).is_some(), "his book should still be there");
    assert!(iain.at(2).is_none(), "his publisher should not");
}

/// A step can join back to any earlier table, not only the one before it.
#[tokio::test]
async fn a_step_can_join_back_past_the_previous_table() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();
    let at = schema();

    // Deliberately odd but well-formed: match publishers whose id equals the
    // *author's* id times nothing — use the author id directly, which matches
    // no publisher, to prove the reference reaches table 0 rather than being
    // silently read from table 1.
    let chain = Chain::start()
        .join(JoinStep::equating(at.at(0, a("id")), b("author_id")))
        .join(JoinStep::equating(at.at(0, a("id")), p("id")));

    let rows = txn
        .chain(&reader(1), &refs(&defs), &chain)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "author ids are 1..3 and publisher ids are 100, 101: {:?}",
        shape(&rows)
    );

    // And the same chain keyed on the book's publisher does find rows, so the
    // emptiness above is the reference and not a broken chain.
    let works = Chain::start()
        .join(JoinStep::equating(at.at(0, a("id")), b("author_id")))
        .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")));
    let rows = txn
        .chain(&reader(1), &refs(&defs), &works)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
}

/// Every table in the chain is read through its own secured plan, so a policy
/// on the third one applies as surely as one on the first.
#[tokio::test]
async fn a_policy_on_any_table_in_the_chain_applies() {
    let security = open().policy(Policy::new(
        "one_house",
        PUBLISHERS,
        Action::EVERYTHING,
        |_: &SecurityContext| Expr::eq(p("house"), Value::Str("Harper".into())),
    ));
    let (store, _) = store(security).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();

    let rows = txn
        .chain(&reader(1), &refs(&defs), &walk())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(houses(&rows), vec!["Harper"]);
}

/// Reading without a grant on the last table is refused, not silently empty.
#[tokio::test]
async fn a_chain_needs_a_grant_on_every_table() {
    let partial = SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::EVERYTHING))
        .grant(Grant::new("r", BOOKS, Action::EVERYTHING));
    let (store, _) = store(partial).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();

    let err = txn
        .chain(&reader(1), &refs(&defs), &walk())
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::AccessDenied { .. }),
        "got {err:?}"
    );
}

/// A chain's shape can disagree with its tables, or reference a table it has
/// not read yet. Both are caught before anything is read.
#[tokio::test]
async fn a_malformed_chain_is_refused() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();
    let at = schema();

    // Too few tables for the steps.
    let two = vec![authors(), books()];
    let err = txn
        .chain(&reader(1), &refs(&two), &walk())
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinNotSupported { .. }),
        "got {err:?}"
    );

    // A forward reference: step one joining to a table read at step two.
    let forward = Chain::start()
        .join(JoinStep::equating(at.at(2, p("id")), b("author_id")))
        .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")));
    let err = txn
        .chain(&reader(1), &refs(&defs), &forward)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinNotSupported { .. }),
        "got {err:?}"
    );

    // No condition at all.
    let cross = Chain::start()
        .join(JoinStep::on([]))
        .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")));
    let err = txn
        .chain(&reader(1), &refs(&defs), &cross)
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinNotSupported { .. }),
        "got {err:?}"
    );

    // And a chain with no steps is a single-table query, which has its own
    // call and should be used instead of a chain of one.
    let err = txn
        .chain(&reader(1), &[&authors()], &Chain::start())
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinNotSupported { .. }),
        "got {err:?}"
    );
}

/// A condition on a step can span the table being added and any earlier one.
#[tokio::test]
async fn a_step_condition_can_span_earlier_tables() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();
    let at = schema();

    // Both u64: keep only books whose id is above their author's.
    let chain =
        Chain::start()
            .join(
                JoinStep::equating(at.at(0, a("id")), b("author_id")).having(
                    Expr::compare_columns(at.at(1, b("id")), CmpOp::Gt, at.at(0, a("id"))),
                ),
            )
            .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")));
    let rows = txn
        .chain(&reader(1), &refs(&defs), &chain)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "book ids 10 and 11 are both above author 1");

    // Reversed, nothing survives step one, so step two has nothing to do.
    let none =
        Chain::start()
            .join(
                JoinStep::equating(at.at(0, a("id")), b("author_id")).having(
                    Expr::compare_columns(at.at(1, b("id")), CmpOp::Lt, at.at(0, a("id"))),
                ),
            )
            .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")));
    let rows = txn
        .chain(&reader(1), &refs(&defs), &none)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(rows.is_empty());
}

/// What each step actually accumulated, which is the first thing to look at
/// when a chain is slow and the estimates said it would not be.
#[tokio::test]
async fn a_chain_reports_what_each_step_produced() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();

    let cursor = txn.chain(&reader(1), &refs(&defs), &walk()).await.unwrap();
    // Three authors, three author-book pairs, two of which had a publisher.
    assert_eq!(cursor.step_counts(), &[3, 3, 2]);

    let plan = txn
        .explain_chain(&reader(1), &refs(&defs), &walk())
        .unwrap();
    assert_eq!(plan.steps.len(), 2);
    assert!(plan.estimated_cost > 0.0);
    let (index, _) = plan.most_expensive_step().expect("two steps");
    assert!(index < 2);
}

/// The accumulated result is held in memory, so it is bounded and reports
/// rather than being killed by the allocator — the same guard a hash join's
/// build side has, applied at every step.
#[tokio::test]
async fn an_accumulated_result_is_bounded() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let txn = store.begin().await.unwrap();

    let err = txn
        .chain(&reader(1), &refs(&defs), &walk().build_limit(1))
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::JoinBuildTooLarge { .. }),
        "got {err:?}"
    );
}

/// Walking down from one record is the shape an ORM asks for, and it is where
/// the loop should win — but only once the inner table is *very* much larger
/// than the rows fetched from it.
///
/// This test used to assert a loop at a hundred thousand inner rows, and that
/// was wrong: measured against object storage, a scan of two hundred thousand
/// rows costs 25 requests and 0.37 s, while four hundred point reads cost
/// ~1,221 requests and 3.73 s. ~~Point reads are about three requests each~~
/// — that figure does not reproduce; #269 re-measured it four ways at about
/// one request per row and `POINT_READ_COST` is 1.0. A scan still returns
/// about eight thousand rows per request, so the crossover sits around
/// **twelve** rows fetched per hundred thousand scanned rather than four —
/// still orders of magnitude further toward scanning than the model this test
/// was written against believed, which is why the assertion below did not
/// move. See `slate-slatedb`'s `cost_calibration` example and
/// `stats.rs`'s table of the four measurements.
///
/// So the inner table here is a hundred *million* rows, which is where a probe
/// genuinely wins, and the hundred-thousand case now asserts the opposite.
#[tokio::test]
async fn the_planner_picks_a_loop_when_the_accumulated_side_is_small() {
    let (store, _) = store(open()).await;
    let defs = tables();
    let store = store.with_statistics(
        Statistics::new()
            .with(AUTHORS, TableStats::with_row_count(10_000_000))
            .with(
                BOOKS,
                // Ten million distinct authors over a hundred million books:
                // ten books each, which is the shape an ORM walks. Without
                // this the default distinct count of 100 would put a million
                // books under every author, where scanning is obviously right.
                TableStats::with_row_count(100_000_000).with_column(
                    b("author_id"),
                    slate_kernel::ColumnStats {
                        distinct: 10_000_000,
                        null_fraction: 0.0,
                    },
                ),
            )
            .with(PUBLISHERS, TableStats::with_row_count(1_000)),
    );
    let txn = store.begin().await.unwrap();
    let at = schema();

    let one = Chain::from(Query::all().filter(Expr::eq(a("id"), Value::U64(1))))
        .join(JoinStep::equating(at.at(0, a("id")), b("author_id")))
        .join(JoinStep::equating(at.at(1, b("publisher_id")), p("id")));
    let plan = txn.explain_chain(&reader(1), &refs(&defs), &one).unwrap();
    assert!(
        matches!(plan.steps[0].algorithm, JoinAlgorithm::NestedLoop),
        "one author against a hundred million books should probe: {:?}",
        plan.steps[0].algorithm
    );

    // Whole tables, and the hash step wins instead.
    let plan = txn
        .explain_chain(&reader(1), &refs(&defs), &walk())
        .unwrap();
    assert!(
        matches!(plan.steps[0].algorithm, JoinAlgorithm::Hash { .. }),
        "ten thousand authors should not each be a probe: {:?}",
        plan.steps[0].algorithm
    );

    // And at a hundred thousand inner rows the scan wins, which is the case
    // this test asserted backwards until object storage was measured.
    drop(txn);
    let store = store.with_statistics(
        Statistics::new()
            .with(AUTHORS, TableStats::with_row_count(10_000))
            .with(BOOKS, TableStats::with_row_count(100_000))
            .with(PUBLISHERS, TableStats::with_row_count(1_000)),
    );
    let txn = store.begin().await.unwrap();
    let plan = txn.explain_chain(&reader(1), &refs(&defs), &one).unwrap();
    assert!(
        matches!(plan.steps[0].algorithm, JoinAlgorithm::Hash { .. }),
        "a hundred thousand inner rows is cheaper to scan than to probe: {:?}",
        plan.steps[0].algorithm
    );
}
