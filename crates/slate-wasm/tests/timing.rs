//! Is the reported time a measurement, or a number?
//!
//! Everything else in this suite would pass against a binding that returned
//! `kernel_ms: 1.0` for every statement. That is the gap these tests exist to
//! close, and it is a genuinely awkward thing to test: you cannot assert a
//! duration without asserting something about a machine.
//!
//! So none of these assert that anything is *fast*. They assert the three
//! properties a clock has and a constant does not:
//!
//! 1. **It is bounded by an independent clock.** `std::time::Instant` around
//!    the whole call is a second measurement of an interval that strictly
//!    contains the timed region, so the reported number can never exceed it.
//!    A constant, or seconds mislabelled as milliseconds, breaks this.
//! 2. **It orders queries by how much work they do.** A point get, a scan of
//!    4,824 rows and a scan of 100,000 differ by orders of magnitude, so their
//!    ordering is robust to a noisy machine in a way a ratio would not be.
//!    A constant breaks this; so does timing the wrong region.
//! 3. **It excludes what is not the kernel.** A `GROUP BY` does everything the
//!    plain `SELECT` over the same access path does, *plus* a hash and a fold
//!    per row, so it can never be the faster of the two — unless the plain one
//!    is being charged for rendering the rows it returns, which the grouping
//!    does not have to render. That ordering is what pins the clock's upper
//!    boundary, and it is machine-independent in a way a ratio is not.
//!
//! What these cannot catch: a clock that is consistently wrong by a constant
//! factor inside the bound. Nothing short of a second implementation would.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value as Json, json};
use slate_wasm::Playground;
use std::io::Read;
use std::time::Instant;

fn loaded() -> Playground {
    let mut playground = Playground::new();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/data/trips.bin.gz");
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    let outcome: Json = serde_json::from_str(&playground.load_trips(&bytes)).unwrap();
    assert_eq!(outcome["ok"], json!(100_000));
    playground
}

/// What one run reported: the kernel's own number, an independent clock around
/// the whole call, and the access path the planner chose.
struct Run {
    kernel: f64,
    outer: f64,
    access: String,
}

fn timed(playground: &Playground, sql: &str) -> Run {
    let outer = Instant::now();
    let raw = playground.sql(sql);
    let outer = outer.elapsed().as_secs_f64() * 1000.0;
    let results: Vec<Json> = serde_json::from_str(&raw).unwrap();
    let last = results.last().unwrap();
    assert!(last["error"].is_null(), "{sql}: {}", last["error"]);
    Run {
        kernel: last["kernelMs"].as_f64().unwrap_or(-1.0),
        outer,
        access: last["plan"]["access"].as_str().unwrap_or("none").to_owned(),
    }
}

/// The best of several runs, to take the machine's word for it as little as
/// possible. A shared CI box will produce outliers; the *floor* of what was
/// observed is the honest reading of "how long does this take here".
fn best(playground: &Playground, sql: &str, runs: usize) -> Run {
    (0..runs)
        .map(|_| timed(playground, sql))
        .min_by(|a, b| a.outer.total_cmp(&b.outer))
        .expect("at least one run")
}

#[test]
fn the_reported_time_never_exceeds_an_independent_clock() {
    let playground = loaded();
    for sql in [
        "SELECT * FROM trips WHERE id = 500",
        "SELECT pickup_zone FROM trips WHERE pickup_zone = 132",
        "SELECT * FROM trips WHERE pickup_zone = 132",
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone",
        "SELECT * FROM trips",
        "INSERT INTO trips VALUES (900001, 7, 1, 1704067200, 600, 2, 5.5, 25.0, 3.0, 31.0, 'cash')",
    ] {
        let Run { kernel, outer, .. } = timed(&playground, sql);
        assert!(kernel >= 0.0, "{sql}: no time reported at all");
        // A tenth of a millisecond of slack: `Instant` and the binding's clock
        // are different sources and the outer one starts marginally later.
        assert!(
            kernel <= outer + 0.1,
            "{sql}: reported {kernel:.3} ms inside a call that took {outer:.3} ms"
        );
    }
}

#[test]
fn the_reported_time_grows_with_the_work() {
    let playground = loaded();
    // Three queries whose costs differ by orders of magnitude: one row, one
    // zone (4,837 rows through an index), and every row in the table. Their
    // *ordering* is what is asserted, because it survives a noisy machine
    // where a ratio would not.
    let point = best(&playground, "SELECT * FROM trips WHERE id = 500", 5).kernel;
    let zone = best(
        &playground,
        "SELECT * FROM trips WHERE pickup_zone = 132",
        5,
    )
    .kernel;
    let scan = best(&playground, "SELECT * FROM trips", 5).kernel;

    assert!(
        point < zone,
        "a point get ({point:.3} ms) should beat a 4,837-row read ({zone:.3} ms)"
    );
    assert!(
        zone < scan,
        "one zone ({zone:.3} ms) should beat the whole table ({scan:.3} ms)"
    );
    // And the gap is not noise: the full scan reads twenty times the rows.
    assert!(
        scan > point * 10.0,
        "scanning 100,000 rows ({scan:.3} ms) is not meaningfully slower than \
         reading one ({point:.3} ms) — is this a constant?"
    );
}

#[test]
fn the_clock_stops_before_the_rows_are_rendered() {
    let playground = loaded();
    // Rendering is hard to isolate by timing, because in this SQL subset every
    // way of returning fewer strings also reads fewer rows — late
    // materialization couples output width to kernel work, so the obvious
    // "narrow vs wide projection" pair differs far more in the kernel than in
    // the rendering, and a mutation that moved `render` back inside the clock
    // sailed through it. This pair does not have that problem.
    //
    // A `GROUP BY` and the plain `SELECT` of its key, over the same access path
    // and the same predicate, read *exactly* the same entries. The grouping
    // then does strictly more work — a hash and a fold per row — and returns
    // one row where the select returns thousands. So on kernel time alone the
    // grouping must be the slower of the two. If the plain select comes out
    // ahead, the only thing it can be paying for is `format!` on the rows it
    // returns, which is not the database.
    for (plain, grouped, rows) in [
        (
            "SELECT pickup_zone FROM trips WHERE pickup_zone = 132",
            "SELECT pickup_zone, count(*) FROM trips WHERE pickup_zone = 132 GROUP BY pickup_zone",
            4_837,
        ),
        (
            "SELECT pickup_zone FROM trips WHERE pickup_zone >= 0",
            "SELECT pickup_zone, count(*) FROM trips WHERE pickup_zone >= 0 GROUP BY pickup_zone",
            100_000,
        ),
    ] {
        let select = best(&playground, plain, 7);
        let group = best(&playground, grouped, 7);

        // Both halves must have run the same plan, or the comparison is
        // between two different amounts of kernel work and proves nothing.
        // This has bitten before and is cheap to rule out.
        assert_eq!(
            select.access, group.access,
            "the pair no longer shares an access path, so the timing below is \
             comparing two different plans"
        );
        assert!(select.kernel > 0.0 && group.kernel > 0.0);

        // Fifteen percent of slack for a shared machine. The mutation this
        // catches is worth 26% on the small pair and 31% on the large one,
        // measured — see the ledger entry.
        assert!(
            select.kernel < group.kernel * 1.15,
            "returning {rows} rows took {:.3} ms but folding the same read into \
             one group took only {:.3} ms — the select is being charged for \
             rendering its output",
            select.kernel,
            group.kernel
        );
    }
}

#[test]
fn a_write_reports_what_the_index_maintenance_cost() {
    let playground = loaded();
    // The number a record layer should most want to show. An insert into
    // `trips` writes the row *and* its `by_pickup_zone` entry, in one
    // transaction, and this is the only place that cost is visible.
    let Run {
        kernel: insert,
        outer,
        ..
    } = timed(
        &playground,
        "INSERT INTO trips VALUES (900002, 9, 1, 1704067200, 600, 2, 5.5, 25.0, 3.0, 31.0, 'cash')",
    );
    assert!(insert > 0.0, "an insert reported no time at all");
    assert!(insert <= outer + 0.1, "{insert} > {outer}");

    // And through the direct entry point, which the JSON path also reports.
    let raw = playground.insert(
        "trips",
        &json!([
            "900003",
            "9",
            "1",
            "1704067200",
            "600",
            "2",
            "5.5",
            "25.0",
            "3.0",
            "31.0",
            "cash"
        ])
        .to_string(),
    );
    let answer: Json = serde_json::from_str(&raw).unwrap();
    assert_eq!(answer["ok"], "inserted", "{answer}");
    assert!(
        answer["ms"].as_f64().is_some_and(|ms| ms >= 0.0),
        "no time on the insert entry point: {answer}"
    );

    let deleted: Json =
        serde_json::from_str(&playground.delete("trips", &json!(["900003"]).to_string())).unwrap();
    assert_eq!(deleted["ok"], "deleted");
    assert!(deleted["ms"].as_f64().is_some(), "{deleted}");
}

#[test]
fn an_update_charges_for_the_read_it_has_to_do() {
    let playground = loaded();
    // `UPDATE ... SET` is a read followed by a whole-row write, and reporting
    // only the write would understate it. The read is a point get on the
    // primary key, so the total must be at least what that costs.
    let point = best(&playground, "SELECT * FROM trips WHERE id = 42", 5).kernel;
    let Run {
        kernel: update,
        outer,
        ..
    } = timed(&playground, "UPDATE trips SET tip = 9.99 WHERE id = 42");
    assert!(
        update >= point,
        "update {update:.3} ms < its own read {point:.3} ms"
    );
    assert!(update <= outer + 0.1, "{update} > {outer}");
}

#[test]
fn a_buffer_reports_a_time_for_every_statement() {
    let playground = loaded();
    let raw = playground.sql(
        "INSERT INTO trips VALUES (900004, 11, 1, 1704067200, 600, 2, 5.5, 25.0, 3.0, 31.0, 'cash');\n\
         SELECT pickup_zone FROM trips WHERE pickup_zone = 11;\n\
         DELETE FROM trips WHERE id = 900004",
    );
    let results: Vec<Json> = serde_json::from_str(&raw).unwrap();
    assert_eq!(results.len(), 3);
    for (i, result) in results.iter().enumerate() {
        assert!(result["error"].is_null(), "statement {}: {result}", i + 1);
        assert!(
            result["kernelMs"].as_f64().is_some_and(|ms| ms >= 0.0),
            "statement {} reported no time: {result}",
            i + 1
        );
    }
}
