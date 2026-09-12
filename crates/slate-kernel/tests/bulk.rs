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

// --- updating in bulk ------------------------------------------------------

fn renamed(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("u{id}@y.com")),
        Value::Str(format!("team-{}", (id + 1) % 3)),
        Value::I64(id as i64 + 100),
    ])
}

async fn seeded(
    security: SecurityCatalog,
    ids: std::ops::Range<u64>,
) -> RecordStore<LatencyStore<MemoryStore>> {
    let (store, _) = new_store(security).await;
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &ids.map(row).collect::<Vec<_>>())
        .await
        .unwrap();
    txn.commit().await.unwrap();
    store
}

async fn emails(store: &RecordStore<LatencyStore<MemoryStore>>) -> Vec<String> {
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(&root(), &table(), &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let mut out: Vec<String> = rows
        .into_iter()
        .map(|r| match &r.values()[1] {
            Value::Str(s) => s.clone(),
            other => panic!("email was {other:?}"),
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn a_bulk_update_replaces_every_row_and_moves_its_index_entries() {
    let store = seeded(open(), 0..20).await;
    let rows: Vec<Row> = (0..20).map(renamed).collect();

    let txn = store.begin().await.unwrap();
    txn.update_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();

    assert_eq!(emails(&store).await, {
        let mut want: Vec<String> = (0..20).map(|id| format!("u{id}@y.com")).collect();
        want.sort();
        want
    });

    // The unique index moved with them: the old address is free and the new
    // one is taken.
    let txn = store.begin().await.unwrap();
    let by_old = txn
        .execute(
            &root(),
            &table(),
            &Query::all()
                .filter(Expr::eq(
                    table().ordinal_of("email").unwrap(),
                    Value::Str("u3@x.com".to_owned()),
                ))
                .using_index(IndexId(10)),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(by_old.is_empty(), "the old index entry is still there");
}

/// One missing row fails the whole batch, and writes none of it.
///
/// A batch is one statement. Anything else hands the caller an error and a
/// partly applied change with no way to tell which rows landed — so this checks
/// not just the error but that committing afterwards changes nothing.
#[tokio::test]
async fn a_bulk_update_of_a_row_that_is_not_there_writes_nothing() {
    let store = seeded(open(), 0..5).await;
    let before = emails(&store).await;

    let mut rows: Vec<Row> = (0..5).map(renamed).collect();
    rows.push(renamed(99));

    let txn = store.begin().await.unwrap();
    let refused = txn.update_many(&root(), &table(), &rows).await;
    assert!(
        matches!(refused, Err(KernelError::RowNotFound { .. })),
        "expected a missing row to be refused, got {refused:?}"
    );
    // Committing anyway must land nothing, because nothing was buffered.
    txn.commit().await.unwrap();

    assert_eq!(emails(&store).await, before, "part of the batch landed");
}

/// `update_many` never creates a row, which is the whole difference from
/// `upsert_many`.
#[tokio::test]
async fn a_bulk_update_does_not_insert() {
    let store = seeded(open(), 0..3).await;

    let txn = store.begin().await.unwrap();
    assert!(
        txn.update_many(&root(), &table(), &[renamed(7)])
            .await
            .is_err(),
        "an update created a row"
    );
    txn.rollback();

    // The same rows through `upsert_many` do create it, so the difference is
    // the mode rather than something else about the batch.
    let txn = store.begin().await.unwrap();
    txn.upsert_many(&root(), &table(), &[renamed(7)])
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(all_ids(&store).await.len(), 4);
}

/// It needs `Update`, and nothing else.
///
/// Routing it through the same code as `insert_many` must not make it demand
/// `Insert` as well: two spellings of one operation would then differ on who
/// may call them.
#[tokio::test]
async fn a_bulk_update_needs_only_permission_to_update() {
    let update_only =
        SecurityCatalog::new().grant(Grant::new("w", T, [Action::Read, Action::Update]));
    let store = seeded(update_only, 0..4).await;
    let writer = SecurityContext::new(Principal::new(Value::Str("w".to_owned())).with_role("w"));

    let txn = store.begin().await.unwrap();
    txn.update_many(&writer, &table(), &(0..4).map(renamed).collect::<Vec<_>>())
        .await
        .expect("a role with Update but not Insert may update in bulk");
    txn.commit().await.unwrap();
    assert_eq!(emails(&store).await[0], "u0@y.com");

    // The control: the same role still may not create a row, so the grant
    // really is narrower than `insert_many` needs.
    let txn = store.begin().await.unwrap();
    assert!(
        txn.insert_many(&writer, &table(), &[row(50)])
            .await
            .is_err(),
        "a role without Insert inserted"
    );
}

/// A row the caller may not see reports the same thing as a row that is not
/// there. Which of the two it was is exactly what a policy exists not to say.
#[tokio::test]
async fn a_bulk_update_cannot_reach_a_row_a_policy_hides() {
    let guarded = SecurityCatalog::new()
        .grant(Grant::new("w", T, Action::ALL))
        .enable_rls(T)
        .policy(Policy::new(
            "own_team",
            T,
            Action::ALL,
            |_context: &SecurityContext| {
                Expr::eq(
                    table().ordinal_of("team").unwrap(),
                    Value::Str("team-0".to_owned()),
                )
            },
        ));
    let store = seeded(guarded, 0..6).await;

    let writer = SecurityContext::new(Principal::new(Value::Str("w".to_owned())).with_role("w"));
    // Row 1 is on team-1, which the policy hides.
    let txn = store.begin().await.unwrap();
    let refused = txn.update_many(&writer, &table(), &[renamed(1)]).await;
    assert!(
        matches!(refused, Err(KernelError::RowNotFound { .. })),
        "a hidden row was updated, or reported differently: {refused:?}"
    );

    // The control: row 0 is on team-0 and goes through, so the refusal above
    // is the policy rather than the batch refusing everything. Its team is left
    // alone — `renamed` moves a row to the next team, and a row edited *out* of
    // the policy is refused for a different reason (`RowCheckFailed`), which
    // would make this control prove the wrong thing.
    let stays = Row::new(vec![
        Value::U64(0),
        Value::Str("u0@y.com".to_owned()),
        Value::Str("team-0".to_owned()),
        Value::I64(100),
    ]);
    let txn = store.begin().await.unwrap();
    txn.update_many(&writer, &table(), &[stays])
        .await
        .expect("a visible row is updatable");
    txn.commit().await.unwrap();
}

/// Its reads go together too, the same as an insert's.
///
/// Counted rather than timed, for the reason the insert version gives: the
/// batch issues the same reads, concurrently, so what changes is the waiting
/// and not the count. A count that went *up* would mean the batch had grown a
/// per-row round trip, which is the regression worth catching.
#[tokio::test]
async fn a_bulk_update_does_not_read_once_per_row() {
    let rows: Vec<Row> = (0..100).map(renamed).collect();

    let (store, counters) = new_store(open()).await;
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &(0..100).map(row).collect::<Vec<_>>())
        .await
        .unwrap();
    txn.commit().await.unwrap();
    counters.reset();
    let txn = store.begin().await.unwrap();
    txn.update_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();
    let batched = counters.gets();

    let (other, other_counters) = new_store(open()).await;
    let txn = other.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &(0..100).map(row).collect::<Vec<_>>())
        .await
        .unwrap();
    txn.commit().await.unwrap();
    other_counters.reset();
    let txn = other.begin().await.unwrap();
    for row in &rows {
        txn.update(&root(), &table(), row).await.unwrap();
    }
    txn.commit().await.unwrap();
    let one_at_a_time = other_counters.gets();

    assert!(
        batched > 0,
        "the batch read nothing, so this measures nothing"
    );
    assert_eq!(batched, one_at_a_time);
    assert_eq!(emails(&store).await, emails(&other).await);
}
