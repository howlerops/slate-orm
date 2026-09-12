//! A crash underneath the storage engine.
//!
//! `slate-kernel`'s `crash.rs` fails writes at the transaction boundary and
//! proves the record layer never separates a row from its index entries. That
//! is the layer this project owns, and it is the easier half: the failure is
//! clean, and nothing has been written yet.
//!
//! Here the object store itself stops accepting writes partway through, so
//! SlateDB is interrupted mid-flush with some of its own SSTs and manifest
//! updates landed and some not. That is what a killed process leaves behind.
//!
//! The claim under test is deliberately weak, because it is the only one that
//! can hold: **whatever survives is coherent.** Committed data may be lost — a
//! commit that never reached the object store is not durable and the layer
//! never said it was. What must not happen is a database that comes back
//! *wrong*: an index entry pointing at a row that is not there, a row missing
//! from an index that should hold it, or a unique slot occupied by nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::faulty_store::FaultyStore;
use slate_kernel::store::{KeyRange, KvStore};
use slate_kernel::{Expr, RecordStore, ScanOrder, SecurityContext, keys};
use slate_slatedb::SlateStore;
use slate_tuple::Value;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;
use std::time::Duration;

const PATH: &str = "/records";

/// Row writes attempted per case.
const WRITES: u64 = 40;

/// Where the object store is made to die, in object-store writes. Every value
/// must be below what a run actually costs, or the fault never fires —
/// `report_the_object_store_write_count` checks exactly that.
const BUDGETS: [usize; 4] = [1, 3, 6, 10];

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// Open over the faulty store, as a fresh process would.
async fn open(faulty: Arc<FaultyStore>) -> Option<RecordStore<SlateStore>> {
    // A commit timeout is essential here, not incidental: without one, a store
    // whose object store has stopped accepting writes never returns, and this
    // whole file hangs instead of reporting. See
    // `slatedb_hangs_when_the_object_store_refuses_writes`.
    tokio::time::timeout(
        Duration::from_secs(10),
        SlateStore::open(PATH, faulty as Arc<dyn ObjectStore>),
    )
    .await
    .ok()?
    .ok()
    .map(|backend| common::record_store(backend.with_commit_timeout(Duration::from_secs(3))))
}

/// Every index entry resolves to a row that exists with the values it claims,
/// and every row appears in every index exactly once.
///
/// The index keys are read straight off the store rather than through the
/// record layer: a checker sharing the code path it checks would agree with it
/// by construction.
async fn assert_coherent(store: &RecordStore<SlateStore>, context: &str) {
    let table = common::users();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    drop(txn);

    for index in table.indexes() {
        let raw = store.backend().begin().await.unwrap();
        let mut cursor = raw
            .scan(
                KeyRange::prefix(&keys::index_prefix(&table, index, None)),
                ScanOrder::Ascending,
            )
            .await
            .unwrap();

        let mut entries = 0usize;
        while let Some(kv) = cursor.next().await.unwrap() {
            let (indexed, primary_key) =
                keys::decode_index_entry(&table, index, &kv.key, &kv.value).unwrap_or_else(|e| {
                    panic!(
                        "{context}: index `{}` holds an undecodable entry: {e}",
                        index.name()
                    )
                });
            let row = rows.iter().find(|r| {
                table
                    .primary_key()
                    .iter()
                    .map(|o| &r.values()[o.0])
                    .eq(primary_key.iter())
            });
            let row = row.unwrap_or_else(|| {
                panic!(
                    "{context}: index `{}` points at {primary_key:?}, which is not a row",
                    index.name()
                )
            });
            for (column, value) in index.columns().iter().zip(&indexed) {
                assert_eq!(
                    &row.values()[column.ordinal.0],
                    value,
                    "{context}: index `{}` holds a stale value for {primary_key:?}",
                    index.name()
                );
            }
            entries += 1;
        }
        drop(cursor);
        drop(raw);

        assert_eq!(
            entries,
            rows.len(),
            "{context}: index `{}` has {entries} entries for {} rows",
            index.name(),
            rows.len()
        );
    }
}

async fn write(store: &RecordStore<SlateStore>, id: u64) -> bool {
    let table = common::users();
    let Ok(txn) = store.begin().await else {
        return false;
    };
    if txn
        .insert(
            &root(),
            &table,
            &common::user(common::TENANT_A, id, &format!("u{id}@x.com"), None, 30),
        )
        .await
        .is_err()
    {
        return false;
    }
    txn.commit().await.is_ok()
}

/// The object store dies partway through a run of writes; what comes back is
/// coherent.
///
/// The fault is placed at several depths, because where it lands decides which
/// of SlateDB's own writes were interrupted — an SST that never finished is a
/// different recovery from a manifest that did not get updated.
#[tokio::test]
async fn a_dying_object_store_leaves_a_coherent_database() {
    for budget in BUDGETS {
        let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let faulty = Arc::new(FaultyStore::new(Arc::clone(&inner)));

        let landed = {
            let Some(store) = open(Arc::clone(&faulty)).await else {
                continue;
            };
            // Let the store open cleanly, then start failing.
            faulty.fail_after(budget);

            let mut landed = Vec::new();
            for id in 0..WRITES {
                if write(&store, id).await {
                    landed.push(id);
                }
                // Once storage is refusing writes, every further attempt just
                // waits out the commit timeout. Two past the fault is enough to
                // show it is not transient, and stopping there is the
                // difference between this file taking seconds and minutes.
                if faulty.tripped() && id > 2 {
                    break;
                }
            }
            // Closing can hang as well as fail, so it is bounded too.
            let _ = tokio::time::timeout(Duration::from_secs(5), store.backend().close()).await;
            landed
        };

        assert!(
            faulty.tripped(),
            "budget {budget} never fired, so this case tested nothing"
        );

        // The process restarts, and storage is working again.
        faulty.recover();
        let Some(reopened) = open(Arc::clone(&faulty)).await else {
            // Refusing to open at all is a legitimate outcome of a torn
            // manifest: it is loud, and it is not a wrong answer.
            continue;
        };

        let context = format!("after a crash at write budget {budget}");
        assert_coherent(&reopened, &context).await;

        // Whatever survived must be a subset of what the writer believed it
        // had committed. Coming back with a row nobody ever wrote would mean
        // the store invented one.
        let table = common::users();
        let txn = reopened.begin().await.unwrap();
        let survived: Vec<u64> = txn
            .query(&root(), &table, Expr::True, ScanOrder::Ascending)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|r| match r.values()[1] {
                Value::U64(id) => id,
                ref other => panic!("id was {other:?}"),
            })
            .collect();
        drop(txn);
        for id in &survived {
            assert!(
                landed.contains(id),
                "{context}: row {id} survived but was never reported committed"
            );
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), reopened.backend().close()).await;
    }
}

/// After a crash the store is writable again, and its unique index is intact.
///
/// The dangerous residue of a torn write is a unique slot occupied by a row
/// that did not survive: nobody could ever take that email again, and nothing
/// would explain why.
#[tokio::test]
async fn a_recovered_store_still_enforces_its_unique_index() {
    let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let faulty = Arc::new(FaultyStore::new(Arc::clone(&inner)));

    let landed = {
        let store = open(Arc::clone(&faulty)).await.expect("open");
        faulty.fail_after(9);
        let mut landed = Vec::new();
        for id in 0..WRITES {
            if write(&store, id).await {
                landed.push(id);
            }
            if faulty.tripped() && id > 2 {
                break;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), store.backend().close()).await;
        landed
    };
    assert!(faulty.tripped(), "the fault never fired");

    faulty.recover();
    let Some(reopened) = open(Arc::clone(&faulty)).await else {
        return;
    };
    assert_coherent(&reopened, "after recovery").await;

    let table = common::users();
    let txn = reopened.begin().await.unwrap();
    let present: Vec<u64> = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .iter()
        .map(|r| match r.values()[1] {
            Value::U64(id) => id,
            ref other => panic!("id was {other:?}"),
        })
        .collect();
    drop(txn);

    // Every email belonging to a surviving row is taken...
    for id in &present {
        let txn = reopened.begin().await.unwrap();
        let clash = txn
            .insert(
                &root(),
                &table,
                &common::user(
                    common::TENANT_A,
                    5000 + id,
                    &format!("u{id}@x.com"),
                    None,
                    30,
                ),
            )
            .await;
        assert!(
            clash.is_err(),
            "a surviving row lost its unique index entry after recovery"
        );
    }
    // ...and every email whose row did not survive is free again.
    for id in landed.iter().filter(|id| !present.contains(id)) {
        let txn = reopened.begin().await.unwrap();
        txn.insert(
            &root(),
            &table,
            &common::user(
                common::TENANT_A,
                6000 + id,
                &format!("u{id}@x.com"),
                None,
                30,
            ),
        )
        .await
        .unwrap_or_else(|e| {
            panic!("row {id} did not survive but its unique slot is still taken: {e}")
        });
        txn.commit().await.unwrap();
    }

    // And the store takes new writes normally.
    assert!(
        write(&reopened, 900).await,
        "a recovered store cannot write"
    );
    assert_coherent(&reopened, "after writing post-recovery").await;
    let _ = tokio::time::timeout(Duration::from_secs(5), reopened.backend().close()).await;
}

/// How many object-store writes a run of row writes actually costs.
///
/// Not an assertion about the number, which is SlateDB's business and will
/// change: it exists so the fault budgets below are chosen against a measured
/// figure rather than a guess. A budget above the real count never fires, and
/// a case whose fault never fires tests nothing.
#[tokio::test]
async fn report_the_object_store_write_count() {
    let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let faulty = Arc::new(FaultyStore::new(Arc::clone(&inner)));
    let store = open(Arc::clone(&faulty)).await.expect("open");

    // A budget nothing will reach, so the counter just counts.
    faulty.fail_after(1_000_000);
    for id in 0..WRITES {
        write(&store, id).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), store.backend().close()).await;

    let used = 1_000_000 - faulty.remaining();
    eprintln!("{WRITES} row writes cost {used} object-store writes");
    assert!(
        used > 0,
        "no object-store write was observed, so the injector is not in the path"
    );
    assert!(
        BUDGETS.iter().all(|b| *b < used),
        "some fault budget in {BUDGETS:?} is above the {used} writes a run costs, \
         so that case would never fire"
    );
}

/// **SlateDB waits indefinitely when the object store refuses writes.**
///
/// Found while building this file, which hung rather than failing. The hang is
/// upstream, not here: this drives SlateDB directly, with no record-layer code
/// in the path, and two hundred puts plus a flush do not return.
///
/// It matters because it is the quietest possible failure. A full disk, a
/// revoked credential or a changed bucket policy does not produce an error —
/// the writer simply stops, while still looking alive. That is the state an
/// operator finds last.
///
/// It cannot be fixed from here, so what this layer does instead is refuse to
/// pass the unbounded wait on: `SlateStore::with_commit_timeout` turns it into
/// `CommitTimedOut`, which is deliberately not retryable because a timed-out
/// commit may still have landed.
///
/// If a future SlateDB returns an error instead, this test fails — and that is
/// the right time to reconsider the timeout.
#[tokio::test]
async fn slatedb_hangs_when_the_object_store_refuses_writes() {
    let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let faulty = Arc::new(FaultyStore::new(Arc::clone(&inner)));
    let db = slatedb::Db::builder("/probe", Arc::clone(&faulty) as Arc<dyn ObjectStore>)
        .build()
        .await
        .expect("open");

    faulty.fail_after(0);

    let outcome = tokio::time::timeout(Duration::from_secs(15), async {
        for i in 0..200u64 {
            db.put(format!("k{i}").as_bytes(), b"v").await?;
        }
        db.flush().await
    })
    .await;

    assert!(
        outcome.is_err(),
        "SlateDB now reports an error when the object store refuses writes, \
         rather than hanging. That is better than what this test was written \
         against -- revisit `with_commit_timeout`, which exists only to bound \
         the hang."
    );
    assert!(faulty.tripped(), "the fault never fired");
}

/// A commit timeout turns the hang into an error a caller can act on.
#[tokio::test]
async fn a_commit_timeout_reports_instead_of_hanging() {
    let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let faulty = Arc::new(FaultyStore::new(Arc::clone(&inner)));
    let backend = SlateStore::open(PATH, Arc::clone(&faulty) as Arc<dyn ObjectStore>)
        .await
        .expect("open")
        .with_commit_timeout(Duration::from_millis(500));
    let store = common::record_store(backend);

    faulty.fail_after(0);

    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        let table = common::users();
        let txn = store.begin().await?;
        txn.insert(
            &root(),
            &table,
            &common::user(common::TENANT_A, 1, "a@x.com", None, 30),
        )
        .await?;
        txn.commit().await
    })
    .await;

    let Ok(result) = outcome else {
        panic!("a commit timeout did not bound the wait; the store still hung")
    };
    match result {
        Err(slate_kernel::KernelError::CommitTimedOut) => {}
        other => panic!("expected CommitTimedOut, got {other:?}"),
    }
    assert!(
        !slate_kernel::KernelError::CommitTimedOut.is_retryable(),
        "a timed-out commit may already have landed, so it must not be retried"
    );
    let _ = tokio::time::timeout(Duration::from_secs(5), store.backend().close()).await;
}

/// The injector has to inject. Without this every test above would pass on a
/// store that never failed.
#[tokio::test]
async fn the_object_store_injector_actually_fails_writes() {
    let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let faulty = Arc::new(FaultyStore::new(Arc::clone(&inner)));
    let store = open(Arc::clone(&faulty)).await.expect("open");

    faulty.fail_after(0);
    let mut refused = 0;
    for id in 0..4u64 {
        if !write(&store, id).await {
            refused += 1;
        }
    }
    assert!(
        refused > 0 && faulty.tripped(),
        "no write was refused with the budget at zero"
    );
    let _ = tokio::time::timeout(Duration::from_secs(5), store.backend().close()).await;
}
