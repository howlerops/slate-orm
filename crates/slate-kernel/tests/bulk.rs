//! Bulk writes: the same guarantees as one-at-a-time, in fewer round trips.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Expr, Grant, KernelError, Policy, Principal, Query, RecordStore, ScanOrder,
    SecurityCatalog, SecurityContext,
};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const T: TableId = TableId(1);

fn table() -> TableDef {
    TableDef::builder("people", T)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .column("team", ValueType::Str)
        .column("age", ValueType::I64)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_email", IndexId(10))
                .column("email")
                .unique(),
        )
        .index(IndexDef::builder("by_team", IndexId(11)).column("team"))
        .build()
        .expect("valid schema")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("u{id}@x.com")),
        Value::Str(format!("team-{}", id % 3)),
        Value::I64(id as i64),
    ])
}

async fn new_store(
    security: SecurityCatalog,
) -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let slow = LatencyStore::new(MemoryStore::new(), LatencyProfile::free());
    let counters = slow.counters();
    (RecordStore::new(slow, catalog, security), counters)
}

fn open() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("w", T, Action::ALL))
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn all_ids(store: &RecordStore<LatencyStore<MemoryStore>>) -> Vec<u64> {
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(&root(), &table(), &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    rows.into_iter()
        .map(|r| match r.values()[0] {
            Value::U64(v) => v,
            ref other => panic!("id was {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_bulk_insert_writes_every_row_and_index_entry() {
    let (store, _) = new_store(open()).await;
    let rows: Vec<Row> = (0..50).map(row).collect();

    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();

    assert_eq!(all_ids(&store).await.len(), 50);

    // The indexes are usable, so the entries really were written.
    let txn = store.begin().await.unwrap();
    let table = table();
    let by_email = table.ordinal_of("email").unwrap();
    let found = txn
        .execute(
            &root(),
            &table,
            &Query::all().filter(Expr::eq(by_email, Value::Str("u7@x.com".into()))),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
}

/// The whole point: the reads a batch needs happen together.
#[tokio::test]
async fn a_bulk_insert_does_not_read_once_per_row() {
    let (store, counters) = new_store(open()).await;
    let rows: Vec<Row> = (0..100).map(row).collect();

    counters.reset();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();
    let batched = counters.gets();

    // The same rows one at a time, for comparison.
    let (other, other_counters) = new_store(open()).await;
    other_counters.reset();
    let txn = other.begin().await.unwrap();
    for row in &rows {
        txn.insert(&root(), &table(), row).await.unwrap();
    }
    txn.commit().await.unwrap();
    let one_at_a_time = other_counters.gets();

    // Both do the same number of reads; the batch issues them concurrently, so
    // the count is unchanged and the waiting is not.
    assert_eq!(batched, one_at_a_time);
    assert_eq!(all_ids(&store).await, all_ids(&other).await);
}

/// Batching must not weaken what it checks.
#[tokio::test]
async fn a_bulk_insert_still_refuses_duplicates() {
    let (store, _) = new_store(open()).await;

    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &[row(1), row(2)])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    // Against a row already stored.
    let txn = store.begin().await.unwrap();
    let err = txn
        .insert_many(&root(), &table(), &[row(3), row(1)])
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::DuplicatePrimaryKey { .. }),
        "got {err:?}"
    );
    txn.rollback();

    // The failed batch wrote nothing.
    assert_eq!(all_ids(&store).await, vec![1, 2]);
}

/// Two rows in one batch can collide with each other, which no amount of
/// reading storage would reveal.
#[tokio::test]
async fn a_bulk_insert_catches_collisions_inside_the_batch() {
    let (store, _) = new_store(open()).await;
    let txn = store.begin().await.unwrap();

    let err = txn
        .insert_many(&root(), &table(), &[row(1), row(2), row(1)])
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::DuplicatePrimaryKey { .. }),
        "got {err:?}"
    );

    // Distinct keys, same unique email.
    let clash = Row::new(vec![
        Value::U64(9),
        Value::Str("u8@x.com".into()),
        Value::Str("team-0".into()),
        Value::I64(9),
    ]);
    let err = txn
        .insert_many(&root(), &table(), &[row(8), clash])
        .await
        .unwrap_err();
    match err {
        KernelError::UniqueViolation { index, .. } => assert_eq!(index, "by_email"),
        other => panic!("expected a unique violation, got {other:?}"),
    }
}

#[tokio::test]
async fn a_bulk_insert_refuses_a_unique_value_already_stored() {
    let (store, _) = new_store(open()).await;
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &[row(1)]).await.unwrap();
    txn.commit().await.unwrap();

    let clash = Row::new(vec![
        Value::U64(2),
        Value::Str("u1@x.com".into()),
        Value::Str("team-0".into()),
        Value::I64(2),
    ]);
    let txn = store.begin().await.unwrap();
    let err = txn
        .insert_many(&root(), &table(), &[clash])
        .await
        .unwrap_err();
    match err {
        KernelError::UniqueViolation { index, .. } => assert_eq!(index, "by_email"),
        other => panic!("expected a unique violation, got {other:?}"),
    }
}

#[tokio::test]
async fn a_bulk_upsert_replaces_and_retires_stale_entries() {
    let (store, _) = new_store(open()).await;
    let table = table();

    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table, &[row(1), row(2)])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let moved = Row::new(vec![
        Value::U64(1),
        Value::Str("moved@x.com".into()),
        Value::Str("team-9".into()),
        Value::I64(99),
    ]);
    let txn = store.begin().await.unwrap();
    txn.upsert_many(&root(), &table, &[moved, row(3)])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(all_ids(&store).await, vec![1, 2, 3]);

    // The old email must no longer resolve.
    let email = table.ordinal_of("email").unwrap();
    let txn = store.begin().await.unwrap();
    let stale = txn
        .execute(
            &root(),
            &table,
            &Query::all().filter(Expr::eq(email, Value::Str("u1@x.com".into()))),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(
        stale.is_empty(),
        "a stale index entry survived a bulk upsert"
    );
}

/// Security is not something a bulk path gets to skip.
#[tokio::test]
async fn a_bulk_write_is_subject_to_the_same_policy() {
    let age = table().ordinal_of("age").unwrap();
    let security = SecurityCatalog::new()
        .grant(Grant::new("w", T, Action::ALL))
        .policy(Policy::new("adults", T, Action::ALL, move |_: &_| {
            Expr::compare(age, slate_kernel::CmpOp::Ge, Value::I64(18))
        }));
    let (store, _) = new_store(security).await;
    let member = SecurityContext::new(Principal::new(Value::U64(1)).with_role("w"));

    let txn = store.begin().await.unwrap();
    let err = txn
        .insert_many(&member, &table(), &[row(20), row(5)])
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::RowCheckFailed { .. }),
        "got {err:?}"
    );
    txn.rollback();

    // And a batch that satisfies the policy goes through.
    let txn = store.begin().await.unwrap();
    txn.insert_many(&member, &table(), &[row(20), row(30)])
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(all_ids(&store).await, vec![20, 30]);
}

#[tokio::test]
async fn an_empty_batch_is_not_an_error() {
    let (store, counters) = new_store(open()).await;
    counters.reset();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &[]).await.unwrap();
    assert_eq!(counters.gets(), 0);
}

/// Ordering must survive the concurrency: a batch is written as written.
#[tokio::test]
async fn a_bulk_write_preserves_row_identity() {
    let (store, _) = new_store(open()).await;
    let rows: Vec<Row> = (0..40).rev().map(row).collect();

    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let stored = txn
        .execute(&root(), &table(), &Query::all().order(ScanOrder::Ascending))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(stored.len(), 40);
    for (index, stored) in stored.iter().enumerate() {
        assert_eq!(*stored, row(index as u64), "row {index} came back wrong");
    }
}
