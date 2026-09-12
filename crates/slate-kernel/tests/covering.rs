//! Index-only scans: when the row lookup can be skipped, and when it must not
//! be.
//!
//! The win is measured in point reads rather than in time, because that is the
//! quantity the optimisation actually changes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Access, Action, CmpOp, Expr, Grant, Policy, Principal, Projection, RecordStore, ScanOrder,
    SecurityCatalog, SecurityContext, plan_projected,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const NOTES: TableId = TableId(1);
const ROWS: u64 = 200;

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .column("body", ValueType::Str)
        .nullable_column("owner", ValueType::U64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        // Holds kind and size, so a query over just those plus the key needs no
        // row read.
        .index(
            IndexDef::builder("by_kind_size", IndexId(10))
                .column("kind")
                .column("size"),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    notes().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(1),
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 4)),
        Value::I64(id as i64),
        Value::Str("a body that would be a waste to fetch".to_owned()),
        Value::U64(id % 3),
    ])
}

async fn store(
    security: SecurityCatalog,
) -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
    let catalog = Catalog::from_tables([notes()]).expect("catalog");
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let table = notes();
    let root = SecurityContext::superuser();

    let txn = loader.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table, &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    let slow = LatencyStore::new(backing, LatencyProfile::free());
    let counters = slow.counters();
    (RecordStore::new(slow, catalog, security), counters)
}

fn open() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("r", NOTES, Action::ALL))
}

fn reader() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(1))
            .with_role("r"),
    )
}

/// The point of the whole thing: a query an index can answer does no row reads.
#[tokio::test]
async fn a_covered_query_reads_no_rows() {
    let (store, counters) = store(open()).await;
    let table = notes();

    counters.reset();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query_projected(
            &reader(),
            &table,
            Expr::eq(col("kind"), Value::Str("kind-1".into())),
            ScanOrder::Ascending,
            &Projection::Columns(vec![col("id"), col("size")]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), (ROWS / 4) as usize);
    assert_eq!(
        counters.gets(),
        0,
        "a covered query should read no rows at all"
    );

    // The projected columns are populated from the index entry.
    for row in &rows {
        assert!(matches!(row.values()[col("id").0], Value::U64(_)));
        assert!(matches!(row.values()[col("size").0], Value::I64(_)));
        // And the ones not asked for are absent rather than wrong.
        assert_eq!(row.values()[col("body").0], Value::Null);
    }
}

/// Asking for a column the index lacks means the rows must be read after all.
#[tokio::test]
async fn an_uncovered_projection_still_reads_rows() {
    let (store, counters) = store(open()).await;
    let table = notes();

    counters.reset();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query_projected(
            &reader(),
            &table,
            Expr::eq(col("kind"), Value::Str("kind-1".into())),
            ScanOrder::Ascending,
            &Projection::Columns(vec![col("body")]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), (ROWS / 4) as usize);
    assert_eq!(
        counters.gets(),
        ROWS / 4,
        "the body is not in the index, so every row has to be fetched"
    );
    for row in &rows {
        assert!(matches!(row.values()[col("body").0], Value::Str(_)));
    }
}

/// Counting is the case with nothing to project at all.
#[tokio::test]
async fn counting_through_an_index_reads_nothing() {
    let (store, counters) = store(open()).await;
    let table = notes();

    counters.reset();
    let txn = store.begin().await.unwrap();
    let matched = txn
        .query_projected(
            &reader(),
            &table,
            Expr::eq(col("kind"), Value::Str("kind-2".into())),
            ScanOrder::Ascending,
            &Projection::none(),
        )
        .await
        .unwrap()
        .count()
        .await
        .unwrap();

    assert_eq!(matched, (ROWS / 4) as usize);
    assert_eq!(counters.gets(), 0);
}

/// The security filter is part of the predicate, so a policy on a column the
/// index lacks has to force the row read rather than be skipped by the
/// optimisation.
#[tokio::test]
async fn a_policy_on_an_uncovered_column_prevents_the_optimisation() {
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", NOTES, Action::ALL))
        .policy(Policy::new(
            "own_notes",
            NOTES,
            [Action::Read],
            |ctx: &SecurityContext| {
                // `owner` is not in the index.
                Expr::eq(
                    notes().ordinal_of("owner").expect("column"),
                    ctx.principal().id.clone(),
                )
            },
        ));
    let (store, counters) = store(security).await;
    let table = notes();

    counters.reset();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query_projected(
            &reader(),
            &table,
            Expr::eq(col("kind"), Value::Str("kind-1".into())),
            ScanOrder::Ascending,
            // The caller asks only for covered columns...
            &Projection::Columns(vec![col("id"), col("size")]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // ...but the policy reads `owner`, so the rows were fetched and filtered.
    assert!(
        counters.gets() > 0,
        "the policy's column was not covered, so rows had to be read"
    );
    for row in &rows {
        assert_eq!(row.values()[col("owner").0], Value::U64(1));
    }
    assert!(
        rows.len() < (ROWS / 4) as usize,
        "the policy filtered nothing"
    );
}

/// A predicate column outside the index also prevents it.
#[tokio::test]
async fn a_filter_on_an_uncovered_column_prevents_the_optimisation() {
    let table = notes();
    let filter = Expr::eq(col("kind"), Value::Str("kind-1".into())).and(Expr::compare(
        col("body"),
        CmpOp::Ne,
        Value::Str("x".into()),
    ));

    let chosen = plan_projected(
        &table,
        &filter,
        ScanOrder::Ascending,
        &Projection::Columns(vec![col("id")]),
    );
    match chosen.access {
        Access::IndexScan { covering, .. } => assert!(
            !covering,
            "the predicate reads `body`, which the index does not hold"
        ),
        // A table scan is also a correct answer here; what must not happen is a
        // covering index scan.
        other => assert!(matches!(other, Access::TableScan { .. }), "got {other:?}"),
    }
}

#[tokio::test]
async fn asking_for_everything_is_not_covered_by_a_partial_index() {
    let table = notes();
    let chosen = plan_projected(
        &table,
        &Expr::eq(col("kind"), Value::Str("kind-1".into())),
        ScanOrder::Ascending,
        &Projection::All,
    );
    match chosen.access {
        Access::IndexScan { covering, .. } => assert!(!covering),
        other => assert!(matches!(other, Access::TableScan { .. }), "got {other:?}"),
    }
}

/// Whatever the projection, the rows returned must be the rows a full read
/// would have returned.
#[tokio::test]
async fn a_projection_never_changes_which_rows_match() {
    let (store, _) = store(open()).await;
    let table = notes();
    let filter = Expr::eq(col("kind"), Value::Str("kind-3".into())).and(Expr::compare(
        col("size"),
        CmpOp::Ge,
        Value::I64(100),
    ));

    let ids = |rows: Vec<Row>| -> Vec<u64> {
        rows.into_iter()
            .map(|r| match r.values()[col("id").0] {
                Value::U64(v) => v,
                ref other => panic!("id was {other:?}"),
            })
            .collect()
    };

    let txn = store.begin().await.unwrap();
    let full = txn
        .query(&reader(), &table, filter.clone(), ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let projected = txn
        .query_projected(
            &reader(),
            &table,
            filter,
            ScanOrder::Ascending,
            &Projection::Columns(vec![col("id")]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert!(!full.is_empty());
    assert_eq!(ids(full), ids(projected));
}
