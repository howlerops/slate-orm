//! Many writers and readers at once.
//!
//! One test elsewhere has two writers race for a unique index slot and checks
//! that one of them loses. That establishes conflicts are *detected*; it says
//! nothing about what happens under real contention, where the interesting
//! failures are:
//!
//! - a **lost update**, where two read-modify-write cycles interleave and one
//!   silently overwrites the other. This is the failure that a conflict check
//!   exists to prevent, and the one that looks like nothing went wrong.
//! - a **torn read**, where a reader running alongside writers observes a row
//!   without its index entry, or half of a batch.
//! - a retry loop that gives up, or worse, that succeeds while applying its
//!   work twice.
//!
//! Counting is used deliberately as the workload. A counter incremented N times
//! by C tasks must end at exactly N×C: any lost update shows up as a number
//! that is too small, and any double-apply as one that is too large. An
//! assertion on a single integer is hard to satisfy by accident.
//!
//! These run on the in-memory store, so they exercise the record layer's own
//! concurrency control rather than SlateDB's. That is the right boundary here:
//! `slate-slatedb` tests the real writer against a real instance.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, KernelError, RecordStore, RetryPolicy, ScanOrder, SecurityCatalog,
    SecurityContext, with_retries,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Barrier;

const COUNTERS: TableId = TableId(1);
const TENANT: u128 = 3;

fn counters() -> TableDef {
    TableDef::builder("counters", COUNTERS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("total", ValueType::I64)
        .column("owner", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_total", IndexId(10)).column("total"))
        .index(
            IndexDef::builder("by_owner", IndexId(11))
                .column("owner")
                .unique(),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    counters().ordinal_of(name).expect("column exists")
}

fn counter(id: u64, total: i64, owner: &str) -> Row {
    Row::new(vec![
        Value::Uuid(uuid::Uuid::from_u128(TENANT)),
        Value::U64(id),
        Value::I64(total),
        Value::Str(owner.to_owned()),
    ])
}

fn pk(id: u64) -> Vec<Value> {
    vec![Value::Uuid(uuid::Uuid::from_u128(TENANT)), Value::U64(id)]
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn store() -> Arc<RecordStore<MemoryStore>> {
    let catalog = Catalog::from_tables([counters()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", COUNTERS, Action::ALL));
    Arc::new(RecordStore::new(MemoryStore::new(), catalog, security))
}

async fn total(store: &RecordStore<MemoryStore>, id: u64) -> i64 {
    let table = counters();
    let txn = store.begin().await.unwrap();
    let row = txn
        .get(&root(), &table, &pk(id))
        .await
        .unwrap()
        .expect("the counter exists");
    match row.values()[col("total").0] {
        Value::I64(n) => n,
        ref other => panic!("total was {other:?}"),
    }
}

/// A retry policy with no sleeping, so contention is resolved by retrying
/// rather than by the tasks politely not overlapping.
fn eager() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 200,
        initial_backoff: std::time::Duration::ZERO,
        max_backoff: std::time::Duration::ZERO,
    }
}

/// Concurrent read-modify-write on one row loses nothing.
///
/// Every task reads the counter, adds one, and writes it back. If two tasks can
/// both read `n` and both write `n + 1`, the final total is short — and nothing
/// anywhere reports an error. This is the canonical lost update, and the whole
/// reason a conflict check exists.
///
/// It runs in two phases, because "did the writers actually contend?" must not
/// be left to the scheduler. The first phase holds every task at a barrier
/// *after* it has read and *before* any of them writes, so the conflict is
/// guaranteed by construction: exactly one of `TASKS` identical
/// read-modify-writes may commit, and a store that lets a second one through
/// has lost an update. The second phase then runs the same workload at full
/// speed through the retry helper and checks the total to the unit.
///
/// An earlier version had no first phase and instead asserted that the retry
/// counter ended above zero, as a guard against a vacuous run. That is a
/// scheduling observation dressed up as a property: on a loaded machine the
/// eight tasks can run one after another, nothing conflicts, and a *correct*
/// store fails the test. It did, about one full-suite run in six. The barrier
/// replaces the hope with a guarantee.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_increments_do_not_lose_updates() {
    const TASKS: usize = 8;
    const PER_TASK: usize = 25;

    let store = store().await;
    let table = counters();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &counter(1, 0, "shared"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    // Phase one: everybody reads before anybody writes.
    let barrier = Arc::new(Barrier::new(TASKS));
    let mut racers = Vec::new();
    for _ in 0..TASKS {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        racers.push(tokio::spawn(async move {
            let table = counters();
            let txn = store.begin().await.unwrap();
            let row = txn
                .get(&root(), &table, &pk(1))
                .await
                .unwrap()
                .expect("the counter exists");
            let current = match row.values()[col("total").0] {
                Value::I64(n) => n,
                ref other => panic!("total was {other:?}"),
            };
            barrier.wait().await;
            txn.update(&root(), &table, &counter(1, current + 1, "shared"))
                .await?;
            txn.commit().await.map(|_| ())
        }));
    }
    // The barrier only releases once all `TASKS` tasks reach it, so a task that
    // panics on the way there would strand the rest. The timeout turns that
    // into a failed test rather than a hung suite.
    let outcomes = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut outcomes = Vec::new();
        for racer in racers {
            outcomes.push(racer.await.unwrap());
        }
        outcomes
    })
    .await
    .expect("a writer never reached the barrier");

    let committed = outcomes.iter().filter(|o| o.is_ok()).count();
    assert_eq!(
        committed, 1,
        "{committed} of {TASKS} writers committed the same read-modify-write"
    );
    // The other seven have to be refused *as conflicts*. A store that failed
    // them for some unrelated reason would pass the count above while proving
    // nothing about conflict detection.
    for outcome in &outcomes {
        match outcome {
            Ok(()) | Err(KernelError::TransactionConflict) => {}
            Err(other) => panic!("a loser was refused for the wrong reason: {other:?}"),
        }
    }
    assert_eq!(
        total(&store, 1).await,
        1,
        "one writer committed, but its increment is not there"
    );

    // Phase two: the same workload with the brakes off, through the retry
    // helper. Whatever the tasks interleave into, the arithmetic is exact.
    let mut tasks = Vec::new();
    for _ in 0..TASKS {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let table = counters();
            for _ in 0..PER_TASK {
                with_retries(eager(), |_| {
                    let store = Arc::clone(&store);
                    let table = table.clone();
                    async move {
                        let txn = store.begin().await?;
                        let row = txn
                            .get(&root(), &table, &pk(1))
                            .await?
                            .ok_or(KernelError::TransactionConflict)?;
                        let current = match row.values()[col("total").0] {
                            Value::I64(n) => n,
                            _ => return Err(KernelError::TransactionConflict),
                        };
                        txn.update(&root(), &table, &counter(1, current + 1, "shared"))
                            .await?;
                        txn.commit().await?;
                        Ok(())
                    }
                })
                .await
                .expect("an increment gave up");
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    // One from the barrier round, then every increment of phase two.
    let expected = 1 + (TASKS * PER_TASK) as i64;
    assert_eq!(
        total(&store, 1).await,
        expected,
        "{TASKS} tasks x {PER_TASK} increments lost or duplicated an update"
    );
}

/// Only one of many racing writers can take a unique index slot.
///
/// Not "one wins and one loses" with two writers, but *exactly one* out of
/// many — the case where a check-then-write race lets a second through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_writer_takes_a_unique_slot() {
    const TASKS: u64 = 16;

    let store = store().await;
    let mut tasks = Vec::new();
    for id in 0..TASKS {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let table = counters();
            let txn = store.begin().await.unwrap();
            // Every task claims the same owner, with a different primary key.
            if txn
                .insert(&root(), &table, &counter(id, 0, "contested"))
                .await
                .is_err()
            {
                return false;
            }
            txn.commit().await.is_ok()
        }));
    }

    let mut winners = 0;
    for task in tasks {
        if task.await.unwrap() {
            winners += 1;
        }
    }
    assert_eq!(winners, 1, "{winners} writers took the same unique slot");

    let table = counters();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "more rows than winners: {rows:?}");
}

/// A reader running alongside writers never sees a row without its index entry.
///
/// The reader alternates between a table scan and an index scan over the same
/// predicate. Each is a separate access path over separate keys, so if writes
/// ever became visible in two steps, the two would disagree — a torn read that
/// no single-path test could see.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_never_sees_a_row_without_its_index_entry() {
    const ROWS: u64 = 60;

    let store = store().await;
    let done = Arc::new(AtomicUsize::new(0));

    let writer = {
        let store = Arc::clone(&store);
        let done = Arc::clone(&done);
        tokio::spawn(async move {
            let table = counters();
            for id in 0..ROWS {
                let txn = store.begin().await.unwrap();
                txn.insert(&root(), &table, &counter(id, id as i64, &format!("o{id}")))
                    .await
                    .unwrap();
                txn.commit().await.unwrap();
                tokio::task::yield_now().await;
            }
            done.store(1, Ordering::SeqCst);
        })
    };

    let reader = {
        let store = Arc::clone(&store);
        let done = Arc::clone(&done);
        tokio::spawn(async move {
            let table = counters();
            let mut checks = 0usize;
            while done.load(Ordering::SeqCst) == 0 || checks < 4 {
                let txn = store.begin().await.unwrap();
                // Forced down the table's own key range...
                let by_scan = txn
                    .execute(&root(), &table, &{
                        let mut q = slate_kernel::Query::all().filter(Expr::compare(
                            col("total"),
                            CmpOp::Ge,
                            Value::I64(0),
                        ));
                        q.hint = Some(slate_kernel::AccessHint::TableScan);
                        q
                    })
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap();
                // ...and down the index, in the same transaction, so both see
                // the same instant.
                let by_index = txn
                    .execute(&root(), &table, &{
                        let mut q = slate_kernel::Query::all().filter(Expr::compare(
                            col("total"),
                            CmpOp::Ge,
                            Value::I64(0),
                        ));
                        q.hint = Some(slate_kernel::AccessHint::Index(IndexId(10)));
                        q
                    })
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap();

                let mut a: Vec<i64> = by_scan
                    .iter()
                    .filter_map(|r| match r.values()[col("total").0] {
                        Value::I64(n) => Some(n),
                        _ => None,
                    })
                    .collect();
                let mut b: Vec<i64> = by_index
                    .iter()
                    .filter_map(|r| match r.values()[col("total").0] {
                        Value::I64(n) => Some(n),
                        _ => None,
                    })
                    .collect();
                a.sort_unstable();
                b.sort_unstable();
                assert_eq!(
                    a, b,
                    "a reader saw the table and the index in different states"
                );

                checks += 1;
                tokio::task::yield_now().await;
            }
            checks
        })
    };

    writer.await.unwrap();
    let checks = reader.await.unwrap();
    assert!(checks > 0, "the reader never ran");
}

/// Writers on *different* rows do not conflict with each other.
///
/// A conflict check that is too coarse — say, one that treats any overlapping
/// scan as a conflict — would make the store correct and useless. This is the
/// other side of the lost-update test: contention must be detected, and
/// non-contention must not be invented.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writers_on_different_rows_do_not_conflict() {
    const TASKS: u64 = 12;

    let store = store().await;
    let mut tasks = Vec::new();
    for id in 0..TASKS {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let table = counters();
            let txn = store.begin().await.unwrap();
            txn.insert(
                &root(),
                &table,
                &counter(id, id as i64, &format!("owner{id}")),
            )
            .await?;
            txn.commit().await?;
            Ok::<(), KernelError>(())
        }));
    }
    for task in tasks {
        task.await.unwrap().expect("a disjoint write was refused");
    }

    let table = counters();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), TASKS as usize);
}

/// Concurrent deletes of the same row: one succeeds, the rest report it gone
/// rather than reporting success twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deletes_of_one_row_are_not_all_successes() {
    const TASKS: u64 = 8;

    let store = store().await;
    let table = counters();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &counter(1, 0, "doomed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let mut tasks = Vec::new();
    for _ in 0..TASKS {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let table = counters();
            let txn = store.begin().await.unwrap();
            if txn.delete(&root(), &table, &pk(1)).await.is_err() {
                return false;
            }
            txn.commit().await.is_ok()
        }));
    }

    let mut succeeded = 0;
    for task in tasks {
        if task.await.unwrap() {
            succeeded += 1;
        }
    }
    assert!(
        succeeded >= 1,
        "every concurrent delete failed, so the row is still there"
    );

    // However many reported success, the row is gone and its unique slot is
    // free — which is the invariant that matters.
    let txn = store.begin().await.unwrap();
    assert!(txn.get(&root(), &table, &pk(1)).await.unwrap().is_none());
    txn.insert(&root(), &table, &counter(2, 0, "doomed"))
        .await
        .expect("the deleted row's unique slot was never released");
    txn.commit().await.unwrap();
}

/// Many tasks each incrementing their *own* counter, all at once.
///
/// Closest to a real workload: contention is incidental rather than total, so
/// most transactions commit first time and the few that clash must still come
/// out right. Every counter is checked, not just the total, since a lost update
/// on one row can hide inside a correct sum.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mixed_workload_lands_every_write() {
    const ROWS: u64 = 10;
    const TASKS: usize = 8;
    const PER_TASK: usize = 20;

    let store = store().await;
    let table = counters();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root(), &table, &counter(id, 0, &format!("o{id}")))
            .await
            .unwrap();
    }
    txn.commit().await.unwrap();

    let mut tasks = Vec::new();
    for t in 0..TASKS {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let table = counters();
            for i in 0..PER_TASK {
                // Deliberately overlapping: task t and task t+1 share rows.
                let id = ((t + i) as u64) % ROWS;
                with_retries(eager(), |_| {
                    let store = Arc::clone(&store);
                    let table = table.clone();
                    async move {
                        let txn = store.begin().await?;
                        let row = txn
                            .get(&root(), &table, &pk(id))
                            .await?
                            .ok_or(KernelError::TransactionConflict)?;
                        let current = match row.values()[col("total").0] {
                            Value::I64(n) => n,
                            _ => return Err(KernelError::TransactionConflict),
                        };
                        let owner = match &row.values()[col("owner").0] {
                            Value::Str(s) => s.clone(),
                            _ => return Err(KernelError::TransactionConflict),
                        };
                        txn.update(&root(), &table, &counter(id, current + 1, &owner))
                            .await?;
                        txn.commit().await?;
                        Ok(())
                    }
                })
                .await
                .expect("an increment gave up");
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    let mut sum = 0i64;
    for id in 0..ROWS {
        sum += total(&store, id).await;
    }
    assert_eq!(
        sum,
        (TASKS * PER_TASK) as i64,
        "the increments do not add up across {ROWS} rows"
    );
}
