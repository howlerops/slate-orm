//! Is SlateDB's block cache on? It is not, and this measures what that costs.
//!
//! ```sh
//! cargo run --release -p slate-headbench --example cache_probe
//! ```
//!
//! # The finding this exists to test
//!
//! SlateDB's `DbBuilder` installs a cache by itself:
//! `DbBuilder::new` calls `default_db_cache()`, which returns a `SplitCache`
//! over a block cache and a meta cache. Both halves are built by
//! `default_block_cache()` / `default_meta_cache()`, and **both are `None`
//! unless the `foyer` or `moka` feature is enabled**. `foyer` is in SlateDB's
//! `default` feature set — and every crate in this workspace declares
//! `slatedb = { version = "0.16", default-features = false }`, re-enabling
//! only `aws`.
//!
//! So the cache object exists, every `get_block` / `get_index` / `get_filter`
//! it is asked returns `Ok(None)`, and every insert is dropped on the floor.
//! Nothing fails; the reads just all go to object storage. This is the same
//! shape as the scan-readahead finding — a capability the layer below already
//! has, declined by accident rather than on purpose.
//!
//! # What is measured
//!
//! Three arms over an identical fixture, each with its own object store so a
//! result cannot leak from one arm to the next:
//!
//! | arm | what it is |
//! |---|---|
//! | `as shipped` | `with_db_cache_disabled()` — exactly today's build |
//! | `cache on` | a cache installed, scans still `cache_blocks: false` (our default) |
//! | `cache on, scans cached` | the same, with `ScanTuning::cache_blocks` flipped |
//!
//! The headline is **object-store GETs**, counted by
//! [`slate_headbench::counting::CountingStore`], for the reason the readahead
//! measurement gives: against an in-memory object store the clock says almost
//! nothing about what a read costs in a bucket, and the request count says
//! almost everything. Wall clock is printed beside it and is a *lower bound* —
//! an avoided GET is nearly free here and is a round trip in a deployment.
//!
//! The control is the **first read after reopening**: a cold cache cannot hit,
//! so that read must cost the same in every arm. If it moves, the arms differ
//! in something other than the cache and nothing below can be believed.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::indexing_slicing,
    clippy::type_complexity,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

use slate_headbench::counting::{CountingStore, Counts};
use slate_headbench::fixture::{catalog, context, events, row, security};
use slate_headbench::stats::{Measure, difference, duration};
use slate_kernel::{Query, RecordStore};
use slate_schema::Row;
use slate_slatedb::{ScanTuning, SlateStore};
use slate_tuple::Value;
use slatedb::Db;
use slatedb::db_cache::{CachedEntry, CachedKey, DbCache};
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The tenant every row belongs to.
const TENANT: u64 = 1;

/// Rows in the fixture. Enough that the table spans many blocks and several
/// SSTs, which is the only way a cache has anything to hold.
///
/// The default is what `docs/performance.md` records and is unchanged.
/// `HEADBENCH_ROWS` exists so `run.sh` can ask for a fixture small enough to
/// prove the benchmark still *runs* in seconds rather than minutes — at which
/// size the numbers mean nothing, which is the point of that mode and is why
/// it is not the default. Three of this crate's five benchmarks already took
/// a row knob; the two that did not are the two a smoke run had to wait for.
const ROWS: u64 = 20_000;

fn rows() -> u64 {
    std::env::var("HEADBENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS)
}

/// Point reads per timed run.
const READS: usize = 200;

/// Where the fixture database lives.
const DB_PATH: &str = "/cache-probe";

fn runs() -> usize {
    std::env::var("HEADBENCH_RUNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(7)
}

// --- the cache ------------------------------------------------------------

/// An unbounded in-memory [`DbCache`], with hit and miss counters.
///
/// Not a proposal for what to ship — the fix is to stop switching SlateDB's
/// own default cache off. It is here because it isolates exactly one variable:
/// no dependency changes, no feature flag, no second build. Being unbounded it
/// is the *best case* for a cache, so treat the numbers as the ceiling on what
/// enabling `foyer` (512 MiB of blocks, 128 MiB of metadata by default) buys
/// on a fixture this size — which fits either way.
#[derive(Default)]
struct ProbeCache {
    entries: Mutex<HashMap<CachedKey, CachedEntry>>,
    block_hits: AtomicU64,
    block_misses: AtomicU64,
    meta_hits: AtomicU64,
    meta_misses: AtomicU64,
}

impl core::fmt::Debug for ProbeCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `CachedEntry` is not `Debug`; the entry count is what a reader wants.
        f.debug_struct("ProbeCache")
            .field("entries", &self.entry_count())
            .finish_non_exhaustive()
    }
}

impl ProbeCache {
    fn get(&self, key: &CachedKey, hits: &AtomicU64, misses: &AtomicU64) -> Option<CachedEntry> {
        let found = self.entries.lock().expect("cache mutex").get(key).cloned();
        if found.is_some() {
            hits.fetch_add(1, Ordering::Relaxed);
        } else {
            misses.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    fn reset(&self) {
        for counter in [
            &self.block_hits,
            &self.block_misses,
            &self.meta_hits,
            &self.meta_misses,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }

    fn report(&self) -> String {
        format!(
            "blocks {}/{} hit, metadata {}/{} hit",
            self.block_hits.load(Ordering::Relaxed),
            self.block_hits.load(Ordering::Relaxed) + self.block_misses.load(Ordering::Relaxed),
            self.meta_hits.load(Ordering::Relaxed),
            self.meta_hits.load(Ordering::Relaxed) + self.meta_misses.load(Ordering::Relaxed),
        )
    }
}

#[async_trait::async_trait]
impl DbCache for ProbeCache {
    async fn get_block(&self, key: &CachedKey) -> Result<Option<CachedEntry>, slatedb::Error> {
        Ok(self.get(key, &self.block_hits, &self.block_misses))
    }

    async fn get_index(&self, key: &CachedKey) -> Result<Option<CachedEntry>, slatedb::Error> {
        Ok(self.get(key, &self.meta_hits, &self.meta_misses))
    }

    async fn get_filter(&self, key: &CachedKey) -> Result<Option<CachedEntry>, slatedb::Error> {
        Ok(self.get(key, &self.meta_hits, &self.meta_misses))
    }

    async fn get_stats(&self, key: &CachedKey) -> Result<Option<CachedEntry>, slatedb::Error> {
        Ok(self.get(key, &self.meta_hits, &self.meta_misses))
    }

    async fn insert(&self, key: CachedKey, value: CachedEntry) {
        self.entries.lock().expect("cache mutex").insert(key, value);
    }

    async fn remove(&self, key: &CachedKey) {
        self.entries.lock().expect("cache mutex").remove(key);
    }

    fn entry_count(&self) -> u64 {
        self.entries.lock().expect("cache mutex").len() as u64
    }
}

// --- the fixture ----------------------------------------------------------

/// Which cache an arm runs with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cache {
    /// Whatever `Db::builder(..).build()` does in this build, with no cache
    /// call at all — the exact path `SlateStore::open` takes. This arm is the
    /// proof: if it matches [`Cache::Off`] rather than [`Cache::On`], then the
    /// cache SlateDB installs by itself is doing nothing here.
    Default,
    /// `with_db_cache_disabled()`: no cache, said explicitly.
    Off,
    /// A cache installed, which is what SlateDB's own defaults would do with
    /// `foyer` or `moka` compiled in.
    On,
}

struct Arm {
    label: &'static str,
    cache: Cache,
    tuning: ScanTuning,
}

/// Seed a fresh database, close it so the rows are in SSTs rather than a
/// memtable, and hand back the object store with its counters.
async fn seeded() -> (Arc<dyn ObjectStore>, Arc<Counts>) {
    let counting = Arc::new(CountingStore::new(Arc::new(InMemory::new())));
    let counts = counting.counts();
    let store: Arc<dyn ObjectStore> = counting;

    let db = Db::builder(DB_PATH, Arc::clone(&store))
        .build()
        .await
        .expect("open SlateDB");
    let backend = Arc::new(
        SlateStore::from_db(Arc::new(db)).with_durability(slate_slatedb::Durability::Visible),
    );
    let records = RecordStore::new(Arc::clone(&backend), catalog(), security());
    let table = events();
    let ctx = context(TENANT);

    for chunk_start in (0..rows()).step_by(1_000) {
        let rows: Vec<Row> = (chunk_start..(chunk_start + 1_000).min(rows()))
            .map(|id| row(TENANT, id))
            .collect();
        let txn = records.begin().await.expect("begin");
        txn.insert_many(&ctx, &table, &rows).await.expect("insert");
        txn.commit().await.expect("commit");
    }
    // Closing flushes; the reopen below then reads object storage.
    backend.close().await.expect("close");
    (store, counts)
}

/// Reopen the seeded database under `arm`'s cache and scan tuning.
async fn reopen(
    store: &Arc<dyn ObjectStore>,
    arm: &Arm,
) -> (Arc<SlateStore>, Option<Arc<ProbeCache>>) {
    let builder = Db::builder(DB_PATH, Arc::clone(store));
    let (db, probe) = match arm.cache {
        Cache::Default => (builder, None),
        Cache::Off => (builder.with_db_cache_disabled(), None),
        Cache::On => {
            let probe = Arc::new(ProbeCache::default());
            (
                builder.with_db_cache(Arc::clone(&probe) as Arc<dyn DbCache>),
                Some(probe),
            )
        }
    };
    let db = db.build().await.expect("reopen SlateDB");
    (
        Arc::new(SlateStore::from_db(Arc::new(db)).with_scan_tuning(arm.tuning)),
        probe,
    )
}

/// The keys a point-read pass touches. Spread across the table so the reads
/// are not all in one block, and fixed so every arm reads the same rows.
fn probe_keys() -> Vec<Vec<Value>> {
    (0..READS)
        .map(|i| {
            let id = (i as u64 * (rows() / READS as u64).max(1)) % rows();
            vec![Value::U64(TENANT), Value::U64(id)]
        })
        .collect()
}

struct Phase {
    /// Wall clock per operation.
    time: Measure,
    /// Object-store GETs per operation, median over runs.
    gets: f64,
    /// GETs on the very first (cold) run of this phase.
    cold_gets: u64,
    /// What the cache did on the cold run.
    cold_cache: String,
}

struct Outcome {
    label: &'static str,
    /// Distinct point reads spread across the table.
    spread: Phase,
    /// The same key, over and over.
    repeat: Phase,
    /// A full scan of the table.
    scan: Phase,
}

/// Run one phase: a fresh reopen for the cold run, then `runs()` timed runs.
///
/// The reopen is what makes the cold run cold — a cache that has already been
/// through another phase would answer from what that phase left behind, and
/// then no arm's first number would mean anything.
async fn phase<F>(
    label: String,
    ops: usize,
    store: &Arc<dyn ObjectStore>,
    arm: &Arm,
    counts: &Counts,
    body: F,
) -> Phase
where
    F: AsyncFn(&RecordStore<Arc<SlateStore>>),
{
    let (backend, probe) = reopen(store, arm).await;
    let records = RecordStore::new(backend, catalog(), security());

    counts.reset();
    if let Some(probe) = &probe {
        probe.reset();
    }
    body(&records).await;
    let cold_gets = counts.gets();
    let cold_cache = probe
        .as_ref()
        .map_or_else(|| "no cache of ours installed".to_owned(), |p| p.report());

    let mut time = Measure::new(label, ops);
    let mut gets = Vec::new();
    for _ in 0..runs() {
        counts.reset();
        let started = Instant::now();
        body(&records).await;
        time.run(started.elapsed());
        gets.push(counts.gets() as f64 / ops as f64);
    }
    Phase {
        time,
        gets: median(&gets),
        cold_gets,
        cold_cache,
    }
}

async fn run(arm: &Arm) -> Outcome {
    let (store, counts) = seeded().await;
    let table = events();
    let ctx = context(TENANT);
    let keys = probe_keys();

    let spread = phase(
        format!("{}: point read, spread over the table", arm.label),
        READS,
        &store,
        arm,
        &counts,
        async |records: &RecordStore<Arc<SlateStore>>| {
            let snapshot = records.snapshot().await.expect("snapshot");
            for key in &keys {
                let found = snapshot.get(&ctx, &table, key).await.expect("get");
                assert!(found.is_some(), "the fixture row is missing");
            }
        },
    )
    .await;

    let repeat = phase(
        format!("{}: point read, one key", arm.label),
        READS,
        &store,
        arm,
        &counts,
        async |records: &RecordStore<Arc<SlateStore>>| {
            let snapshot = records.snapshot().await.expect("snapshot");
            for _ in 0..READS {
                let found = snapshot.get(&ctx, &table, &keys[0]).await.expect("get");
                assert!(found.is_some(), "the fixture row is missing");
            }
        },
    )
    .await;

    let scan = phase(
        format!("{}: full scan of {} rows", arm.label, rows()),
        1,
        &store,
        arm,
        &counts,
        async |records: &RecordStore<Arc<SlateStore>>| {
            let snapshot = records.snapshot().await.expect("snapshot");
            let rows = snapshot
                .execute(&ctx, &table, &Query::all())
                .await
                .expect("scan")
                .count()
                .await
                .expect("count");
            assert_eq!(rows as u64, self::rows(), "a scan lost rows");
        },
    )
    .await;

    Outcome {
        label: arm.label,
        spread,
        repeat,
        scan,
    }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied().unwrap_or(f64::NAN)
}

#[tokio::main]
async fn main() {
    slate_slatedb::announce();
    println!("# Does SlateDB have a block cache in this build?\n");
    println!(
        "Fixture: {} rows of the `events` table, written, closed, reopened, over an\n\
         in-memory object store wrapped in a counting store. Runs: {}, `median [min – max]`.\n",
        rows(),
        runs()
    );
    println!(
        "GET counts are the finding. Wall clock over an in-memory object store is a\n\
         *lower bound*: here an avoided GET costs a memcpy, in a bucket it is a round trip.\n"
    );

    let arms = [
        Arm {
            label: "as shipped (default)",
            cache: Cache::Default,
            tuning: ScanTuning::default(),
        },
        Arm {
            label: "cache off, said so",
            cache: Cache::Off,
            tuning: ScanTuning::default(),
        },
        Arm {
            label: "cache on",
            cache: Cache::On,
            tuning: ScanTuning::default(),
        },
        Arm {
            label: "cache on, scans cached",
            cache: Cache::On,
            tuning: ScanTuning {
                cache_blocks: true,
                ..ScanTuning::default()
            },
        },
    ];

    let mut outcomes = Vec::new();
    for arm in &arms {
        outcomes.push(run(arm).await);
    }

    let phases: [(&str, fn(&Outcome) -> &Phase); 3] = [
        ("point read, spread over the table", |o| &o.spread),
        ("point read, one key", |o| &o.repeat),
        ("full scan", |o| &o.scan),
    ];

    println!("\n## Control: the cold run of each phase\n");
    println!("Each phase reopens the database, so its first run meets an empty cache and");
    println!("cannot hit. Where the cold numbers agree across arms, the arms differ only");
    println!("in the cache; where they do not, say so before reading anything else.\n");
    println!("{:<36} {:<26} {:>10}", "phase", "arm", "cold GETs");
    println!("{:-<76}", "");
    for (name, get) in phases {
        for outcome in &outcomes {
            println!(
                "{:<36} {:<26} {:>10}",
                name,
                outcome.label,
                get(outcome).cold_gets
            );
        }
    }

    println!("\n## Object-store GETs per operation, warm\n");
    println!("{:<36} {:<26} {:>12}", "phase", "arm", "GETs/op");
    println!("{:-<78}", "");
    for (name, get) in phases {
        for outcome in &outcomes {
            println!(
                "{:<36} {:<26} {:>12.2}",
                name,
                outcome.label,
                get(outcome).gets
            );
        }
    }

    println!("\n## Wall clock (lower bound; the object store is in memory)\n");
    for (_, get) in phases {
        for outcome in &outcomes {
            println!("  {}", get(outcome).time.line());
        }
        println!();
    }

    println!("## What the cache did on the cold run\n");
    for (name, get) in phases {
        for outcome in &outcomes {
            println!(
                "  {:<34} {:<26} {}",
                name,
                outcome.label,
                get(outcome).cold_cache
            );
        }
    }

    println!("\n## Verdict\n");
    for (name, get) in phases {
        let Some(off) = outcomes.first() else {
            continue;
        };
        for outcome in outcomes.iter().skip(1) {
            println!(
                "  {:<34} {:<26} {}",
                name,
                outcome.label,
                difference(&get(off).time, &get(outcome).time)
            );
        }
        let off_gets = get(off).gets;
        for outcome in outcomes.iter().skip(1) {
            let on_gets = get(outcome).gets;
            let ratio = if on_gets > 0.0 {
                format!("{:.1}x fewer", off_gets / on_gets)
            } else if off_gets > 0.0 {
                "to zero".to_owned()
            } else {
                "both zero".to_owned()
            };
            println!(
                "  {:<34} {:<26} {off_gets:.2} -> {on_gets:.2} GETs/op ({ratio})",
                name, outcome.label
            );
        }
        println!();
    }
    let _ = duration(0.0);
}
