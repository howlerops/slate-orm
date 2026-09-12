//! Writer behaviour: conflict retry, and what must never be retried.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, Expr, Grant, KernelError, RecordStore, RetryPolicy, ScanOrder, SecurityCatalog,
    SecurityContext, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::atomic::{AtomicU32, Ordering};

const T: TableId = TableId(1);

fn table() -> TableDef {
    TableDef::builder("t", T)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_email", IndexId(10))
                .column("email")
                .unique(),
        )
        .build()
        .expect("valid schema")
}

fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("w", T, Action::ALL));
    RecordStore::new(MemoryStore::new(), catalog, security)
}

fn row(id: u64) -> Row {
    Row::new(vec![Value::U64(id), Value::Str(format!("u{id}@x.com"))])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

#[tokio::test]
async fn transact_commits_on_success() {
    let store = store();
    let ctx = root();
    let t = table();

    // The realistic call shape: the closure borrows locals from this frame.
    store
        .transact(async |txn| txn.insert(&ctx, &t, &row(1)).await)
        .await
        .unwrap();

    let txn = store.begin().await.unwrap();
    assert!(txn.get(&ctx, &t, &[Value::U64(1)]).await.unwrap().is_some());
}

#[tokio::test]
async fn transact_rolls_back_on_failure() {
    let store = store();
    let ctx = root();
    let t = table();

    let err = store
        .transact(async |txn| {
            txn.insert(&ctx, &t, &row(1)).await?;
            // Fail after writing, so a missing rollback would leave the row.
            Err::<(), _>(KernelError::RowNotFound {
                table: "t".to_owned(),
            })
        })
        .await
        .unwrap_err();
    assert!(matches!(err, KernelError::RowNotFound { .. }));
    assert!(
        store.backend().is_empty(),
        "a failed attempt left writes behind"
    );
}

/// A conflict is a retry signal, and the retry must re-run the closure against a
/// *fresh* snapshot rather than replay the buffered writes.
#[tokio::test]
async fn transact_retries_a_conflict_and_re_reads() {
    let store = store();
    let ctx = root();
    let t = table();
    let attempts = AtomicU32::new(0);

    let written = store
        .transact(async |txn| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            // Second time round, take a different key — only possible because
            // the closure genuinely re-runs rather than being replayed.
            let id = if attempt == 0 { 1 } else { 2 };
            txn.insert(&ctx, &t, &row(id)).await?;

            if attempt == 0 {
                // Another writer takes key 1 while this attempt is open.
                let competitor = store.begin().await?;
                competitor.insert(&ctx, &t, &row(1)).await?;
                competitor.commit().await?;
            }
            Ok(id)
        })
        .await
        .unwrap();

    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "should have retried once"
    );
    assert_eq!(written, 2, "the retry should have observed the new state");

    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&ctx, &t, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "both writers' rows should be present");
}

/// Retrying a deterministic failure turns a clear error into a hang.
#[tokio::test]
async fn transact_does_not_retry_a_deterministic_failure() {
    let store = store();
    let ctx = root();
    let t = table();

    let txn = store.begin().await.unwrap();
    txn.insert(&ctx, &t, &row(1)).await.unwrap();
    txn.commit().await.unwrap();

    let attempts = AtomicU32::new(0);
    let err = store
        .transact(async |txn| {
            attempts.fetch_add(1, Ordering::SeqCst);
            txn.insert(&ctx, &t, &row(1)).await
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, KernelError::DuplicatePrimaryKey { .. }),
        "got {err:?}"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "should not have retried"
    );
}

#[tokio::test]
async fn a_conflict_that_never_clears_reports_the_conflict() {
    let store = store().with_retry_policy(RetryPolicy {
        max_attempts: 3,
        ..RetryPolicy::none()
    });
    let ctx = root();
    let t = table();
    let attempts = AtomicU32::new(0);

    // Every attempt loses to a fresh competitor for the same key. The
    // competitor writes the key and removes it again, so it lands in the commit's
    // write set — enough to conflict — while leaving the row absent, so the next
    // attempt reaches the same point rather than failing as a duplicate.
    let err = store
        .transact(async |txn| {
            attempts.fetch_add(1, Ordering::SeqCst);
            txn.insert(&ctx, &t, &row(1)).await?;
            let competitor = store.begin().await?;
            competitor.upsert(&ctx, &t, &row(1)).await?;
            competitor.delete(&ctx, &t, &[Value::U64(1)]).await?;
            competitor.commit().await?;
            Ok(())
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, KernelError::TransactionConflict),
        "the caller should see the conflict itself, got {err:?}"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

/// Fencing means another writer took over. Retrying past it is a split brain
/// still trying to write, so it must be terminal.
#[test]
fn only_a_conflict_is_retryable() {
    assert!(KernelError::TransactionConflict.is_retryable());
    assert!(!KernelError::WriterFenced.is_retryable());
    assert!(
        !KernelError::DuplicatePrimaryKey {
            table: "t".to_owned()
        }
        .is_retryable()
    );
    assert!(
        !KernelError::AccessDenied {
            table: "t".to_owned(),
            action: "read",
        }
        .is_retryable()
    );
}

#[test]
fn backoff_grows_then_stops_growing() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.backoff_for(1), core::time::Duration::ZERO);

    // Jittered, so assert the envelope rather than exact values.
    let mut previous_ceiling = core::time::Duration::ZERO;
    for attempt in 2..=8 {
        let waited = policy.backoff_for(attempt);
        assert!(
            waited <= policy.max_backoff,
            "attempt {attempt} exceeded the cap"
        );
        assert!(waited > core::time::Duration::ZERO);
        previous_ceiling = previous_ceiling.max(waited);
    }
    assert!(previous_ceiling <= policy.max_backoff);

    // Jitter must actually vary, or writers that collided once collide again.
    let capped = RetryPolicy {
        max_attempts: 10,
        initial_backoff: core::time::Duration::from_millis(64),
        max_backoff: core::time::Duration::from_millis(64),
    };
    let samples: std::collections::BTreeSet<_> = (0..64).map(|_| capped.backoff_for(4)).collect();
    assert!(samples.len() > 1, "backoff was not jittered");

    // And `none()` really means none.
    assert_eq!(RetryPolicy::none().max_attempts, 1);
}
