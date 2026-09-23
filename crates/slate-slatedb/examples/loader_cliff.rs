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
        "{:>10}  {:>9}  {:>10}  {:>12}",
        "rows", "chunk s", "rows/s", "PUTs so far"
    );
    println!("{:-<48}", "");

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
            "{:>10}  {:>9.2}  {:>10.0}  {:>12}",
            written,
            seconds,
            CHUNK as f64 / seconds.max(1e-9),
            counters.puts()
        );
    }
    let wall = started.elapsed().as_secs_f64();
    store.backend().close().await.expect("close");

    println!();
    println!(
        "loaded {total} rows in {wall:.1}s, {} PUTs",
        counters.puts()
    );

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
