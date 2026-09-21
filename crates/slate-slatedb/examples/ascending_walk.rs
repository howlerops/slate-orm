//! Does an inverted index's walk cost less per row than an ordinary index's?
//!
//! ```sh
//! cargo run -p slate-slatedb --example ascending_walk
//! ```
//!
//! `docs/full-text.md` §6 leaves this open, and states a reason:
//!
//! > `POINT_READ_COST` was calibrated on 400 rows reached through an ordinary
//! > index, whose entries are in *column* order, so the row keys are
//! > scattered. Under one term of an inverted index the primary keys are
//! > ascending, and ascending reads may coalesce into far fewer block fetches.
//!
//! Reading `keys.rs` casts doubt on the premise before any measurement: an
//! index entry is `0x02 <index id> <tenant?> <indexed tuple> <primary key
//! tuple>`, so under **one** indexed value the primary keys already ascend.
//! An equality on an ordinary index and one term of an inverted index produce
//! the same shape of walk. If that is right, the difference the doc reaches
//! for is not there, and what actually varies is *density* — how many table
//! rows separate consecutive matches — which is a property of the predicate
//! and not of the index's kind.
//!
//! Four arms separate the two. Each returns the same number of rows, so GETs
//! per row is comparable across all four:
//!
//! | arm | index | rows matched |
//! | --- | --- | --- |
//! | spread, ordinary | `by_bucket`, equality | every 500th row |
//! | spread, inverted | `by_text`, one term | every 500th row, the same ones |
//! | dense, ordinary | `by_cluster`, equality | one contiguous run |
//! | dense, inverted | `by_text`, one term | the same contiguous run |
//!
//! If the doc is right, the two inverted arms beat the two ordinary ones. If
//! this file's reading is right, the two *dense* arms beat the two *spread*
//! ones and the index's kind does not matter.
//!
//! **GETs, not wall clock, is the finding**, which is why this runs usefully
//! in a debug build: the count of object-store requests does not depend on
//! optimisation. The seconds printed beside it are this container's and are
//! not comparable to anything in `docs/performance.md`.

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
    AccessHint, Action, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext,
    Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_slatedb::SlateStore;
use slate_tuple::{Value, ValueType};
use std::time::Instant;

const DOCS: TableId = TableId(1);
const BY_BUCKET: IndexId = IndexId(10);
const BY_CLUSTER: IndexId = IndexId(11);
const BY_TEXT: IndexId = IndexId(12);

/// Rows in the fixture.
///
/// `HEADBENCH_ROWS` overrides it, for the same reason the head-node benchmarks
/// take one: a smoke run needs to prove this still executes, and at a small
/// size every arm fits in one block and the answer is four zeroes.
const ROWS: u64 = 200_000;

/// Rows each arm returns. The spread arm takes every `ROWS / MATCHES`-th row;
/// the dense arm takes the first `MATCHES`.
const MATCHES: u64 = 400;

fn rows() -> u64 {
    std::env::var("HEADBENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS)
}

fn stride() -> u64 {
    (rows() / MATCHES).max(1)
}

fn docs() -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("bucket", ValueType::I64)
        .column("cluster", ValueType::I64)
        .column("text", ValueType::Str)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_bucket", BY_BUCKET).column("bucket"))
        .index(IndexDef::builder("by_cluster", BY_CLUSTER).column("cluster"))
        .index(IndexDef::builder("by_text", BY_TEXT).column("text").text())
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    docs().ordinal_of(name).expect("column exists")
}

/// One row.
///
/// `bucket` and `cluster` carry the two densities, and `text` carries the same
/// two as *terms* so the inverted arms match exactly the same row sets. The
/// filler words are there so a term is one of several in a realistic document
/// rather than the whole column, which is what makes the tokenizer do work.
fn row(id: u64) -> Row {
    let mut words = String::from("lorem ipsum dolor sit amet consectetur");
    if id % stride() == 7 {
        words.push_str(" scatterling");
    }
    if id < MATCHES {
        words.push_str(" clustered");
    }
    Row::new(vec![
        Value::U64(id),
        Value::I64((id % stride()) as i64),
        Value::I64((id / MATCHES) as i64),
        Value::Str(words),
        // Enough body that a row is a realistic size rather than a few bytes,
        // so blocks fill at a rate that resembles real data. The same padding
        // `cost_calibration` uses, so the two files' GET counts are readable
        // against each other.
        Value::Str(format!(
            "body for row {id}, padded out to a realistic width"
        )),
    ])
}

struct Arm {
    label: &'static str,
    query: Query,
    hint: AccessHint,
}

fn arms() -> Vec<Arm> {
    vec![
        Arm {
            label: "spread, ordinary index",
            query: Query::all().filter(Expr::eq(col("bucket"), Value::I64(7))),
            hint: AccessHint::Index(BY_BUCKET),
        },
        Arm {
            label: "spread, inverted index",
            query: Query::all().filter(Expr::contains(col("text"), "scatterling")),
            hint: AccessHint::Index(BY_TEXT),
        },
        Arm {
            label: "dense, ordinary index",
            query: Query::all().filter(Expr::eq(col("cluster"), Value::I64(0))),
            hint: AccessHint::Index(BY_CLUSTER),
        },
        Arm {
            label: "dense, inverted index",
            query: Query::all().filter(Expr::contains(col("text"), "clustered")),
            hint: AccessHint::Index(BY_TEXT),
        },
    ]
}

#[tokio::main]
async fn main() {
    let server = s3server::LocalS3::start("slate-orm").await;
    let counters = server.counters();
    let path = "/ascending-walk";
    let total = rows();

    println!("# Does an inverted index's walk cost less per row?\n");
    println!(
        "Fixture: {total} rows of `docs`, over a real S3 server in this process.\n\
         Each arm returns {MATCHES} rows; the spread arms take every {}th row and\n\
         the dense arms take one contiguous run. GETs is the finding, not seconds.\n",
        stride()
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
        let root = SecurityContext::superuser();
        let load = Instant::now();
        for start in (0..total).step_by(1_000) {
            let batch: Vec<Row> = (start..(start + 1_000).min(total)).map(row).collect();
            let txn = store.begin().await.unwrap();
            txn.insert_many(&root, &docs(), &batch).await.unwrap();
            txn.commit().await.unwrap();
        }
        backend.close().await.unwrap();
        println!(
            "  loaded in {:.1}s, {} puts\n",
            load.elapsed().as_secs_f64(),
            counters.puts()
        );
    }

    let root = SecurityContext::superuser();

    println!(
        "{:<26} {:>6} {:>8} {:>12} {:>9}",
        "arm", "rows", "GETs", "GETs per row", "wall"
    );
    println!("{:-<66}", "");

    let mut seen = Vec::new();
    for arm in arms() {
        // A *fresh* store per arm, and this is the whole reason the first
        // version of this file was wrong. Run over one open store, the spread
        // ordinary arm made 427 GETs and the three arms after it made 1, 6 and
        // 3 — which reads as a spectacular win for the inverted index and is
        // nothing but SlateDB's block cache holding what the first arm pulled
        // in. `cache_probe` in `slate-headbench` exists because that cache is
        // real; measuring across it is measuring the order the arms are
        // written in.
        let backend = SlateStore::open_s3(path, server.config())
            .await
            .expect("reopen");
        let security = SecurityCatalog::new().grant(Grant::new("r", DOCS, Action::ALL));
        let mut store =
            RecordStore::new(backend, Catalog::from_tables([docs()]).unwrap(), security);
        let stats = {
            let txn = store.begin().await.unwrap();
            txn.analyze(&root, &docs()).await.unwrap()
        };
        let mut all = Statistics::default();
        all.set(DOCS, stats);
        store.set_statistics(all);

        let mut query = arm.query;
        query.hint = Some(arm.hint);
        // After `analyze`, which itself reads the table: reset here so the
        // count is the arm's reads and not the statistics pass's.
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
                .len()
        };
        let wall = started.elapsed();
        let gets = counters.gets();
        println!(
            "{:<26} {got:>6} {gets:>8} {:>12.3} {:>8.2}s",
            arm.label,
            gets as f64 / got.max(1) as f64,
            wall.as_secs_f64()
        );
        // Every arm must return the same rows, or the comparison is between
        // two different amounts of work wearing one column heading. This is
        // the assertion that makes the table mean anything.
        assert_eq!(
            got as u64, MATCHES,
            "`{}` returned {got} rows, not {MATCHES}; the fixture and the arm disagree",
            arm.label
        );
        seen.push((arm.label, gets as f64 / got as f64));
    }

    println!("\n--- which variable moved it? ---");
    let per = |label: &str| {
        seen.iter()
            .find(|(name, _)| *name == label)
            .expect("arm ran")
            .1
    };
    let spread_ordinary = per("spread, ordinary index");
    let spread_inverted = per("spread, inverted index");
    let dense_ordinary = per("dense, ordinary index");
    let dense_inverted = per("dense, inverted index");
    println!(
        "  index kind, held at one density:  ordinary {spread_ordinary:.3} vs inverted \
         {spread_inverted:.3} (spread),"
    );
    println!(
        "                                    ordinary {dense_ordinary:.3} vs inverted {dense_inverted:.3} (dense)"
    );
    println!(
        "  density, held at one index kind:  spread {spread_ordinary:.3} vs dense \
         {dense_ordinary:.3} (ordinary),"
    );
    println!(
        "                                    spread {spread_inverted:.3} vs dense {dense_inverted:.3} (inverted)"
    );
    println!(
        "\n  POINT_READ_COST is {}. It is one number for both kinds, and the question\n  \
         this file answers is whether it should be two.",
        slate_kernel::stats::POINT_READ_COST
    );
}
