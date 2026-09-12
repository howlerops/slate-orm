//! Writer handover: what happens around the moment ownership changes.
//!
//! `replica.rs` establishes that a second writer fences the first, in the
//! simplest arrangement: commit, take over, commit again, observe the error.
//! That is the happy path of an unhappy event. The cases that decide whether a
//! head node can be replaced safely are the ones either side of it —
//!
//! - a transaction the old writer **opened before** the takeover and commits
//!   after it. The writer had no way to know; the store has to tell it.
//! - whether being fenced is **terminal**. A writer that is fenced once and
//!   succeeds later is a split brain, and a lease that can be reacquired by
//!   accident is worse than no lease.
//! - whether the new writer **sees everything** the old one committed, and can
//!   write on top of it without tripping over the old writer's index entries.
//! - a **chain** of handovers, since a lease usually changes hands more than
//!   once over a service's life and each takeover must fence only the writer
//!   it replaced.
//!
//! There is no lease manager here — SlateDB fences but does not elect — so
//! "takeover" means opening a second writer over the same path, which is what
//! a new head node doing the same thing amounts to.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use slate_kernel::{Expr, KernelError, RecordStore, ScanOrder, SecurityContext};
use slate_slatedb::SlateStore;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;

const PATH: &str = "/records";

async fn writer(store: Arc<dyn ObjectStore>) -> RecordStore<SlateStore> {
    common::record_store(SlateStore::open(PATH, store).await.expect("open writer"))
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// Write one row, reporting what happened rather than unwrapping.
async fn write(store: &RecordStore<SlateStore>, id: u64) -> Result<(), KernelError> {
    let table = common::users();
    let txn = store.begin().await?;
    txn.insert(
        &root(),
        &table,
        &common::user(common::TENANT_A, id, &format!("u{id}@x.com"), None, 30),
    )
    .await?;
    txn.commit().await?;
    Ok(())
}

async fn ids(store: &RecordStore<SlateStore>) -> Vec<u64> {
    let table = common::users();
    let txn = store.begin().await.unwrap();
    let mut out: Vec<u64> = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .iter()
        .map(|r| match r.values()[1] {
            slate_tuple::Value::U64(id) => id,
            ref other => panic!("id was {other:?}"),
        })
        .collect();
    out.sort_unstable();
    out
}

/// A transaction opened *before* the takeover is still fenced when it commits.
///
/// This is the case a writer cannot defend against on its own: the transaction
/// was legitimate when it started. If the store let it through, the two writers
/// would have interleaved and the lease would mean nothing.
#[tokio::test]
async fn a_transaction_opened_before_the_takeover_is_fenced() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let table = common::users();

    let first = writer(Arc::clone(&object_store)).await;
    write(&first, 1).await.expect("the first writer owns it");

    // Opened while the first writer is still the owner.
    let in_flight = first.begin().await.unwrap();
    in_flight
        .insert(
            &root(),
            &table,
            &common::user(common::TENANT_A, 2, "u2@x.com", None, 31),
        )
        .await
        .expect("buffering a write does not touch the object store");

    // Ownership changes underneath it.
    let second = writer(Arc::clone(&object_store)).await;

    match in_flight.commit().await {
        Err(KernelError::WriterFenced) => {}
        other => panic!("an in-flight transaction survived a takeover: {other:?}"),
    }

    // And nothing it buffered is visible to the new owner.
    assert_eq!(ids(&second).await, vec![1], "a fenced write became visible");
    second.backend().close().await.unwrap();
}

/// Being fenced is terminal, not a bad moment.
///
/// A writer that retries and eventually succeeds is a split brain. `WriterFenced`
/// is deliberately not retryable, and this checks the store agrees with the
/// error type: several attempts, all refused.
#[tokio::test]
async fn a_fenced_writer_stays_fenced() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());

    let first = writer(Arc::clone(&object_store)).await;
    write(&first, 1).await.unwrap();
    let second = writer(Arc::clone(&object_store)).await;

    for attempt in 0..5u64 {
        match write(&first, 100 + attempt).await {
            Err(KernelError::WriterFenced) => {}
            other => panic!("attempt {attempt} after fencing gave {other:?}"),
        }
    }

    assert_eq!(
        ids(&second).await,
        vec![1],
        "a fenced writer got a write through on a later attempt"
    );
    second.backend().close().await.unwrap();
}

/// The new writer inherits everything, including the index state.
///
/// Taking over an empty-looking database, or one whose unique index the new
/// writer cannot see, would be worse than being fenced: it would accept a
/// duplicate.
#[tokio::test]
async fn the_new_writer_inherits_the_old_writers_state() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let table = common::users();

    let first = writer(Arc::clone(&object_store)).await;
    for id in 0..5 {
        write(&first, id).await.unwrap();
    }

    let second = writer(Arc::clone(&object_store)).await;
    assert_eq!(
        ids(&second).await,
        vec![0, 1, 2, 3, 4],
        "the new writer did not see the old writer's rows"
    );

    // The unique index came across too: the old writer's emails are taken.
    let txn = second.begin().await.unwrap();
    let clash = txn
        .insert(
            &root(),
            &table,
            &common::user(common::TENANT_A, 99, "u3@x.com", None, 40),
        )
        .await;
    assert!(
        clash.is_err(),
        "the new writer did not inherit the unique index"
    );
    drop(txn);

    // And it can write on top.
    write(&second, 5).await.expect("the new writer can write");
    assert_eq!(ids(&second).await, vec![0, 1, 2, 3, 4, 5]);
    second.backend().close().await.unwrap();
}

/// Handover happens more than once, and each takeover fences only its
/// predecessor.
///
/// A fencing token that is not monotonic would let an *older* writer come back
/// — the second writer fencing the third rather than the other way round.
#[tokio::test]
async fn a_chain_of_handovers_fences_only_the_predecessor() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());

    let mut writers = Vec::new();
    for generation in 0..4u64 {
        let next = writer(Arc::clone(&object_store)).await;
        write(&next, generation)
            .await
            .unwrap_or_else(|e| panic!("generation {generation} could not write: {e}"));

        // Every earlier generation is fenced, including ones fenced already.
        for (earlier, old) in writers.iter().enumerate() {
            match write(old, 900 + earlier as u64).await {
                Err(KernelError::WriterFenced) => {}
                other => panic!(
                    "generation {earlier} wrote after generation {generation} took over: {other:?}"
                ),
            }
        }
        writers.push(next);
    }

    let current = writers.last().expect("four generations");
    assert_eq!(
        ids(current).await,
        vec![0, 1, 2, 3],
        "each generation's own write should have landed, and nothing else"
    );
    for store in &writers {
        let _ = store.backend().close().await;
    }
}

/// A fenced writer cannot read either: `begin` itself fails.
///
/// This was written expecting the opposite, on the reasoning that a stale read
/// is still a read and a head node stepping down would want to drain its
/// in-flight queries. It does not work that way — once fenced, the store is
/// unusable, and `begin` returns `WriterFenced` before a transaction exists.
///
/// That is defensible: a fenced writer's view is arbitrarily far behind, and
/// there is no way for it to say how far. Failing fast is more honest than
/// serving a snapshot whose age nobody can bound.
///
/// It is pinned here because it constrains how a head node is replaced. A
/// takeover is not a graceful drain: reads in flight on the old writer fail
/// too, so anything that must keep serving during a handover has to be reading
/// from a replica, not from the writer. That is what the topology already
/// recommends, and this makes it a requirement rather than a preference.
#[tokio::test]
async fn a_fenced_writer_cannot_read_either() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let table = common::users();

    let first = writer(Arc::clone(&object_store)).await;
    write(&first, 1).await.unwrap();
    // A read works while it still owns the database.
    assert_eq!(ids(&first).await, vec![1], "premise");

    let second = writer(Arc::clone(&object_store)).await;
    write(&second, 2).await.unwrap();

    // Fenced for writes...
    assert!(matches!(
        write(&first, 3).await,
        Err(KernelError::WriterFenced)
    ));
    // ...and for reads, at `begin`, before a transaction is even opened.
    match first.begin().await {
        Err(KernelError::WriterFenced) => {}
        Ok(_) => panic!("a fenced writer opened a read transaction"),
        Err(other) => panic!("expected WriterFenced from begin, got {other:?}"),
    }

    // The new owner is unaffected and sees both rows.
    let txn = second.begin().await.unwrap();
    let count = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .len();
    assert_eq!(count, 2);
    drop(txn);
    second.backend().close().await.unwrap();
}

/// After a handover the store is still consistent: every row the new writer
/// sees is reachable through the index, and vice versa.
///
/// A takeover in the middle of a write is the realistic way to end up with a
/// half-applied change, and an index that disagrees with its table is the
/// symptom that does not announce itself.
#[tokio::test]
async fn the_index_and_the_table_agree_after_a_handover() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let table = common::users();

    let first = writer(Arc::clone(&object_store)).await;
    for id in 0..20 {
        write(&first, id).await.unwrap();
    }
    // An in-flight write that will be fenced.
    let doomed = first.begin().await.unwrap();
    doomed
        .insert(
            &root(),
            &table,
            &common::user(common::TENANT_A, 999, "doomed@x.com", None, 30),
        )
        .await
        .unwrap();

    let second = writer(Arc::clone(&object_store)).await;
    assert!(matches!(
        doomed.commit().await,
        Err(KernelError::WriterFenced)
    ));

    // A scan and an index scan must see the same rows.
    let age = common::users().ordinal_of("age").expect("age");
    let txn = second.begin().await.unwrap();
    let by_scan = ids(&second).await;
    let by_index: Vec<u64> = txn
        .query(
            &root(),
            &table,
            Expr::compare(age, slate_kernel::CmpOp::Ge, slate_tuple::Value::I64(18)),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .iter()
        .map(|r| match r.values()[1] {
            slate_tuple::Value::U64(id) => id,
            ref other => panic!("id was {other:?}"),
        })
        .collect();
    let mut by_index = by_index;
    by_index.sort_unstable();

    assert_eq!(
        by_scan, by_index,
        "the index and the table disagreed after a handover"
    );
    assert!(
        !by_scan.contains(&999),
        "the fenced writer's row is visible: {by_scan:?}"
    );

    // The doomed row's unique slot was never taken.
    drop(txn);
    let txn = second.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &common::user(common::TENANT_A, 50, "doomed@x.com", None, 30),
    )
    .await
    .expect("a fenced write held onto its unique index slot");
    txn.commit().await.unwrap();
    second.backend().close().await.unwrap();
}
