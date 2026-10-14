//! What one full scan costs in instructions, on a machine that is not quiet.
//!
//! ```sh
//! cargo build --release -p slate-headbench --example scan_count
//! for n in 0 200; do SCANS=$n /usr/bin/time -l target/release/examples/scan_count; done
//! ```
//!
//! The difference in "instructions retired" between the two runs, divided by
//! 200, is the cost of one scan of `cache_probe`'s 20,000-row fixture. Seeding,
//! start-up and the warm-up scan are in both runs and cancel.
//!
//! # Why instructions and not the clock
//!
//! `cache_probe` measured a full scan 7% slower on SlateDB 0.17 than 0.16, on a
//! Mac at load average 6.5–16. Re-run the next day at load 15–19 it gave
//! spreads of ±300%, because a process that lands on an efficiency core runs
//! at a fraction of the speed of one on a performance core. Instructions
//! retired count work, not time, so they do not care what else is running:
//! five runs of one build agree to within a fraction of a percent, in one of
//! two or three modes about 1.5M apart (`docs/performance.md` §8g).
//!
//! `/usr/bin/time -l` reports the counter on Apple silicon. Elsewhere,
//! `perf stat -e instructions` is the same measurement.
//!
//! # What it cannot tell you
//!
//! Instructions are not time: a cache miss costs one instruction and a
//! hundred cycles. And the same source built two ways — from the registry or
//! from a path — differed by up to 1.5% here, so a difference that small
//! between two *builds* is not a difference between two *versions* until it
//! survives being built the same way.
//!
//! With no `SCANS` it seeds, scans once and exits, which is what
//! `run_examples.sh --smoke` runs.

// An example's failures are its output, as in `cache_probe`.
#![allow(clippy::expect_used)]

use slate_headbench::fixture::{catalog, context, events, row, security};
use slate_kernel::{Query, RecordStore};
use slate_schema::Row;
use slate_slatedb::SlateStore;
use slatedb::Db;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;

/// `cache_probe`'s fixture size, so the two examples measure one table.
const ROWS: u64 = 20_000;
const TENANT: u64 = 1;
const DB_PATH: &str = "/scan-count";

// One thread, so the count is one scan's work and not also a scheduler
// deciding where to run it.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    // The count means nothing without the build behind it: the same source
    // built two ways differed by 1.5% (`docs/performance.md` §8g).
    slate_slatedb::announce();
    let scans: usize = std::env::var("SCANS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    seed(&store).await;

    // Reopened, so the scans read SSTs from the object store as `cache_probe`'s
    // do, rather than a memtable.
    let db = Db::builder(DB_PATH, Arc::clone(&store))
        .build()
        .await
        .expect("reopen SlateDB");
    let records = RecordStore::new(
        Arc::new(SlateStore::from_db(Arc::new(db))),
        catalog(),
        security(),
    );
    let (table, ctx) = (events(), context(TENANT));
    // One more than asked: the first warms the cache in every run, so the
    // difference between two runs is warm scans only.
    for _ in 0..=scans {
        let snapshot = records.snapshot().await.expect("snapshot");
        let rows = snapshot
            .execute(&ctx, &table, &Query::all())
            .await
            .expect("scan")
            .count()
            .await
            .expect("count");
        assert_eq!(rows as u64, ROWS, "a scan lost rows");
    }
}

async fn seed(store: &Arc<dyn ObjectStore>) {
    let db = Db::builder(DB_PATH, Arc::clone(store))
        .build()
        .await
        .expect("open SlateDB");
    let backend = Arc::new(
        SlateStore::from_db(Arc::new(db)).with_durability(slate_slatedb::Durability::Visible),
    );
    let records = RecordStore::new(Arc::clone(&backend), catalog(), security());
    let (table, ctx) = (events(), context(TENANT));
    for start in (0..ROWS).step_by(1_000) {
        let rows: Vec<Row> = (start..(start + 1_000).min(ROWS))
            .map(|id| row(TENANT, id))
            .collect();
        let txn = records.begin().await.expect("begin");
        txn.insert_many(&ctx, &table, &rows).await.expect("insert");
        txn.commit().await.expect("commit");
    }
    backend.close().await.expect("close");
}
