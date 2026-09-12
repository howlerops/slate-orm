//! Does the cost model predict what object storage actually charges?
//!
//! ```sh
//! cargo run --release -p slate-slatedb --example cost_calibration
//! ```
//!
//! The planner costs plans in object-storage round trips, and every number in
//! `docs/performance.md` comes from multiplying that cost by 2.2 ms — a figure
//! taken from a *fixture*, not from storage. So the model has been calibrated
//! against itself, which is exactly the mistake this project has made before
//! and written down twice.
//!
//! This measures the two halves separately, against an S3 server running in
//! the process:
//!
//! - **Is the unit right?** For each query shape, what the planner predicted
//!   in round trips against the GETs the server actually served.
//! - **Is the constant right?** Wall time divided by GETs served, which is
//!   what a round trip costs here.
//!
//! The absolute latency is not AWS's — a loopback socket is far faster than a
//! real network, and this makes no claim otherwise. What transfers is the
//! *ratio*: whether a plan the model calls twice as expensive really does
//! twice the work. That is the part the planner's decisions rest on, and it is
//! measurable without a cloud account.

// Benchmark code, and meant to panic if an assumption about the fixture
// breaks: a silently short result table would be worse than a stack trace.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::cast_precision_loss
)]

#[path = "../tests/common/s3server.rs"]
mod s3server;

use slate_kernel::{
    Action, CmpOp, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext, Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_slatedb::SlateStore;
use slate_tuple::{Value, ValueType};
use std::time::Instant;

const EVENTS: TableId = TableId(1);
const ROWS: u64 = 200_000;

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

fn col(name: &str) -> Ordinal {
    events().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::I64((id % 500) as i64),
        Value::Str(format!("kind-{}", id % 20)),
        // Enough body that a row is a realistic size rather than a few bytes,
        // so blocks fill at a rate that resembles real data.
        Value::Str(format!(
            "body for row {id}, padded out to a realistic width"
        )),
    ])
}

#[tokio::main]
async fn main() {
    let server = s3server::LocalS3::start("slate-orm").await;
    let counters = server.counters();
    let path = format!("/calibration-{}", std::process::id());

    println!("Loading {ROWS} rows through SlateDB over S3...");
    let load = Instant::now();
    {
        let backend = SlateStore::open_s3(path.clone(), server.config())
            .await
            .expect("open");
        let catalog = Catalog::from_tables([events()]).expect("catalog");
        let security = SecurityCatalog::new().grant(Grant::new("r", EVENTS, Action::ALL));
        let store = RecordStore::new(backend, catalog, security);
        let root = SecurityContext::superuser();

        for chunk in 0..(ROWS / 5_000) {
            let rows: Vec<Row> = (chunk * 5_000..(chunk + 1) * 5_000).map(row).collect();
            let txn = store.begin().await.unwrap();
            txn.insert_many(&root, &events(), &rows).await.unwrap();
            txn.commit().await.unwrap();
        }
        store.backend().close().await.unwrap();
    }
    println!(
        "  loaded in {:.1}s, {} puts to object storage\n",
        load.elapsed().as_secs_f64(),
        counters.puts()
    );

    // Reopened, so the data is genuinely in object storage rather than a
    // memtable. Without this every query would be served from memory and the
    // GET counts would all be zero.
    let backend = SlateStore::open_s3(path, server.config())
        .await
        .expect("reopen");
    let catalog = Catalog::from_tables([events()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", EVENTS, Action::ALL));
    let mut store = RecordStore::new(backend, catalog, security);
    let root = SecurityContext::superuser();

    let stats = {
        let txn = store.begin().await.unwrap();
        txn.analyze(&root, &events()).await.unwrap()
    };
    let mut all = Statistics::default();
    all.set(EVENTS, stats);
    store.set_statistics(all);

    let cases: Vec<(&str, Query)> = vec![
        (
            "point get by primary key",
            Query::all().filter(Expr::eq(col("id"), Value::U64(12_345))),
        ),
        (
            "index equality (one bucket)",
            Query::all().filter(Expr::eq(col("bucket"), Value::I64(7))),
        ),
        (
            "narrow key range",
            Query::all().filter(
                Expr::compare(col("id"), CmpOp::Ge, Value::U64(1_000)).and(Expr::compare(
                    col("id"),
                    CmpOp::Lt,
                    Value::U64(2_000),
                )),
            ),
        ),
        (
            "wide key range",
            Query::all().filter(Expr::compare(col("id"), CmpOp::Lt, Value::U64(50_000))),
        ),
        ("full scan", Query::all()),
    ];

    println!(
        "{:<28} {:>7} {:>10} {:>8} {:>9} {:>11}",
        "query", "rows", "predicted", "GETs", "wall", "ms per GET"
    );
    println!("{:-<78}", "");

    let mut points = Vec::new();
    for (name, query) in cases {
        let predicted = {
            let txn = store.begin().await.unwrap();
            txn.explain(&root, &events(), &query)
                .unwrap()
                .estimated_cost
        };

        counters.reset();
        let started = Instant::now();
        let rows = {
            let txn = store.begin().await.unwrap();
            txn.execute(&root, &events(), &query)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap()
                .len()
        };
        let wall = started.elapsed();
        let gets = counters.gets();

        let per_get = if gets == 0 {
            f64::NAN
        } else {
            wall.as_secs_f64() * 1000.0 / gets as f64
        };
        println!(
            "{name:<28} {rows:>7} {predicted:>10.1} {gets:>8} {:>8.2}s {per_get:>10.3}",
            wall.as_secs_f64()
        );
        if gets > 0 {
            points.push((name, predicted, gets as f64, per_get));
        }
    }

    // The decisive question: for a query where the model and reality disagree
    // most, does the planner actually pick the slower plan?
    println!("\n--- does the disagreement change a decision? ---");
    let contested = Query::all().filter(Expr::eq(col("bucket"), Value::I64(7)));
    for (label, hint) in [
        ("planner's choice", None),
        (
            "forced table scan",
            Some(slate_kernel::AccessHint::TableScan),
        ),
        (
            "forced index",
            Some(slate_kernel::AccessHint::Index(IndexId(10))),
        ),
    ] {
        let mut query = contested.clone();
        query.hint = hint;
        let explained = {
            let txn = store.begin().await.unwrap();
            txn.explain(&root, &events(), &query).unwrap()
        };
        counters.reset();
        let started = Instant::now();
        let rows = {
            let txn = store.begin().await.unwrap();
            txn.execute(&root, &events(), &query)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap()
                .len()
        };
        println!(
            "  {label:<20} {:<28} cost {:>8.1}  {:>6} GETs  {:>7.2}s  ({rows} rows)",
            format!("{:?}", explained.access)
                .split_whitespace()
                .next()
                .unwrap_or("?"),
            explained.estimated_cost,
            counters.gets(),
            started.elapsed().as_secs_f64()
        );
    }

    // The ORM shape: one parent row, then its children out of a large table.
    // Intuition says probe; the recalibrated model says scan. Intuition has
    // been wrong here before, so measure it.
    println!("\n--- one row against a large table: probe, or scan? ---");
    {
        let one_bucket = Expr::eq(col("bucket"), Value::I64(11));
        // A probe-shaped access: fetch the handful of rows for one bucket
        // through the index, which is what a nested loop does per outer row.
        counters.reset();
        let started = Instant::now();
        let mut probe_query = Query::all().filter(one_bucket.clone());
        probe_query.hint = Some(slate_kernel::AccessHint::Index(IndexId(10)));
        let probed = {
            let txn = store.begin().await.unwrap();
            txn.execute(&root, &events(), &probe_query)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap()
                .len()
        };
        let probe_wall = started.elapsed();
        let probe_gets = counters.gets();

        counters.reset();
        let started = Instant::now();
        let mut scan_query = Query::all().filter(one_bucket);
        scan_query.hint = Some(slate_kernel::AccessHint::TableScan);
        let scanned = {
            let txn = store.begin().await.unwrap();
            txn.execute(&root, &events(), &scan_query)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap()
                .len()
        };
        let scan_wall = started.elapsed();
        let scan_gets = counters.gets();

        println!(
            "  {probed} rows via index:      {probe_gets:>6} GETs  {:>7.2}s",
            probe_wall.as_secs_f64()
        );
        println!(
            "  same rows via full scan: {scan_gets:>6} GETs  {:>7.2}s  (of {ROWS} rows)",
            scan_wall.as_secs_f64()
        );
        println!(
            "  => scanning {} rows beats {probed} point reads: {}",
            ROWS,
            scan_wall < probe_wall
        );
        assert_eq!(probed, scanned, "the two paths disagreed on the answer");
    }

    println!("\n--- is the unit right? ---");
    println!(
        "The model counts round trips. If it is measuring the right thing, the\n\
         ratio of predicted cost to GETs served should be roughly constant\n\
         across shapes, even if the constant itself is not 1."
    );
    println!("\n{:<28} {:>12}", "query", "GETs / cost");
    for (name, predicted, gets, _) in &points {
        let ratio = if *predicted > 0.0 {
            gets / predicted
        } else {
            f64::NAN
        };
        println!("{name:<28} {ratio:>12.2}");
    }

    println!("\n--- is the constant right? ---");
    let measured: Vec<f64> = points.iter().map(|(_, _, _, per)| *per).collect();
    if !measured.is_empty() {
        let mean = measured.iter().sum::<f64>() / measured.len() as f64;
        println!(
            "Measured {:.3} ms per GET against a loopback S3 server, over {} shapes.\n\
             `docs/performance.md` uses 2.2 ms, which is a wide-area figure — this\n\
             number is not a correction to it, and the two are not comparable.\n\
             What is comparable is the shape of the table above.",
            mean,
            measured.len()
        );
    }

    store.backend().close().await.unwrap();
}
