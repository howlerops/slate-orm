//! Overlapping the row reads an index scan implies.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext, Statistics,
    TableStats,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const T: TableId = TableId(1);
const ROWS: u64 = 240;
const MATCHING: u64 = 60;

fn table() -> TableDef {
    TableDef::builder("t", T)
        .column("id", ValueType::U64)
        .column("bucket", ValueType::U64)
        .column("payload", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_bucket", IndexId(10)).column("bucket"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::U64(id % 4),
        // Not in the index, so the query cannot be answered from it.
        Value::Str("payload".to_owned()),
    ])
}

/// Statistics that make the index the cheaper plan, so the pipelined path is
/// the one under test.
async fn store(profile: LatencyProfile) -> RecordStore<LatencyStore<MemoryStore>> {
    {
        let catalog = Catalog::from_tables([table()]).expect("catalog");
        let backing = MemoryStore::new();
        let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
        let root = SecurityContext::superuser();
        let txn = loader.begin().await.unwrap();
        for id in 0..ROWS {
            txn.insert(&root, &table(), &row(id)).await.unwrap();
        }
        txn.commit().await.unwrap();

        // A big table with a very selective bucket: the planner should reach for
        // the index, which is the path that does a read per row.
        let stats = TableStats::with_row_count(10_000_000).with_column(
            col("bucket"),
            slate_kernel::ColumnStats {
                distinct: 1_000_000,
                null_fraction: 0.0,
            },
        );
        RecordStore::new(
            LatencyStore::new(backing, profile),
            catalog,
            SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL)),
        )
        .with_statistics(Statistics::new().with(T, stats))
    }
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn query() -> Query {
    Query::all().filter(Expr::eq(col("bucket"), Value::U64(1)))
}

/// Overlapping the reads must not change the answer, or their order.
#[tokio::test]
async fn pipelining_preserves_order_and_contents() {
    let store = store(LatencyProfile::free()).await;
    let table = table();
    let txn = store.begin().await.unwrap();

    assert!(
        txn.explain(&root(), &table, &query())
            .unwrap()
            .to_string()
            .contains("Index Scan"),
        "the test needs the non-covering index path"
    );

    let ids = |prefetch: usize| {
        let table = table.clone();
        let store = &store;
        async move {
            let txn = store.begin().await.unwrap();
            let rows = txn
                .execute(&root(), &table, &query())
                .await
                .unwrap()
                .prefetch(prefetch)
                .collect()
                .await
                .unwrap();
            rows.into_iter()
                .map(|r| match r.values()[0] {
                    Value::U64(v) => v,
                    ref other => panic!("id was {other:?}"),
                })
                .collect::<Vec<_>>()
        }
    };

    let serial = ids(1).await;
    assert_eq!(serial.len(), MATCHING as usize);
    assert!(serial.windows(2).all(|w| w[0] < w[1]), "not in key order");

    for prefetch in [2usize, 7, 16, 64, 1000] {
        assert_eq!(
            ids(prefetch).await,
            serial,
            "prefetch {prefetch} changed the result"
        );
    }
}

/// A limit must not be paid for in wasted reads beyond the prefetch depth.
#[tokio::test]
async fn a_limit_bounds_the_wasted_reads() {
    let store = store(LatencyProfile::free()).await;
    let table = table();
    let counters = store.backend().counters();

    counters.reset();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(&root(), &table, &query().limit(5))
        .await
        .unwrap()
        .prefetch(8)
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 5);
    assert!(
        counters.gets() <= 5 + 8,
        "read {} rows to return 5 with a prefetch of 8",
        counters.gets()
    );
}

/// The point of it: with round trips charged, overlapping is dramatically
/// faster than waiting for each read in turn.
#[tokio::test]
async fn overlapping_reads_beats_waiting_for_each() {
    let store = store(LatencyProfile::object_storage()).await;
    let table = table();

    let time_with = |prefetch: usize| {
        let table = table.clone();
        let store = &store;
        async move {
            let started = std::time::Instant::now();
            let txn = store.begin().await.unwrap();
            let count = txn
                .execute(&root(), &table, &query())
                .await
                .unwrap()
                .prefetch(prefetch)
                .count()
                .await
                .unwrap();
            assert_eq!(count, MATCHING as usize);
            started.elapsed()
        }
    };

    let serial = time_with(1).await;
    let pipelined = time_with(16).await;
    println!("{MATCHING} row index scan: serial {serial:?}, prefetch 16 {pipelined:?}");
    assert!(
        pipelined * 3 < serial,
        "expected overlapping to be much faster: {pipelined:?} vs {serial:?}"
    );
}
