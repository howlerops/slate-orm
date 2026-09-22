//! Does the cost model still hold above the scale it was calibrated at?
//!
//! ```sh
//! cargo run --release -p slate-slatedb --example cost_at_scale
//! SCALE_ROWS=200000,600000,1000000 \
//!   cargo run --release -p slate-slatedb --example cost_at_scale
//! ```
//!
//! `cost_calibration` measured two constants against a real S3 server at
//! **200,000 rows** and nothing above it has been measured:
//!
//! - `SCAN_ROW_COST`, which is the claim that a scan returns about
//!   **8,000 rows per object-store request**;
//! - `POINT_READ_COST`, which is the claim that a point read costs about
//!   **one request**.
//!
//! Their values are not restated here. This doc said ~~`POINT_READ_COST = 3.0`~~
//! for nine tasks after #269 measured 1.0, because a literal copy of a
//! constant has nothing holding it to the constant. The English figures above
//! are derived, so `check_cost_prose.py` can and does check them.
//!
//! Both are averages over a corpus, not constants of nature, and both could
//! move with scale for reasons that have nothing to do with the model being
//! wrong: more rows means more SSTs, deeper levels, and a block cache that
//! stops holding the whole database. This example runs the same measurement at
//! several sizes and prints the two constants beside each other, so whether
//! they move is a table rather than an argument.
//!
//! # Two things this measures that `cost_calibration` does not
//!
//! **Cold against warm.** `cost_calibration` runs `analyze` — a full read of
//! the table — before it measures anything, so every number it reports is taken
//! against a block cache that has just seen the entire database. At 200,000
//! rows that may be the whole thing. This example measures the same shapes
//! *before* `analyze` as well as after, because if the two disagree then the
//! calibrated constant describes a cache-hit ratio rather than storage, and it
//! is the cold number that survives to a larger deployment.
//!
//! **Point reads in bulk.** A single point read against a warm cache can be
//! zero requests, which averages into nonsense. Here a batch of reads walks
//! pseudo-random keys across the whole keyspace and the requests are divided by
//! the batch.
//!
//! **A scan costs what the reads before it left behind.** The headline finding
//! of this file is not a constant but a variable nothing models. The same full
//! scan of the same 200,000 rows costs:
//!
//! | the store has already | GETs | rows/GET |
//! | --- | ---: | ---: |
//! | read nothing at all | 53 | 3,774 |
//! | served 200 random point reads | 205, 207, 207 | 976 |
//! | served `analyze` and 400 of them | 371 | 539 |
//!
//! Three consecutive scans give the middle row, so this is not "the first one
//! paid and the rest are free" — a *partially* populated block cache
//! fragments a scan into many small ranged reads instead of a few large ones,
//! and more of it fragments it further. `SCAN_ROW_COST` says 8,000 rows per
//! request, which is none of these; it is what a scan costs when the cache
//! already holds the whole table, which is the state `cost_calibration`
//! measures in and says so.
//!
//! This is also why that file and this one appeared to disagree by 8× about
//! the same scan on a byte-identical fixture. They do not: one scans a store
//! that has done nothing and the other one that has just done 200 point
//! reads, and both numbers are right.
//!
//! The S3 server is `s3s` in this process over a loopback socket, the same
//! fixture `cost_calibration` and `scan_tuning` use. The absolute wall times
//! are not AWS's and no claim is made that they are; what transfers is request
//! counts and their ratios.

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

use slate_kernel::stats::{POINT_READ_COST, SCAN_ROW_COST};
use slate_kernel::{
    AccessHint, Action, CmpOp, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext,
    Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_slatedb::SlateStore;
use slate_tuple::{Value, ValueType};
use std::time::Instant;

const EVENTS: TableId = TableId(1);

/// Rows per bucket value, so index selectivity is the same at every scale.
///
/// `bucket` takes `rows / BUCKETS` values… no: it takes `BUCKETS` values, so a
/// single bucket is always 1/500th of the table however large the table is.
/// Holding *selectivity* fixed rather than *row count* fixed is what makes the
/// index/scan comparison mean the same thing at every size.
const BUCKETS: u64 = 500;

/// The same schema `cost_calibration` uses, so the two are comparable.
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
        Value::I64((id % BUCKETS) as i64),
        Value::Str(format!("kind-{}", id % 20)),
        Value::Str(format!(
            "body for row {id}, padded out to a realistic width"
        )),
    ])
}

/// A deterministic walk over the keyspace.
///
/// Deterministic so two runs read the same keys, and a walk rather than a
/// sequential range so the reads do not all land in one block and report a
/// point read as free.
struct Walk(u64);

impl Walk {
    fn next(&mut self, modulus: u64) -> u64 {
        // xorshift64*, which is enough spread for choosing keys.
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % modulus
    }
}

fn scales() -> Vec<u64> {
    match std::env::var("SCALE_ROWS") {
        Ok(list) => list
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .filter(|n| *n > 0)
            .collect(),
        Err(_) => vec![200_000],
    }
}

/// What one scale produced.
struct Point {
    rows: u64,
    load_seconds: f64,
    puts: u64,
    cold_point_gets: f64,
    warm_point_gets: f64,
    cold_scan_rows_per_get: f64,
    warm_scan_rows_per_get: f64,
    analyze_seconds: f64,
    index_predicted: f64,
    index_gets: u64,
    index_rows: usize,
    scan_predicted: f64,
    scan_gets: u64,
    contested: String,
}

#[tokio::main]
async fn main() {
    let server = s3server::LocalS3::start("slate-orm").await;
    let counters = server.counters();
    let sizes = scales();

    println!("# Does the cost model hold above 200,000 rows?");
    println!();
    println!("S3 server: `s3s` in this process, over a loopback socket.");
    println!("Scales: {sizes:?}");
    println!();
    // Printed from the constants, not restated beside them. They were
    // restated, and one went stale: this line carried the old point-read
    // figure for as long as nobody ran the benchmark, which is every run
    // between #269 re-measuring it and #278 noticing. A benchmark whose
    // header misreports the model it is measuring against is worse than one
    // that prints nothing.
    println!("The two calibrated constants, read from `slate_kernel::stats` so the");
    println!("table below can be read against them:");
    println!(
        "  SCAN_ROW_COST   = {SCAN_ROW_COST}  →  a scan returns ~{:.0} rows per request",
        1.0 / SCAN_ROW_COST
    );
    println!(
        "  POINT_READ_COST = {POINT_READ_COST}       →  a point read costs {POINT_READ_COST} request(s)"
    );
    println!();

    let mut points: Vec<Point> = Vec::new();

    for rows in sizes {
        println!("\n{:=<78}", "");
        println!("== {rows} rows");
        println!("{:=<78}\n", "");

        let path = format!("/scale-{rows}-{}", std::process::id());

        // --- load ---------------------------------------------------------
        counters.reset();
        let load = Instant::now();
        {
            let backend = SlateStore::open_s3(path.clone(), server.config())
                .await
                .expect("open");
            let catalog = Catalog::from_tables([events()]).expect("catalog");
            let security = SecurityCatalog::new().grant(Grant::new("r", EVENTS, Action::ALL));
            let store = RecordStore::new(backend, catalog, security);
            let root = SecurityContext::superuser();
            let mut written = 0u64;
            while written < rows {
                let end = (written + 5_000).min(rows);
                let batch: Vec<Row> = (written..end).map(row).collect();
                let txn = store.begin().await.unwrap();
                txn.insert_many(&root, &events(), &batch).await.unwrap();
                txn.commit().await.unwrap();
                written = end;
            }
            store.backend().close().await.unwrap();
        }
        let load_seconds = load.elapsed().as_secs_f64();
        let puts = counters.puts();
        let load_gets = counters.gets();
        println!(
            "loaded in {load_seconds:.1}s, {puts} PUTs and {load_gets} GETs to object storage \
             ({:.1} µs and {:.2} GETs per row)",
            load_seconds * 1e6 / rows as f64,
            load_gets as f64 / rows as f64
        );
        if std::env::var("SCALE_LOAD_ONLY").is_ok() {
            // The load is the measurement. Used to locate the knee below,
            // where the rest of the suite would cost minutes per point and
            // answer a question the load time has already answered.
            continue;
        }

        // A scan on a *pristine* store, before anything else touches it.
        //
        // This file measures a full scan at ~205 requests where
        // `cost_calibration --cold` measures ~28 on a byte-identical fixture,
        // and repeating the scan does not make it cheaper, so it is not a
        // cache. The one difference left is that the scan there runs on a
        // store that has done nothing, and here on one that has just done 200
        // random point reads. This is that difference, measured.
        {
            let pristine = SlateStore::open_s3(path.clone(), server.config())
                .await
                .expect("reopen");
            let catalog = Catalog::from_tables([events()]).expect("catalog");
            let security = SecurityCatalog::new().grant(Grant::new("r", EVENTS, Action::ALL));
            let store = RecordStore::new(pristine, catalog, security);
            let root = SecurityContext::superuser();
            counters.reset();
            let counted = {
                let txn = store.begin().await.unwrap();
                txn.execute(&root, &events(), &Query::all())
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap()
                    .len()
            };
            assert_eq!(counted as u64, rows);
            println!(
                "scan on a pristine store: {} GETs, {:.0} rows/GET",
                counters.gets(),
                rows as f64 / counters.gets().max(1) as f64
            );
            // Closed, not merely dropped. Two writers on one path fence each
            // other, and leaving this one holding the lease made every later
            // measurement fail with `WriterFenced` — which is the storage
            // layer being right and this file being careless.
            store.backend().close().await.unwrap();
        }

        // --- reopen, so the data is in object storage and not a memtable ---
        let backend = SlateStore::open_s3(path.clone(), server.config())
            .await
            .expect("reopen");
        let catalog = Catalog::from_tables([events()]).expect("catalog");
        let security = SecurityCatalog::new().grant(Grant::new("r", EVENTS, Action::ALL));
        let mut store = RecordStore::new(backend, catalog, security);
        let root = SecurityContext::superuser();

        // --- cold: before anything has read the whole table ----------------
        //
        // Order matters and this is the whole reason the section exists.
        // `cost_calibration` runs `analyze` first, which reads every row, so
        // its numbers are all warm. Point reads go first here, then the scan,
        // then `analyze`, then both again.
        const PROBES: u64 = 200;
        let mut walk = Walk(0x2545_F491_4F6C_DD1D);
        counters.reset();
        let mut found = 0usize;
        for _ in 0..PROBES {
            let id = walk.next(rows);
            let query = Query::all().filter(Expr::eq(col("id"), Value::U64(id)));
            let txn = store.begin().await.unwrap();
            found += txn
                .execute(&root, &events(), &query)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap()
                .len();
        }
        let cold_point_gets = counters.gets() as f64 / PROBES as f64;
        assert_eq!(found as u64, PROBES, "every probe should find its row");
        println!(
            "cold point reads:  {PROBES} probes, {} GETs, {cold_point_gets:.2} per read",
            counters.gets()
        );

        // Three scans back to back, all reported.
        //
        // This file measured a *warm* scan costing more than a cold one, which
        // no cache can do, so the first thing to establish was whether the
        // count is even stable. It is — 205, 207, 207 — and repeating the scan
        // does not make it cheaper, which rules out "the first one paid for
        // the cache". Read with the pristine scan above, the three numbers say
        // what the variable is: **how scattered the reads before this one
        // were.** 53 on a store that has read nothing, ~206 after 200 random
        // point reads, ~370 after `analyze` and 400 of them. A partially
        // populated block cache fragments a scan'''s reads; it does not serve
        // them.
        let mut repeats = Vec::new();
        let mut scanned = 0usize;
        let mut cold_scan_seconds = 0.0;
        for attempt in 0..3 {
            counters.reset();
            let started = Instant::now();
            scanned = {
                let txn = store.begin().await.unwrap();
                txn.execute(&root, &events(), &Query::all())
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap()
                    .len()
            };
            if attempt == 0 {
                cold_scan_seconds = started.elapsed().as_secs_f64();
            }
            repeats.push(counters.gets());
        }
        let cold_scan_gets = repeats[0];
        println!("full scan, three times:  {repeats:?} GETs");
        assert_eq!(scanned as u64, rows, "the scan must return every row");
        let cold_scan_rows_per_get = rows as f64 / cold_scan_gets.max(1) as f64;
        println!(
            "scan after probes: {rows} rows, {cold_scan_gets} GETs, \
             {cold_scan_rows_per_get:.0} rows/GET, {cold_scan_seconds:.2}s"
        );

        // --- analyze -------------------------------------------------------
        let started = Instant::now();
        let stats = {
            let txn = store.begin().await.unwrap();
            txn.analyze(&root, &events()).await.unwrap()
        };
        let analyze_seconds = started.elapsed().as_secs_f64();
        let mut all = Statistics::default();
        all.set(EVENTS, stats);
        store.set_statistics(all);
        println!("analyze:           {analyze_seconds:.2}s");

        // --- warm: the state `cost_calibration` measured in -----------------
        counters.reset();
        for _ in 0..PROBES {
            let id = walk.next(rows);
            let query = Query::all().filter(Expr::eq(col("id"), Value::U64(id)));
            let txn = store.begin().await.unwrap();
            let _ = txn
                .execute(&root, &events(), &query)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
        }
        let warm_point_gets = counters.gets() as f64 / PROBES as f64;
        println!(
            "warm point reads:  {PROBES} probes, {} GETs, {warm_point_gets:.2} per read",
            counters.gets()
        );

        counters.reset();
        let started = Instant::now();
        let scan_predicted = {
            let txn = store.begin().await.unwrap();
            txn.explain(&root, &events(), &Query::all())
                .unwrap()
                .estimated_cost
        };
        let warm_scanned = {
            let txn = store.begin().await.unwrap();
            txn.execute(&root, &events(), &Query::all())
                .await
                .unwrap()
                .collect()
                .await
                .unwrap()
                .len()
        };
        let warm_scan_gets = counters.gets();
        let warm_scan_seconds = started.elapsed().as_secs_f64();
        assert_eq!(warm_scanned as u64, rows);
        let warm_scan_rows_per_get = rows as f64 / warm_scan_gets.max(1) as f64;
        println!(
            "scan after analyze:{rows} rows, {warm_scan_gets} GETs, \
             {warm_scan_rows_per_get:.0} rows/GET, {warm_scan_seconds:.2}s, \
             model predicted {scan_predicted:.1}"
        );

        // --- the shapes, predicted against served ---------------------------
        let cases: Vec<(&str, Query)> = vec![
            (
                "point get by primary key",
                Query::all().filter(Expr::eq(col("id"), Value::U64(rows / 3))),
            ),
            (
                "index equality (one bucket)",
                Query::all().filter(Expr::eq(col("bucket"), Value::I64(7))),
            ),
            (
                "narrow key range (0.5%)",
                Query::all().filter(Expr::compare(col("id"), CmpOp::Ge, Value::U64(1_000)).and(
                    Expr::compare(col("id"), CmpOp::Lt, Value::U64(1_000 + rows / 200)),
                )),
            ),
            (
                "wide key range (25%)",
                Query::all().filter(Expr::compare(col("id"), CmpOp::Lt, Value::U64(rows / 4))),
            ),
            ("full scan", Query::all()),
        ];

        println!(
            "\n{:<30} {:>9} {:>11} {:>9} {:>9} {:>12}",
            "query", "rows", "predicted", "GETs", "wall", "GETs / cost"
        );
        println!("{:-<84}", "");
        let mut index_predicted = f64::NAN;
        let mut index_gets = 0u64;
        let mut index_rows = 0usize;
        for (name, query) in cases {
            let predicted = {
                let txn = store.begin().await.unwrap();
                txn.explain(&root, &events(), &query)
                    .unwrap()
                    .estimated_cost
            };
            counters.reset();
            let started = Instant::now();
            let produced = {
                let txn = store.begin().await.unwrap();
                txn.execute(&root, &events(), &query)
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap()
                    .len()
            };
            let wall = started.elapsed().as_secs_f64();
            let gets = counters.gets();
            let ratio = if predicted > 0.0 {
                gets as f64 / predicted
            } else {
                f64::NAN
            };
            println!(
                "{name:<30} {produced:>9} {predicted:>11.2} {gets:>9} {wall:>8.2}s {ratio:>12.2}"
            );
            if name.starts_with("index equality") {
                index_predicted = predicted;
                index_gets = gets;
                index_rows = produced;
            }
        }

        // --- does the disagreement change a decision? -----------------------
        //
        // The one that matters. A constant that has drifted is only a problem
        // if it makes the planner choose the slower plan, so the contested
        // shape is run three ways at every scale.
        println!("\n--- does the model still pick the faster plan? ---");
        let contested = Query::all().filter(Expr::eq(col("bucket"), Value::I64(7)));
        let mut chosen_access = String::new();
        let mut timings: Vec<(String, f64, u64, f64)> = Vec::new();
        for (label, hint) in [
            ("planner's choice", None),
            ("forced table scan", Some(AccessHint::TableScan)),
            ("forced index", Some(AccessHint::Index(IndexId(10)))),
        ] {
            let mut query = contested.clone();
            query.hint = hint;
            let explained = {
                let txn = store.begin().await.unwrap();
                txn.explain(&root, &events(), &query).unwrap()
            };
            counters.reset();
            let started = Instant::now();
            let produced = {
                let txn = store.begin().await.unwrap();
                txn.execute(&root, &events(), &query)
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap()
                    .len()
            };
            let wall = started.elapsed().as_secs_f64();
            let access = format!("{:?}", explained.access)
                .split_whitespace()
                .next()
                .unwrap_or("?")
                .to_owned();
            if label == "planner's choice" {
                chosen_access = access.clone();
            }
            println!(
                "  {label:<20} {access:<16} cost {:>9.2}  {:>7} GETs  {wall:>7.2}s  ({produced} rows)",
                explained.estimated_cost,
                counters.gets(),
            );
            timings.push((
                label.to_owned(),
                wall,
                counters.gets(),
                explained.estimated_cost,
            ));
        }
        let scan_wall = timings
            .iter()
            .find(|(label, ..)| label == "forced table scan")
            .map_or(f64::NAN, |(_, wall, ..)| *wall);
        let index_wall = timings
            .iter()
            .find(|(label, ..)| label == "forced index")
            .map_or(f64::NAN, |(_, wall, ..)| *wall);
        let faster = if scan_wall < index_wall {
            "TableScan"
        } else {
            "Index"
        };
        let verdict = if chosen_access.starts_with(faster) {
            format!("chose {chosen_access}, and {faster} is faster — correct")
        } else {
            format!("chose {chosen_access} but {faster} is faster — WRONG at this scale")
        };
        println!("  => {verdict}");

        store.backend().close().await.unwrap();

        points.push(Point {
            rows,
            load_seconds,
            puts,
            cold_point_gets,
            warm_point_gets,
            cold_scan_rows_per_get,
            warm_scan_rows_per_get,
            analyze_seconds,
            index_predicted,
            index_gets,
            index_rows,
            scan_predicted,
            scan_gets: warm_scan_gets,
            contested: verdict,
        });
    }

    // --- the two constants, across scales ----------------------------------
    println!("\n\n{:=<96}", "");
    println!("== The two calibrated constants, against scale");
    println!("{:=<96}\n", "");
    println!(
        "{:>10} {:>10} {:>9} {:>13} {:>13} {:>12} {:>12} {:>10}",
        "rows",
        "load s",
        "PUTs",
        "rows/GET cold",
        "rows/GET warm",
        "GETs/read c",
        "GETs/read w",
        "analyze s"
    );
    println!("{:-<96}", "");
    for point in &points {
        println!(
            "{:>10} {:>10.1} {:>9} {:>13.0} {:>13.0} {:>12.2} {:>12.2} {:>10.2}",
            point.rows,
            point.load_seconds,
            point.puts,
            point.cold_scan_rows_per_get,
            point.warm_scan_rows_per_get,
            point.cold_point_gets,
            point.warm_point_gets,
            point.analyze_seconds,
        );
    }
    // Derived, for the reason the header above is derived: this line said
    // "3 requests per read" from #269 until #278, in the summary a reader of
    // the run actually reads.
    println!(
        "\nSCAN_ROW_COST says {:.0} rows per request; POINT_READ_COST says \
         {POINT_READ_COST} request(s)\nper read. Both were measured at 200,000 rows, warm.",
        1.0 / SCAN_ROW_COST
    );

    println!(
        "\n{:>10} {:>12} {:>10} {:>10} {:>14} {:>10}",
        "rows", "scan pred.", "scan GETs", "idx rows", "idx predicted", "idx GETs"
    );
    println!("{:-<72}", "");
    for point in &points {
        println!(
            "{:>10} {:>12.1} {:>10} {:>10} {:>14.1} {:>10}",
            point.rows,
            point.scan_predicted,
            point.scan_gets,
            point.index_rows,
            point.index_predicted,
            point.index_gets,
        );
    }

    println!("\nplan choice on the contested shape:");
    for point in &points {
        println!("  {:>10} rows: {}", point.rows, point.contested);
    }
}
