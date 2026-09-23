//! ClickBench against the slate-orm record layer.
//!
//! ```sh
//! curl -O https://datasets.clickhouse.com/hits_compatible/athena_partitioned/hits_0.parquet
//! cargo run --release -p slate-clickbench -- hits_0.parquet
//! ```
//!
//! # What this is, and is not
//!
//! ClickBench is an analytics benchmark: one wide table, forty-three scan-and-
//! aggregate queries, no joins. This record layer is not an analytics engine —
//! it is a row store with a row-at-a-time executor, built for a keyspace on
//! object storage. Running ClickBench against it is not a fair fight and is
//! not meant to be one.
//!
//! It is run because it is *adversarial*: a standard, public, unsympathetic
//! workload nobody here designed for, which is worth far more as a source of
//! findings than another benchmark written by the same person who wrote the
//! engine.
//!
//! # What it is not comparable to
//!
//! Published ClickBench results are 100 million rows on dedicated hardware.
//! This runs one hundredth of that dataset, in memory, in a container. The
//! numbers below say what this engine does on these queries; they say nothing
//! about how it compares to anything with a published score, and lining them
//! up beside one would be dishonest.

// A benchmark, and meant to stop loudly if the fixture is not what it should
// be: a silently wrong result table would be worse than a stack trace.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::too_many_arguments
)]

mod load;
mod queries;
mod schema;

use slate_kernel::{SecurityContext, Statistics};
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() {
    slate_kernel::build::announce();
    let mut args = std::env::args().skip(1);
    let path = args.next().map_or_else(
        || {
            eprintln!("usage: slate-clickbench <hits.parquet> [row limit]");
            std::process::exit(2);
        },
        PathBuf::from,
    );
    let limit: Option<usize> = args.next().and_then(|a| a.parse().ok());

    eprintln!("loading {}", path.display());
    let (store, counters, rows) = load::load(&path, limit).await;
    let table = schema::table();
    let root = SecurityContext::superuser();

    eprintln!("analysing");
    let analyzed = {
        let txn = store.begin().await.expect("begin");
        let started = Instant::now();
        let stats = txn.analyze(&root, &table).await.expect("analyze");
        eprintln!("  analysed in {:.1}s", started.elapsed().as_secs_f64());
        stats
    };
    let store = store.with_statistics(Statistics::new().with(schema::HITS, analyzed));

    println!(
        "\nClickBench over {rows} rows, in memory. Not comparable to published\n\
         ClickBench results, which are 100M rows on dedicated hardware.\n"
    );
    println!(
        "{:>4}  {:>10}  {:>7}  {:>12}  {:>8}  {:<34}  answer",
        "Q", "wall", "rows", "scanned", "reads", "plan"
    );
    println!("{:-<150}", "");

    let mut total = 0.0f64;
    for runnable in queries::runnable() {
        let txn = store.begin().await.expect("begin");
        counters.reset();
        let started = Instant::now();
        let outcome = queries::run(&txn, &root, &table, runnable.number)
            .await
            .expect("run a query");
        let wall = started.elapsed().as_secs_f64();
        total += wall;
        println!(
            "{:>4}  {:>9.2}s  {:>7}  {:>12}  {:>8}  {:<34}  {}",
            runnable.number,
            wall,
            outcome.rows,
            counters.scan_rows(),
            counters.gets(),
            outcome.plan,
            outcome.answer
        );
    }
    println!("{:-<150}", "");
    println!(
        "{:>4}  {total:>9.2}s  ({} of 43 queries)",
        "sum",
        queries::runnable().len()
    );

    if queries::UNSUPPORTED.is_empty() {
        println!("\nNot run: none.");
    } else {
        println!("\nNot run, and what each would need:");
        for missing in queries::UNSUPPORTED {
            println!("  Q{:<3} {}", missing.number, missing.needs);
        }
    }

    println!("\nNotes on how some queries are expressed:");
    for runnable in queries::runnable() {
        if let Some(note) = runnable.note {
            println!("  Q{:<3} {note}", runnable.number);
        }
    }

    println!("\nWhat was run:");
    for runnable in queries::runnable() {
        let sql = runnable.sql;
        println!("  Q{:<3} {sql}", runnable.number);
    }
}
