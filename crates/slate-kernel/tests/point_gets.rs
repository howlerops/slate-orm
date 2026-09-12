//! `IN` over a primary key, which is a set of reads rather than a scan.
//!
//! Loading a set of records by id is the commonest thing an ORM does after
//! loading one, and until this existed it scanned the whole table to do it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::plan::MAX_POINT_GETS;
use slate_kernel::{
    AccessSummary, Action, CmpOp, Expr, Grant, Policy, Principal, Query, RecordStore,
    SecurityCatalog, SecurityContext, Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const NOTES: TableId = TableId(1);
const TENANTS: u64 = 2;
const PER_TENANT: u64 = 200;

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    notes().ordinal_of(name).expect("column exists")
}

fn row(tenant: u64, id: u64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 4)),
        Value::I64(id as i64),
    ])
}

fn open() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("r", NOTES, Action::ALL))
}

fn reader(tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(tenant))
            .with_role("r"),
    )
}

async fn store(
    security: SecurityCatalog,
) -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
    let catalog = Catalog::from_tables([notes()]).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let root = SecurityContext::superuser();

    let txn = loader.begin().await.unwrap();
    let rows: Vec<Row> = (0..TENANTS)
        .flat_map(|t| (0..PER_TENANT).map(move |id| row(t, id)))
        .collect();
    txn.insert_many(&root, &notes(), &rows).await.unwrap();
    txn.commit().await.unwrap();

    // Analysed rather than hand-written. Statistics invented for a test
    // encode whatever the author assumed, and an assumption about the data is
    // exactly what the planner is being tested on.
    let analyzed = {
        let txn = loader.begin().await.unwrap();
        let mut stats = txn.analyze(&root, &notes()).await.unwrap();
        // The seeded corpus is small, and a small table is cheaper to scan
        // whole than to fetch a handful of rows out of: a scan returns ~8000
        // rows per object-store request while a point read costs ~3, so point
        // gets only win once the table is large. That crossover is
        // `planner.rs`'s subject; this file is about whether an `IN` over the
        // key becomes point gets *when it should*, so the row count is set to
        // a size where it should.
        stats.row_count = 5_000_000;
        stats
    };

    let slow = LatencyStore::new(backing, LatencyProfile::free());
    let counters = slow.counters();
    (
        RecordStore::new(slow, catalog, security)
            .with_statistics(Statistics::new().with(NOTES, analyzed)),
        counters,
    )
}

fn ids(values: impl IntoIterator<Item = u64>) -> Expr {
    Expr::In {
        column: col("id"),
        values: values.into_iter().map(Value::U64).collect(),
    }
}

fn seen(rows: &[Row]) -> Vec<u64> {
    rows.iter()
        .map(|r| match r.get(col("id")) {
            Some(Value::U64(id)) => *id,
            other => panic!("expected an id, got {other:?}"),
        })
        .collect()
}

/// The point of the whole thing: reads, not a scan.
#[tokio::test]
async fn an_in_over_the_key_reads_only_those_rows() {
    let (store, counters) = store(open()).await;
    let table = notes();
    let txn = store.begin().await.unwrap();
    let query = Query::all().filter(ids([3, 17, 42, 99]));

    let plan = txn.explain(&reader(0), &table, &query).unwrap();
    assert_eq!(
        plan.access,
        AccessSummary::PointGets { keys: 4 },
        "got {plan}"
    );

    counters.reset();
    let rows = txn
        .execute(&reader(0), &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(seen(&rows), vec![3, 17, 42, 99]);
    assert_eq!(counters.gets(), 4, "one read per key, and no more");
    assert_eq!(counters.scans(), 0, "nothing was scanned");
}

/// Rows come back in key order, the same order a scan would have given, so a
/// caller cannot tell which path ran from the shape of the result.
#[tokio::test]
async fn the_rows_arrive_in_key_order() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(
            &reader(0),
            &notes(),
            &Query::all().filter(ids([99, 3, 42, 17])),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(seen(&rows), vec![3, 17, 42, 99]);
}

/// A repeated value is one row, and a key with no row is simply absent —
/// naming a key is not a claim that it exists.
#[tokio::test]
async fn duplicates_collapse_and_absent_keys_are_skipped() {
    let (store, counters) = store(open()).await;
    let txn = store.begin().await.unwrap();

    counters.reset();
    let rows = txn
        .execute(
            &reader(0),
            &notes(),
            &Query::all().filter(ids([5, 5, 5, 900_000])),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(seen(&rows), vec![5]);
    assert_eq!(
        counters.gets(),
        2,
        "one read for 5, one for the missing key"
    );
}

/// The tenant comes from the policy, not the caller, so an `IN` cannot be used
/// to name a key in someone else's tenant — the key is not even constructible.
#[tokio::test]
async fn an_in_cannot_reach_another_tenant() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    // Tenant 1 has rows with these ids too. A reader in tenant 0 asking for
    // them gets tenant 0's.
    let rows = txn
        .execute(&reader(0), &notes(), &Query::all().filter(ids([1, 2, 3])))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|r| r.get(col("tenant_id")) == Some(&Value::U64(0))),
        "a read crossed a tenant boundary: {rows:?}"
    );
}

/// A row policy still filters what the reads return. The rows are fetched and
/// then rejected, which costs a read but cannot leak one.
#[tokio::test]
async fn a_row_policy_still_applies_to_a_point_get_set() {
    let security = open().policy(Policy::new(
        "small_only",
        NOTES,
        Action::ALL,
        |_: &SecurityContext| Expr::compare(col("size"), CmpOp::Lt, Value::I64(10)),
    ));
    let (store, _) = store(security).await;
    let txn = store.begin().await.unwrap();

    let rows = txn
        .execute(
            &reader(0),
            &notes(),
            &Query::all().filter(ids([1, 2, 50, 60])),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(seen(&rows), vec![1, 2], "the large rows are hidden");
}

/// Another predicate alongside the `IN` still runs: the reads narrow what is
/// looked at, they do not decide what is returned.
#[tokio::test]
async fn a_residual_still_filters_the_rows_read() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let rows = txn
        .execute(
            &reader(0),
            &notes(),
            &Query::all().filter(
                ids([1, 2, 3, 4, 5]).and(Expr::eq(col("kind"), Value::Str("kind-1".into()))),
            ),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(seen(&rows), vec![1, 5], "ids where id % 4 == 1");
}

/// Past some size the reads cost more than the scan they replace, and the cost
/// model says so without anyone special-casing it.
#[tokio::test]
async fn a_large_set_goes_back_to_a_scan() {
    let (store, _) = store(open()).await;
    let table = notes();
    let txn = store.begin().await.unwrap();

    let small = txn
        .explain(&reader(0), &table, &Query::all().filter(ids(0..20)))
        .unwrap();
    assert!(
        matches!(small.access, AccessSummary::PointGets { .. }),
        "got {small}"
    );

    // The corpus is 400 rows; a scan costs about 5. Reading 400 keys costs 25
    // waves, so the scan wins long before the key set does.
    let large = txn
        .explain(&reader(0), &table, &Query::all().filter(ids(0..400)))
        .unwrap();
    assert!(
        matches!(large.access, AccessSummary::TableScan),
        "got {large}"
    );
}

/// And past the hard cap the planner does not even build the keys, whatever
/// the cost model would have said.
#[tokio::test]
async fn an_enormous_set_is_not_expanded() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();
    let huge = txn
        .explain(
            &reader(0),
            &notes(),
            &Query::all().filter(ids(0..(MAX_POINT_GETS as u64 + 1))),
        )
        .unwrap();
    assert!(
        matches!(huge.access, AccessSummary::TableScan),
        "got {huge}"
    );
}

/// An `IN` that does not pin the whole key is not a set of point gets. Getting
/// this wrong would read a handful of keys and call it the answer.
///
/// What it may become instead is a range per value over an index that leads on
/// the column; that path has its own file, `index_in.rs`. All this asserts is
/// the thing that would be a wrong answer rather than a slow one.
#[tokio::test]
async fn an_in_on_a_non_key_column_is_not_a_point_get() {
    let (store, _) = store(open()).await;
    let txn = store.begin().await.unwrap();

    let plan = txn
        .explain(
            &reader(0),
            &notes(),
            &Query::all().filter(Expr::In {
                column: col("kind"),
                values: vec![Value::Str("kind-1".into()), Value::Str("kind-2".into())],
            }),
        )
        .unwrap();
    assert!(
        !matches!(plan.access, AccessSummary::PointGets { .. }),
        "an IN on a non-key column became point gets: {plan}"
    );

    // And it still returns the right rows.
    let rows = txn
        .execute(
            &reader(0),
            &notes(),
            &Query::all().filter(Expr::In {
                column: col("kind"),
                values: vec![Value::Str("kind-1".into())],
            }),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), (PER_TENANT / 4) as usize);
}

/// A limit stops the reads early rather than fetching the whole set.
#[tokio::test]
async fn a_limit_caps_the_reads() {
    let (store, counters) = store(open()).await;
    let txn = store.begin().await.unwrap();

    counters.reset();
    let rows = txn
        .execute(
            &reader(0),
            &notes(),
            &Query::all().filter(ids(0..100)).limit(3),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        counters.gets() <= 4,
        "a limit of three should not read a hundred keys, read {}",
        counters.gets()
    );
}

/// The point-get set must agree with the scan it replaces, for a spread of
/// shapes. A faster path that returns different rows is not a faster path.
#[tokio::test]
async fn point_gets_agree_with_a_scan() {
    let (store, _) = store(open()).await;
    let table = notes();
    let txn = store.begin().await.unwrap();

    let cases: Vec<Expr> = vec![
        ids([0]),
        ids([0, 1, 2]),
        ids([199, 0, 100]),
        ids([7, 7, 8]),
        ids(0..30),
        ids([1, 2, 3]).and(Expr::compare(col("size"), CmpOp::Ge, Value::I64(2))),
        ids([1, 2, 3]).and(Expr::eq(col("kind"), Value::Str("kind-2".into()))),
    ];

    for filter in cases {
        let planned = txn
            .execute(&reader(0), &table, &Query::all().filter(filter.clone()))
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let scanned = txn
            .execute(
                &reader(0),
                &table,
                &Query::all().filter(filter.clone()).using_table_scan(),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(seen(&planned), seen(&scanned), "disagreed on {filter:?}");
    }
}
