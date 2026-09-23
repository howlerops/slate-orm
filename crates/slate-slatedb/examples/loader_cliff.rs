//! Where does the loader stop being linear, and is `l0_sst_size_bytes` why?
//!
//! `README.md` has carried this as open since #118: loading rows through
//! SlateDB is linear in the row count until somewhere between 500,000 and
//! 600,000, past which four attempts never finished. The recorded hypothesis
//! is that SlateDB's default `l0_sst_size_bytes` of 64 MiB "lines up
//! suspiciously well" with where it happens, and the recorded objection is
//! that **the size that would settle it is the size that will not finish.**
//!
//! That objection assumes the test has to be run *at* the cliff. It does not.
//! SlateDB applies write backpressure when L0 fills, and L0's capacity is
//! `l0_max_ssts` × `l0_sst_size_bytes` — 8 × 64 MiB = 512 MiB by default,
//! which is about where 500,000 rows of this fixture land. If that is the
//! mechanism, then **shrinking `l0_sst_size_bytes` shrinks L0 in proportion
//! and the cliff moves down with it**, to a row count that loads in a minute.
//! A prediction that names where the cliff will be is worth more than another
//! run that cannot reach it.
//!
//! So this loads in chunks, timing each one, at whatever `l0_sst_size_bytes`
//! it is given:
//!
//! ```text
//! L0_SST_MB=8 SCALE_ROWS=120000 cargo run --release \
//!     -p slate-slatedb --example loader_cliff
//! ```
//!
//! # Reading the output
//!
//! Each row is one chunk of `CHUNK` rows and the seconds it took. Linear
//! loading means a flat column. The cliff is where that column stops being
//! flat — not one slow chunk, which is noise, but a level shift that persists.
//!
//! There is deliberately no predicted row count printed here. The in-process
//! `s3s` counts requests and not bytes, so a prediction would have to divide
//! L0's capacity by a *recorded* bytes-per-row literal — and a literal copied
//! into a measurement is what half this repository's corrections have been
//! about. The claim under test needs no such number: run this at two values
//! of `L0_SST_MB` and see whether the cliff moves in proportion to the knob.
//!
//! # What it found
//!
//! Both halves, and the second corrects the first.
//!
//! There is **no cliff** on this build: 600,000 rows load in 12.6 seconds and
//! a million in 22, three runs each, against a recorded "did not finish in
//! five minutes, on three attempts". The disk is not it either — the `MiB on
//! disk` column exists because nothing had ever weighed the fake S3's
//! directory, and the store turns out to want 238 bytes a row, so the sizes
//! that would not finish were never near this container's free space.
//!
//! But there **is** a reproducible event, and `l0_sst_size_bytes` places it
//! linearly: a compaction every ~510,000 rows at the stock 64 MiB, every
//! ~130,000 at 16 MiB, every ~60,000 at 8 MiB. The directory nearly doubles,
//! PUTs jump from 2 a chunk to 9, and the chunk takes about 0.3 s longer.
//! The first one at the stock setting lands inside the 500,000–600,000 band
//! the cliff was recorded in.
//!
//! So the knob hypothesis was right about *where* and wrong about *why*, and
//! the run above that cleared it went too far: it tested backpressure, which
//! really is not the mechanism, and concluded the knob had no part. Reading
//! only the timing column is what made that possible — the disk column is
//! what separates "nothing happens" from "something happens and it is small".
//!
//! # What this cannot say
//!
//! It measures one row shape against a loopback `s3s`, so the *rows* at which
//! a cliff appears are a property of this fixture and this machine. What
//! transfers is whether the cliff **moves with the knob**, which is a question
//! about mechanism rather than about any particular number.

// The same set every example in this crate allows: a benchmark that returns a
// `Result` up a chain nobody reads is noise, and a panic here is the correct
// outcome — a fixture that will not load has no measurement to report.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

#[path = "../tests/common/s3server.rs"]
mod s3server;

use slate_kernel::{Action, Grant, RecordStore, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_slatedb::SlateStore;
use slate_tuple::{Value, ValueType};
use slatedb::Db;
use slatedb::config::Settings;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const EVENTS: TableId = TableId(1);
const BUCKETS: u64 = 500;
/// Rows per timed chunk. Small enough that a cliff lands inside the run
/// rather than between two points, large enough that per-chunk noise averages
/// out.
const CHUNK: u64 = 10_000;
/// Rows per transaction, matching `cost_at_scale` so the two are comparable.
const BATCH: u64 = 5_000;

fn rows() -> u64 {
    std::env::var("SCALE_ROWS")
        .ok()
        .and_then(|value| value.split(',').next().unwrap_or("").parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(120_000)
}

/// `l0_sst_size_bytes`, in MiB. The knob the README's hypothesis named.
fn l0_sst_mb() -> usize {
    env_mb("L0_SST_MB", 64)
}

/// `max_unflushed_bytes`, in MiB. The knob that turned out to bind first.
///
/// SlateDB pauses writers when unflushed key/value bytes exceed this, and it
/// defaults to 1 GiB — sixteen times the L0 SST size and, unlike L0, reached
/// by a loader that writes faster than it flushes. Shrinking `L0_SST_MB`
/// eightfold moved nothing, because at these sizes the data never reaches L0
/// at all: a 120,000-row load issues under thirty PUTs.
fn max_unflushed_mb() -> usize {
    env_mb("MAX_UNFLUSHED_MB", 1024)
}

fn env_mb(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(fallback)
}

/// Bytes the fake S3 server is holding on real disk.
///
/// `s3s_fs` writes objects to a temporary directory, so a load competes for
/// free space with everything else on this container. Nothing measured that
/// before: the request counters count *requests*, and a 120,000-row load
/// issues under thirty of them, so PUT counts say nothing about volume.
/// `performance.md` has listed the disk as an unruled-out cause of the loader
/// cliff since #118 while giving no way to weigh it.
///
/// Walks rather than shelling out to `du`, so the number is the same on any
/// machine that can run the example, and sums apparent size rather than
/// blocks — what the store *wrote*, not how the filesystem rounded it.
fn bytes_on_disk(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => bytes_on_disk(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// A load that left nothing on disk did not happen.
///
/// The disk column is the whole point of this example now, and a reported
/// number nothing checks is the defect half this repository's corrections have
/// been about. `run_examples.sh` runs this at 2,000 rows, so a walk that
/// stopped recursing — or an `s3s_fs` that stopped writing — fails there
/// rather than printing a quiet 0.
///
/// It is its own function rather than an `assert!` inline in `main` because
/// `cargo test --example` never runs `main`, so left inline the one check that
/// guards every reported number would be reachable only through
/// `run_examples.sh`. Split out, `a_load_that_wrote_nothing_is_refused` covers
/// the condition directly.
///
/// The runner does reach it, and does run on this container — in `--release`,
/// per the recipe in `CLAUDE.md`; it is the *debug* build of these nine
/// examples that exhausts the disk. Breaking the directory walk so a real
/// store reads as empty fails here as `loader_cliff, exit 101`, which is this
/// guard firing. An earlier version of this comment said the suite could not
/// be run at all, and a mutation was recorded unverifiable on that basis.
fn assert_wrote_something(on_disk: u64, total: u64) {
    assert!(
        on_disk > 0,
        "loaded {total} rows and the server's directory is empty: either the \
         walk is broken or nothing was written"
    );
}

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("id", ValueType::U64)
        .column("bucket", ValueType::I64)
        .column("kind", ValueType::Str)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_bucket", IndexId(10)).column("bucket"))
        .build()
        .expect("valid schema")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::I64((id % BUCKETS) as i64),
        Value::Str(format!("kind-{}", id % 20)),
        Value::Str(format!(
            "body for row {id}, padded out to a realistic width"
        )),
    ])
}

#[tokio::main]
async fn main() {
    slate_slatedb::announce();
    let server = s3server::LocalS3::start("slate-orm").await;
    let counters = server.counters();
    let total = rows();
    let mb = l0_sst_mb();
    let unflushed_mb = max_unflushed_mb();

    // Built from the defaults and overridden, rather than constructed field by
    // field: a hand-written `Settings` would silently stop tracking SlateDB's
    // other defaults the moment one of them changed, and this example's whole
    // claim is that only one knob differs from a stock database.
    let settings = Settings {
        l0_sst_size_bytes: mb * 1024 * 1024,
        max_unflushed_bytes: unflushed_mb * 1024 * 1024,
        ..Settings::default()
    };
    let l0_capacity = settings.l0_max_ssts * settings.l0_sst_size_bytes;

    println!("# Where does the loader stop being linear?");
    println!();
    println!("l0_sst_size_bytes : {} MiB", mb);
    println!("l0_max_ssts       : {}", settings.l0_max_ssts);
    println!(
        "L0 capacity       : {:.0} MiB before backpressure",
        l0_capacity as f64 / (1024.0 * 1024.0)
    );
    println!("max_unflushed_bytes: {unflushed_mb} MiB before writers pause");
    println!("rows              : {total}, in chunks of {CHUNK}");
    println!();

    let path = format!("/cliff-{mb}-{unflushed_mb}-{}", std::process::id());
    let object_store = server.config().build().expect("object store");
    let store_handle = Db::builder(path.as_str(), object_store)
        .with_settings(settings.clone())
        .build()
        .await
        .expect("open db with settings");

    let backend = SlateStore::from_db(Arc::new(store_handle));
    let catalog = Catalog::from_tables([events()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", EVENTS, Action::ALL));
    let store = RecordStore::new(backend, catalog, security);
    let root = SecurityContext::superuser();

    counters.reset();
    println!(
        "{:>10}  {:>9}  {:>10}  {:>12}  {:>12}",
        "rows", "chunk s", "rows/s", "PUTs so far", "MiB on disk"
    );
    println!("{:-<62}", "");

    let mut written = 0u64;
    let mut timings: Vec<(u64, f64)> = Vec::new();
    let started = Instant::now();
    while written < total {
        let chunk_end = (written + CHUNK).min(total);
        let chunk_started = Instant::now();
        let mut at = written;
        while at < chunk_end {
            let end = (at + BATCH).min(chunk_end);
            let batch: Vec<Row> = (at..end).map(row).collect();
            let txn = store.begin().await.expect("begin");
            txn.insert_many(&root, &events(), &batch)
                .await
                .expect("insert");
            txn.commit().await.expect("commit");
            at = end;
        }
        let seconds = chunk_started.elapsed().as_secs_f64();
        written = chunk_end;
        timings.push((written, seconds));
        println!(
            "{:>10}  {:>9.2}  {:>10.0}  {:>12}  {:>12.1}",
            written,
            seconds,
            CHUNK as f64 / seconds.max(1e-9),
            counters.puts(),
            bytes_on_disk(server.directory()) as f64 / (1024.0 * 1024.0)
        );
    }
    let wall = started.elapsed().as_secs_f64();
    store.backend().close().await.expect("close");

    println!();
    let on_disk = bytes_on_disk(server.directory());
    println!(
        "loaded {total} rows in {wall:.1}s, {} PUTs",
        counters.puts()
    );
    println!(
        "on disk           : {:.1} MiB, {:.0} bytes a row",
        on_disk as f64 / (1024.0 * 1024.0),
        on_disk as f64 / total as f64
    );
    assert_wrote_something(on_disk, total);

    // A level shift, not a single slow chunk. The first quarter is the
    // baseline because it is before any plausible cliff at these sizes; a
    // chunk is called slow when it takes half again as long as that.
    let baseline: f64 = {
        let head = timings.len().div_ceil(4).max(1);
        timings.iter().take(head).map(|(_, s)| *s).sum::<f64>() / head as f64
    };
    let threshold = baseline * 1.5;
    let first_slow = timings.iter().find(|(_, s)| *s > threshold);
    println!("baseline chunk    : {baseline:.2}s, so a chunk is slow past {threshold:.2}s");
    match first_slow {
        Some((at, seconds)) => {
            println!("observed cliff    : first slow chunk ends at {at} rows ({seconds:.2}s)")
        }
        None => println!("observed cliff    : none — every chunk stayed under {threshold:.2}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::{assert_wrote_something, bytes_on_disk};

    /// The walk recurses, and sums apparent size rather than counting files.
    ///
    /// Built rather than pointed at a real store: a store's layout is
    /// SlateDB's to change, and a test that breaks when it does would be
    /// testing the dependency. What is this example's own is the arithmetic.
    #[test]
    fn sums_every_file_at_every_depth() {
        let root = tempfile::tempdir().expect("temp dir");
        std::fs::write(root.path().join("a"), vec![0u8; 10]).expect("write a");
        let nested = root.path().join("sub").join("deeper");
        std::fs::create_dir_all(&nested).expect("create nested");
        std::fs::write(root.path().join("sub").join("b"), vec![0u8; 5]).expect("write b");
        std::fs::write(nested.join("c"), vec![0u8; 7]).expect("write c");

        assert_eq!(bytes_on_disk(root.path()), 22);
    }

    /// An empty directory weighs nothing, and so does one that is not there.
    ///
    /// The second case is why the walk returns 0 rather than panicking on a
    /// missing path: it is called once per chunk while the server is running,
    /// and a measurement that aborts the run it is measuring is worse than a
    /// measurement that reads low for one line.
    #[test]
    fn empty_and_missing_both_weigh_nothing() {
        let root = tempfile::tempdir().expect("temp dir");
        assert_eq!(bytes_on_disk(root.path()), 0);
        assert_eq!(bytes_on_disk(&root.path().join("not-there")), 0);
    }

    /// The empty store is refused, which is what makes every disk figure here
    /// a measurement rather than a print statement.
    ///
    /// `catch_unwind` rather than `#[should_panic]`, and the reason is about
    /// the tooling: libtest prints a failing should-panic case as
    /// `test NAME - should panic ... FAILED`, and `scripts/mutate.py` reads
    /// `test NAME ... FAILED`, so the extra words make a *caught* mutation
    /// score as unreadable. Verified by hitting it. A test whose failures the
    /// mutation runner cannot read is a test that cannot defend the code.
    #[test]
    fn a_load_that_wrote_nothing_is_refused() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| assert_wrote_something(0, 2_000));
        std::panic::set_hook(previous);
        assert!(outcome.is_err(), "an empty store was accepted");
    }

    /// And a single byte is enough to say the write path ran. The threshold is
    /// deliberately not a per-row floor: this example is the thing that
    /// *measures* bytes a row, so a floor here would be the recorded literal
    /// #292 refused to print.
    #[test]
    fn one_byte_is_enough() {
        assert_wrote_something(1, 2_000);
    }
}
