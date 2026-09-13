//! What one request can make the server allocate or compute.
//!
//! A join has [`slate_kernel::DEFAULT_BUILD_LIMIT`] and reports
//! `JoinBuildTooLarge` rather than dying. Nothing else that holds unbounded
//! state per request has an equivalent, and nothing bounds the *work* an
//! `IN` list costs per row. These are measurements rather than assertions
//! about a threshold: each records what is currently unbounded.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, Aggregate, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext, SortKey,
    memory::MemoryStore,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::time::Instant;

const T: TableId = TableId(1);
const ROWS: u64 = 2_000;

fn table() -> TableDef {
    TableDef::builder("t", T)
        .column("id", ValueType::U64)
        .column("k", ValueType::I64)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("app", T, Action::ALL))
}

fn app() -> SecurityContext {
    SecurityContext::new(slate_kernel::Principal::new(Value::U64(1)).with_role("app"))
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(
            &root,
            &table(),
            &Row::new(vec![Value::U64(id), Value::I64(id as i64)]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    store
}

/// A large `IN` list is accepted and costs O(list) per scanned row.
///
/// `MAX_POINT_GETS` and `MAX_INDEX_RANGES` stop a large list becoming an
/// access path; they do not stop it staying in the residual, where it is
/// re-scanned linearly for every candidate row.
#[tokio::test]
async fn a_large_in_list_is_accepted_and_costs_per_row() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();

    let run = |n: usize| {
        let values: Vec<Value> = (0..n).map(|i| Value::I64(1_000_000 + i as i64)).collect();
        Query::all().filter(Expr::In {
            column: Ordinal(1),
            values,
        })
    };

    let small = run(8);
    let start = Instant::now();
    let n = txn.count(&app(), &table(), &small).await.unwrap();
    let cheap = start.elapsed();
    assert_eq!(n, 0);

    let big = run(50_000);
    let start = Instant::now();
    let n = txn.count(&app(), &table(), &big).await.unwrap();
    let dear = start.elapsed();
    assert_eq!(n, 0, "neither list matches anything");

    // Recorded rather than asserted as a hard threshold: on a loaded machine
    // the ratio moves, but the shape does not.
    println!(
        "IN(8) over {ROWS} rows: {cheap:?}; IN(50000): {dear:?} \
         ({}x)",
        dear.as_secs_f64() / cheap.as_secs_f64().max(f64::MIN_POSITIVE)
    );
    assert!(
        dear > cheap * 20,
        "expected the list length to show up per row: {cheap:?} vs {dear:?}"
    );
}

/// A `GROUP BY` on a unique column holds one entry per row, with no cap.
#[tokio::test]
async fn a_group_by_holds_every_distinct_key_with_no_limit() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by(
            &app(),
            &table(),
            &Query::all(),
            &[Ordinal(1)],
            &[Aggregate::Count],
        )
        .await
        .unwrap();
    assert_eq!(
        groups.len() as u64,
        ROWS,
        "one group per row, all held in memory at once, and nothing refuses it"
    );
}

/// `COUNT(DISTINCT)` holds every distinct encoded value, with no cap.
#[tokio::test]
async fn count_distinct_holds_every_value_with_no_limit() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let values = txn
        .aggregate(
            &app(),
            &table(),
            &Query::all(),
            &[Aggregate::CountDistinct(Ordinal(1))],
        )
        .await
        .unwrap();
    assert_eq!(values[0], Value::U64(ROWS));
}

/// An unlimited `ORDER BY` materialises the whole result before the first row.
///
/// With a limit the executor uses a bounded heap; without one it collects
/// everything, and nothing caps that.
#[tokio::test]
async fn an_unlimited_sort_materialises_the_whole_result() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(
            &app(),
            &table(),
            &Query::all().sort_by([SortKey::desc(Ordinal(1))]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len() as u64, ROWS);
}
