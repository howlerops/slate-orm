//! The retry helper, and the escape hatch for callers it cannot serve.
//!
//! [`RecordStore::transact`] takes an `AsyncFn`, which is what lets the body
//! borrow both the transaction it is handed and the caller's own locals. That
//! is the right bound for almost everyone and an impossible one for callers
//! inside a `#[async_trait]` method: the trait's futures are boxed with a
//! `Send` bound, and the compiler cannot prove `Send` for a higher-ranked
//! future built over a borrowed transaction. Swapping `transact_boxed` for
//! `transact` in `Service::write` below gives three copies of
//!
//! ```text
//! error: implementation of `Send` is not general enough
//!   = note: `Send` would have to be implemented for the type `&'0 u64`, for any lifetime `'0`...
//! ```
//!
//! pointing at the method signature and naming types the caller never wrote.
//! There is nothing to annotate.
//!
//! `transact_boxed` exists for that position, so the test for it has to *be*
//! in that position. A test that called it from an ordinary `async fn` would
//! pass whether or not the signature solved the problem it exists to solve —
//! the interesting part is not that the function works, it is that this call
//! site compiles at all.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use async_trait::async_trait;
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Grant, KernelError, Query, RecordStore, SecurityCatalog, SecurityContext,
};
use slate_schema::{Catalog, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const T: TableId = TableId(1);

fn table() -> TableDef {
    TableDef::builder("notes", T)
        .column("id", ValueType::U64)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn note(id: u64, body: &str) -> Row {
    Row::new(vec![Value::U64(id), Value::Str(body.to_owned())])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn store() -> Arc<RecordStore<MemoryStore>> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("w", T, Action::ALL));
    Arc::new(RecordStore::new(MemoryStore::new(), catalog, security))
}

async fn count(store: &RecordStore<MemoryStore>) -> usize {
    let txn = store.begin().await.unwrap();
    txn.execute(&root(), &table(), &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .len()
}

/// A trait shaped like the one a gRPC service implements: `#[async_trait]`,
/// with `Send` futures, taking `&self`.
#[async_trait]
trait Writes: Send + Sync {
    async fn write(&self, id: u64, body: String) -> Result<(), KernelError>;
}

struct Service {
    store: Arc<RecordStore<MemoryStore>>,
    /// How many times the closure was asked for a future.
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl Writes for Service {
    async fn write(&self, id: u64, body: String) -> Result<(), KernelError> {
        let attempts = Arc::clone(&self.attempts);
        self.store
            .transact_boxed(move |txn| {
                // Cloned into each future rather than borrowed from the
                // closure: the returned future may borrow the transaction and
                // nothing else, which is the whole cost of this signature.
                let (body, attempts) = (body.clone(), Arc::clone(&attempts));
                Box::pin(async move {
                    attempts.fetch_add(1, Ordering::Relaxed);
                    txn.insert(&root(), &table(), &note(id, &body)).await
                })
            })
            .await
    }
}

/// The point of the whole signature: this call site compiles.
#[tokio::test]
async fn it_can_be_called_from_an_async_trait_method() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let service = Service {
        store: store(),
        attempts: Arc::clone(&attempts),
    };

    service.write(1, "hello".to_owned()).await.unwrap();
    assert_eq!(count(&service.store).await, 1);
    assert_eq!(attempts.load(Ordering::Relaxed), 1);

    // And it is a real transaction, so the same key twice is a duplicate
    // rather than a silent overwrite.
    let refused = service.write(1, "again".to_owned()).await;
    assert!(
        matches!(refused, Err(KernelError::DuplicatePrimaryKey { .. })),
        "expected a duplicate, got {refused:?}"
    );
    assert_eq!(count(&service.store).await, 1);
}

/// It commits, and it rolls back.
#[tokio::test]
async fn a_failing_body_leaves_nothing_behind() {
    let store = store();
    let outcome: Result<(), KernelError> = store
        .transact_boxed(|txn| {
            Box::pin(async move {
                txn.insert(&root(), &table(), &note(1, "written")).await?;
                Err(KernelError::RowNotFound {
                    table: "notes".to_owned(),
                })
            })
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(count(&store).await, 0, "a rolled-back write landed");
}

/// A non-retryable error is returned rather than retried forever.
#[tokio::test]
async fn a_body_that_cannot_succeed_is_not_retried() {
    let store = store();
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&attempts);
    let outcome: Result<(), KernelError> = store
        .transact_boxed(move |_txn| {
            let counted = Arc::clone(&counted);
            Box::pin(async move {
                counted.fetch_add(1, Ordering::Relaxed);
                Err(KernelError::RowNotFound {
                    table: "notes".to_owned(),
                })
            })
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        1,
        "a hopeless body was retried"
    );
}

/// It retries a conflict, which is the reason to use it rather than `begin`.
///
/// The body is asked for a fresh future each attempt and reads inside it, so
/// the retry sees the winner's write rather than the snapshot it started from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_retries_a_conflict_and_re_reads() {
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table(), &note(1, "a")).await.unwrap();
    txn.commit().await.unwrap();

    let attempts = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for n in 0..8u64 {
        let store = Arc::clone(&store);
        let attempts = Arc::clone(&attempts);
        tasks.push(tokio::spawn(async move {
            store
                .transact_boxed(move |txn| {
                    let attempts = Arc::clone(&attempts);
                    Box::pin(async move {
                        attempts.fetch_add(1, Ordering::Relaxed);
                        // Read-modify-write on one row, which is what makes
                        // these contend at all.
                        let current = txn
                            .get(&root(), &table(), &[Value::U64(1)])
                            .await?
                            .ok_or(KernelError::TransactionConflict)?;
                        let body = match &current.values()[1] {
                            Value::Str(s) => format!("{s}{n}"),
                            other => panic!("body was {other:?}"),
                        };
                        txn.update(&root(), &table(), &note(1, &body)).await
                    })
                })
                .await
        }));
    }
    for task in tasks {
        task.await.unwrap().expect("a writer gave up");
    }

    // Eight appends to one row, each reading what the last wrote: the body is
    // "a" plus eight digits. A lost update makes it shorter, and a double-apply
    // longer.
    let txn = store.begin().await.unwrap();
    let row = txn
        .get(&root(), &table(), &[Value::U64(1)])
        .await
        .unwrap()
        .unwrap();
    match &row.values()[1] {
        Value::Str(body) => assert_eq!(body.len(), 9, "the appends do not add up: {body}"),
        other => panic!("body was {other:?}"),
    }
    assert!(
        attempts.load(Ordering::Relaxed) >= 8,
        "fewer attempts than writers"
    );
}

/// It may borrow the caller's locals, which is what makes it usable for the
/// head node rather than only for closures over owned data.
///
/// The lifetime on the transaction is named, not higher-ranked. Quantifying it
/// too (`&'t RecordTransaction<'t>`) compiles here and then forces every
/// capture to be `'static`, which fails at the one call site the signature
/// exists for — the head node's autocommit hands the body a `&SecurityContext`
/// and a batch of rows it does not own. So this test captures by reference on
/// purpose.
#[tokio::test]
async fn the_body_may_borrow_the_callers_locals() {
    let store = store();
    let context = root();
    let rows = vec![note(1, "borrowed"), note(2, "also borrowed")];
    // Neither of these is `'static`, and neither is cloned into the future.
    let context = &context;
    let rows = &rows;

    store
        .transact_boxed(move |txn| {
            Box::pin(async move {
                txn.insert_many(context, &table(), rows).await?;
                Ok(())
            })
        })
        .await
        .expect("a body over borrowed locals");

    assert_eq!(count(&store).await, 2);
}
