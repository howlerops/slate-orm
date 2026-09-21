//! What SlateDB's scan settings are worth, against a real S3 server.
//!
//! ```sh
//! cargo run --release -p slate-slatedb --example scan_tuning
//! ```
//!
//! Every other measurement in this project runs against a latency *model*.
//! This one does not: it starts an S3 server in the process, writes a table,
//! reopens the database so the data is genuinely in object storage rather than
//! a memtable, and scans it.
//!
//! The question is narrow. SlateDB can fetch several blocks per request and
//! several requests at once, and ships both off — `read_ahead_bytes: 1`,
//! `max_fetch_tasks: 1`. We were passing its defaults, so a scan paid a round
//! trip per block, in turn. This measures what turning that on is worth.

// Benchmark code, and meant to panic if an assumption about the fixture
// breaks: a silently short result table would be worse than a stack trace.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::unwrap_used
)]

#[path = "../tests/common/s3server.rs"]
mod s3server;

use slate_kernel::{Action, Grant, Query, RecordStore, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, Row, TableDef, TableId};
use slate_slatedb::{ScanTuning, SlateStore};
use slate_tuple::{Value, ValueType};
use std::time::Instant;

const EVENTS: TableId = TableId(1);
const ROWS: u64 = 20_000;

/// `SCALE_ROWS` overrides it, the same name `cost_at_scale` takes.
///
/// One name per crate rather than one per example: a smoke run has to shrink
/// every fixture it meets, and a runner that needs a different variable for
/// each is a list nobody keeps current. The default is what
/// `docs/performance.md` records and is unchanged.
fn rows() -> u64 {
    std::env::var("SCALE_ROWS")
        .ok()
        .and_then(|value| value.split(',').next().unwrap_or("").parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS)
}

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 8)),
        // Wide enough that the table spans many blocks, which is the whole
        // point: a scan of one block cannot show what readahead is worth.
        Value::Str("x".repeat(200)),
    ])
}

fn catalog() -> Catalog {
    Catalog::from_tables([events()]).expect("catalog")
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("bench", EVENTS, Action::ALL))
}

/// Write the table and get it out of memory and into object storage.
async fn load(path: &str, config: slate_slatedb::S3Config) {
    let backend = SlateStore::open_s3(path.to_owned(), config)
        .await
        .expect("open");
    let store = RecordStore::new(backend.clone(), catalog(), security());
    let root = SecurityContext::superuser();

    for chunk in (0..rows()).collect::<Vec<_>>().chunks(1_000) {
        let rows: Vec<Row> = chunk.iter().map(|id| row(*id)).collect();
        let txn = store.begin().await.expect("begin");
        txn.insert_many(&root, &events(), &rows)
            .await
            .expect("write");
        txn.commit().await.expect("commit");
    }
    // Closing flushes; reopening below then reads SSTs rather than a memtable.
    backend.close().await.expect("close");
}

async fn scan_once(
    path: &str,
    config: slate_slatedb::S3Config,
    tuning: ScanTuning,
    counters: &s3server::S3Counters,
) -> (usize, f64, u64) {
    let backend = SlateStore::open_s3(path.to_owned(), config)
        .await
        .expect("reopen")
        .with_scan_tuning(tuning);
    let store = RecordStore::new(backend.clone(), catalog(), security());
    let root = SecurityContext::superuser();

    let txn = store.begin().await.expect("begin");
    counters.reset();
    let started = Instant::now();
    let counted = txn
        .execute(&root, &events(), &Query::all())
        .await
        .expect("scan")
        .count()
        .await
        .expect("count");
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let gets = counters.gets();
    drop(txn);
    backend.close().await.expect("close");
    (counted, elapsed, gets)
}

#[tokio::main]
async fn main() {
    let server = s3server::LocalS3::start("slate-orm").await;
    let counters = server.counters();

    let settings: Vec<(&str, ScanTuning)> = vec![
        (
            "SlateDB defaults (what we passed)",
            ScanTuning::conservative(),
        ),
        (
            "64 KiB, one task",
            ScanTuning {
                read_ahead_bytes: 64 * 1024,
                max_fetch_tasks: 1,
                cache_blocks: false,
            },
        ),
        (
            "1 MiB, one task",
            ScanTuning {
                read_ahead_bytes: 1024 * 1024,
                max_fetch_tasks: 1,
                cache_blocks: false,
            },
        ),
        ("1 MiB, four tasks (our default)", ScanTuning::default()),
        (
            "1 MiB, eight tasks",
            ScanTuning {
                read_ahead_bytes: 1024 * 1024,
                max_fetch_tasks: 8,
                cache_blocks: false,
            },
        ),
    ];

    // Each setting gets its own database, so no block cache carries a result
    // from one measurement into the next. Loading is not timed.
    let mut paths = Vec::new();
    for index in 0..settings.len() {
        let path = format!("/scan-tuning/{index}");
        load(&path, server.config()).await;
        paths.push(path);
    }

    println!(
        "{} rows over an in-process S3 server, read back after a reopen",
        rows()
    );
    println!(
        "Each setting is measured twice: once in this order and once reversed, \n\
         so a warming server cannot be mistaken for a faster plan.\n"
    );
    println!(
        "{:<34} {:>9} {:>6} {:>9} {:>11} {:>11}",
        "scan settings", "readahead", "tasks", "S3 GETs", "forward", "reversed"
    );
    println!("{:-<86}", "");

    let mut forward = Vec::new();
    for (index, (_, tuning)) in settings.iter().enumerate() {
        let (rows, wall, gets) =
            scan_once(&paths[index], server.config(), *tuning, &counters).await;
        assert_eq!(rows as u64, self::rows(), "a scan lost rows");
        forward.push((wall, gets));
    }

    let mut reversed = vec![(0.0, 0u64); settings.len()];
    for (index, (_, tuning)) in settings.iter().enumerate().rev() {
        let (rows, wall, gets) =
            scan_once(&paths[index], server.config(), *tuning, &counters).await;
        assert_eq!(rows as u64, self::rows(), "a scan lost rows");
        reversed[index] = (wall, gets);
    }

    for (index, (label, tuning)) in settings.iter().enumerate() {
        println!(
            "{:<34} {:>9} {:>6} {:>9} {:>9.1}ms {:>9.1}ms",
            label,
            tuning.read_ahead_bytes,
            tuning.max_fetch_tasks,
            forward[index].1,
            forward[index].0,
            reversed[index].0,
        );
    }

    let (base, _) = forward[0];
    println!();
    for (index, (label, _)) in settings.iter().enumerate().skip(1) {
        println!(
            "  {label:<32} {:.0}x fewer requests, {:.0}x faster",
            forward[0].1 as f64 / forward[index].1.max(1) as f64,
            base / forward[index].0,
        );
    }
}
