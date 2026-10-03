//! Where does a search stop being worth an inverted index, and does the
//! planner, given per-term counts, put the line there?
//!
//! ```sh
//! cargo run -p slate-slatedb --example term_crossover
//! ```
//!
//! Before `analyze` counted terms, every search was estimated at
//! `TERM_SELECTIVITY` — a thousandth — and a non-covering index is chosen only
//! below about one row in 8,000 (`SCAN_ROW_COST / POINT_READ_COST`), so the
//! planner scanned for every word, including one held by a single row. With
//! counts it should take the index for a rare word and scan for a common one,
//! and the place it switches should be the place the two paths actually cost
//! the same. This file measures both sides of that.
//!
//! The fixture holds a ladder of terms: `k{n}` is in exactly `n` rows,
//! **spread evenly** across the table. Spread rather than clustered on
//! purpose: `ascending_walk` measured a contiguous run at about a twentieth of
//! a GET per row, so a clustered term would flatter the index and move the
//! crossover a long way right. Spread is the case the cost model is written
//! for — one GET per row followed — and the conservative one.
//!
//! For each rung, three numbers from three separately opened stores: the GETs
//! a forced walk of the index makes, the GETs a forced table scan makes, and
//! the GETs the planner's own choice makes. Beside them, what the planner
//! chooses with the counts and what it chose without.
//!
//! **GETs, not wall clock, is the finding**, as in `ascending_walk`: the count
//! of object-store requests does not depend on optimisation. The seconds are
//! this machine's.

// Benchmark code, and meant to panic if an assumption about the fixture
// breaks: a silently short result table would be worse than a stack trace.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

#[path = "../tests/common/s3server.rs"]
mod s3server;

use slate_kernel::plan::{Access, plan_full};
use slate_kernel::stats::{POINT_READ_COST, SCAN_ROW_COST, TableStats};
use slate_kernel::{
    AccessHint, Action, Expr, Grant, Projection, Query, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext, Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_slatedb::SlateStore;
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use std::time::Instant;

const DOCS: TableId = TableId(1);
const BY_TEXT: IndexId = IndexId(12);

/// Rows in the fixture. `SCALE_ROWS` overrides it, as in `ascending_walk`.
const ROWS: u64 = 200_000;

/// How many rows hold each rung's term.
///
/// Dense around the predicted crossover — `ROWS * SCAN_ROW_COST /
/// POINT_READ_COST` is 25 at the recorded size — and sparse either side,
/// where the answer is not in doubt and one rung each way shows it.
const LADDER: [u64; 12] = [1, 4, 8, 12, 16, 20, 24, 28, 32, 48, 64, 256];

fn rows() -> u64 {
    std::env::var("SCALE_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS)
}

fn docs() -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("text", ValueType::Str)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_text", BY_TEXT).column("text").text())
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    docs().ordinal_of(name).expect("column exists")
}

/// Whether row `id` holds rung `n`'s term: every `rows / n`-th row, offset by
/// the rung so two rungs' rows are not the same rows. At a smoke size the
/// stride can fall below the offset, which is why the offset is reduced
/// modulo it — the defect `ascending_walk`'s `mark` records.
fn holds(id: u64, n: u64) -> bool {
    let stride = (rows() / n).max(1);
    id % stride == n % stride && id / stride < n
}

/// One row: filler words every row has, which is the common end of the
/// vocabulary, plus whichever rung terms land here. The body is the padding
/// `ascending_walk` and `cost_calibration` use, so a block holds about as
/// many rows here as there, and the three files' GET counts read together.
fn row(id: u64) -> Row {
    let mut words = String::from("lorem ipsum dolor sit amet consectetur");
    for n in LADDER {
        if holds(id, n) {
            words.push_str(&format!(" k{n}"));
        }
    }
    Row::new(vec![
        Value::U64(id),
        Value::Str(words),
        Value::Str(format!(
            "body for row {id}, padded out to a realistic width"
        )),
    ])
}

fn search(n: u64) -> Expr {
    Expr::contains(col("text"), &format!("k{n}"))
}

/// What the planner chooses for rung `n` under `stats`.
fn verdict(stats: &TableStats, n: u64) -> &'static str {
    let plan = plan_full(
        &docs(),
        Arc::new(search(n)),
        ScanOrder::Ascending,
        &Projection::All,
        stats,
        None,
        &[],
    );
    match plan.access {
        Access::IndexScan { .. } => "index",
        Access::TableScan { .. } => "scan",
        other => panic!("unexpected access: {other:?}"),
    }
}

#[tokio::main]
async fn main() {
    slate_slatedb::announce();
    let server = s3server::LocalS3::start("slate-orm").await;
    let counters = server.counters();
    let path = "/term-crossover";
    let total = rows();
    let root = SecurityContext::superuser();

    println!("# Where a search stops being worth an inverted index\n");
    println!(
        "Fixture: {total} rows over a real S3 server in this process. Term `k<n>`\n\
         is in exactly n rows, spread evenly. Every arm opens its own store.\n"
    );

    {
        let backend = SlateStore::open_s3(path, server.config())
            .await
            .expect("open");
        let store = RecordStore::new(
            backend.clone(),
            Catalog::from_tables([docs()]).unwrap(),
            SecurityCatalog::new(),
        );
        let load = Instant::now();
        for start in (0..total).step_by(1_000) {
            let batch: Vec<Row> = (start..(start + 1_000).min(total)).map(row).collect();
            let txn = store.begin().await.unwrap();
            txn.insert_many(&root, &docs(), &batch).await.unwrap();
            txn.commit().await.unwrap();
        }
        backend.close().await.unwrap();
        println!("  loaded in {:.1}s\n", load.elapsed().as_secs_f64());
    }

    let security = || SecurityCatalog::new().grant(Grant::new("r", DOCS, Action::ALL));

    // Analysed once and handed to every arm's store. Statistics are plain
    // data; analysing per arm would cost a full scan each and change nothing.
    let counted = {
        let backend = SlateStore::open_s3(path, server.config())
            .await
            .expect("reopen");
        let store = RecordStore::new(backend, Catalog::from_tables([docs()]).unwrap(), security());
        let txn = store.begin().await.unwrap();
        txn.analyze(&root, &docs()).await.unwrap()
    };
    // What the planner had before terms were counted. Nothing else `analyze`
    // records bears on a `contains` — its estimate was the row count times
    // `TERM_SELECTIVITY` and nothing more — so the row count alone is that
    // planner's whole input here.
    let uncounted = TableStats::with_row_count(counted.row_count);

    // The arm's GETs and the rows it returned, from a store opened for it
    // alone. A fresh store is the whole reason `ascending_walk`'s first
    // version was wrong: over one open store the block cache answers the
    // second arm from what the first pulled in.
    let measure = |hint: Option<AccessHint>, n: u64| {
        let stats = counted.clone();
        let config = server.config();
        let counters = counters.clone();
        let root = root.clone();
        async move {
            let backend = SlateStore::open_s3(path, config).await.expect("reopen");
            let mut store =
                RecordStore::new(backend, Catalog::from_tables([docs()]).unwrap(), security());
            store.set_statistics(Statistics::new().with(DOCS, stats));
            let mut query = Query::all().filter(search(n));
            query.hint = hint;
            counters.reset();
            let started = Instant::now();
            let got = {
                let txn = store.begin().await.unwrap();
                txn.execute(&root, &docs(), &query)
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap()
                    .len() as u64
            };
            (counters.gets(), got, started.elapsed().as_secs_f64())
        }
    };

    println!(
        "{:>5} {:>10} {:>10} {:>10}   {:>8} {:>8}  {:>6}",
        "rows", "index GETs", "scan GETs", "chosen", "without", "with", "wall"
    );
    println!("{:-<70}", "");

    let mut rungs = Vec::new();
    for n in LADDER {
        let (index, got_index, _) = measure(Some(AccessHint::Index(BY_TEXT)), n).await;
        let (scan, got_scan, wall) = measure(Some(AccessHint::TableScan), n).await;
        let (chosen, got_chosen, _) = measure(None, n).await;
        // Every arm must return the rung's rows, or the comparison is between
        // two different amounts of work under one heading.
        for (arm, got) in [
            ("index", got_index),
            ("scan", got_scan),
            ("chosen", got_chosen),
        ] {
            assert_eq!(got, n, "rung {n}: the {arm} arm returned {got} rows");
        }
        let before = verdict(&uncounted, n);
        let after = verdict(&counted, n);
        println!(
            "{n:>5} {index:>10} {scan:>10} {chosen:>10}   {before:>8} {after:>8}  {wall:>5.2}s"
        );
        rungs.push((n, index, scan, chosen, after));
    }

    let predicted = total as f64 * SCAN_ROW_COST / POINT_READ_COST;
    let measured = rungs
        .iter()
        .filter(|(_, index, scan, ..)| index < scan)
        .map(|(n, ..)| *n)
        .max();
    let chosen_up_to = rungs
        .iter()
        .filter(|(.., after)| *after == "index")
        .map(|(n, ..)| *n)
        .max();
    let regret: u64 = rungs
        .iter()
        .map(|(_, index, scan, chosen, _)| chosen.saturating_sub(*index.min(scan)))
        .sum();
    println!("\n--- where the line is ---");
    println!("  the cost constants predict the index wins below {predicted:.0} rows");
    println!("  measured, the index made fewer GETs up to {measured:?} rows");
    println!("  given counts, the planner takes the index up to {chosen_up_to:?} rows");
    println!("  without counts, it takes the index for none of them");
    println!("  GETs the planner's choices spent over the cheaper arm, summed: {regret}");
}
