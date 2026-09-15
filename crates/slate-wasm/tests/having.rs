//! `HAVING`: filtering groups, and the type hazard underneath it.
//!
//! The kernel has had `Grouping::having` all along; the parser had no way to
//! reach it, which made `HAVING` the one construct missing from the grammar.
//! Wiring it up is small. What is not small is the literal's *type*, and that
//! is what most of this file is about.
//!
//! `Value` orders by class before magnitude, and `F64` ranks above `I64` and
//! `U64` (which share a rank and compare through `i128`). So a float compared
//! against an integer is decided by the ranks and never looks at the numbers:
//! `F64(0.5) > I64(1000000)` is **true**. A `HAVING` whose literal is typed
//! from the table column rather than from the aggregate therefore returns
//! every group, silently, with no error anywhere.
//!
//! `avg` is always a double even over integer columns, and `sum` is a double
//! only over a real one, so the aggregate — not the column — is the only thing
//! that knows. Two tests below exist purely to pin that down.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value as Json, json};
use slate_wasm::Playground;
use std::io::Read;

fn loaded() -> Playground {
    let mut playground = Playground::new();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/data/trips.bin.gz");
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(file)
        .read_to_end(&mut bytes)
        .unwrap();
    let outcome: Json = serde_json::from_str(&playground.load_trips(&bytes)).unwrap();
    assert_eq!(outcome["ok"], json!(100_000), "{outcome}");
    playground
}

fn run(playground: &Playground, text: &str) -> Json {
    let all: Vec<Json> = serde_json::from_str(&playground.sql(text)).unwrap();
    all.last().expect("a result").clone()
}

fn ok(playground: &Playground, text: &str) -> Json {
    let last = run(playground, text);
    assert!(last["error"].is_null(), "{text}: {}", last["error"]);
    last
}

fn refused(playground: &Playground, text: &str) -> String {
    let last = run(playground, text);
    last["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("{text} was accepted: {last}"))
        .to_owned()
}

/// Every group, for the comparisons below to be a fraction of.
fn all_zones(playground: &Playground) -> u64 {
    ok(
        playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone",
    )["returned"]
        .as_u64()
        .unwrap()
}

#[test]
fn it_keeps_only_the_groups_that_pass() {
    let playground = loaded();
    let all = all_zones(&playground);
    assert_eq!(all, 226, "the fixture's zone count changed");

    let filtered = ok(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone HAVING count(*) > 3000",
    );
    assert_eq!(filtered["returned"], json!(9), "{filtered}");

    // And the survivors really do pass: the smallest count returned is over
    // the threshold. Asserting only the row count would pass against a HAVING
    // that kept nine arbitrary groups.
    let smallest = filtered["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[1].as_str().unwrap().parse::<u64>().unwrap())
        .min()
        .unwrap();
    assert!(smallest > 3000, "a group with {smallest} trips survived");
}

#[test]
fn an_integer_literal_against_a_double_aggregate_is_not_a_free_pass() {
    let playground = loaded();
    let all = all_zones(&playground);

    // `duration` is an `I64` column and `avg` returns `F64` regardless, so
    // this is the pair where the column's type and the aggregate's disagree —
    // and it is the only shape that catches the bug. Typing `1500` from the
    // column gives `I64(1500)`, and `F64(anything) > I64(1500)` is true by
    // class rank, so all 226 zones come back with no error anywhere.
    //
    // Written first against `avg(total)`, which proved nothing: `total` is
    // already an `F64`, so both readings of the type agree and the mutation
    // sailed through. The disagreement is the test.
    let bare = ok(
        &playground,
        "SELECT pickup_zone, avg(duration) FROM trips GROUP BY pickup_zone \
         HAVING avg(duration) > 1500",
    );
    assert_eq!(
        bare["returned"],
        json!(110),
        "an integer literal against avg() over an integer column admitted {} of {all} groups — \
         the literal is being typed from the column, not the aggregate",
        bare["returned"]
    );

    // Written as a decimal it must mean exactly the same thing. If these two
    // disagree, the answer depends on how the reader spelled the number.
    let decimal = ok(
        &playground,
        "SELECT pickup_zone, avg(duration) FROM trips GROUP BY pickup_zone \
         HAVING avg(duration) > 1500.0",
    );
    assert_eq!(
        decimal["rows"], bare["rows"],
        "`1500` and `1500.0` disagree"
    );

    // And the same over a column that really is a double, where the two
    // readings agree and the answer must not change either.
    let real = ok(
        &playground,
        "SELECT pickup_zone, avg(total) FROM trips GROUP BY pickup_zone HAVING avg(total) > 100",
    );
    assert_eq!(real["returned"], json!(1), "{real}");
}

#[test]
fn a_sum_over_an_integer_column_stays_an_integer() {
    let playground = loaded();
    // The other side of the same coin. `Total::sum` returns `I64` for an
    // integer column and `F64` only for a real one, so typing every sum as a
    // double would be the same bug pointing the other way: `I64(x) > F64(600)`
    // is *false* by rank for every group, and the answer would be empty rather
    // than complete.
    //
    // `duration` is seconds, so a busy zone's total is in the millions.
    let sums = ok(
        &playground,
        "SELECT pickup_zone, sum(duration) FROM trips GROUP BY pickup_zone \
         HAVING sum(duration) > 3000000",
    );
    let returned = sums["returned"].as_u64().unwrap();
    assert!(
        returned > 0 && returned < 226,
        "sum(duration) > 3000000 matched {returned} of 226 zones, which is not a filter"
    );

    // And over a real column a sum is a double, so an integer literal has to
    // work there too.
    let real = ok(
        &playground,
        "SELECT pickup_zone, sum(total) FROM trips GROUP BY pickup_zone HAVING sum(total) > 100000",
    );
    let returned = real["returned"].as_u64().unwrap();
    assert!(returned > 0 && returned < 226, "sum(total): {returned}");
}

#[test]
fn a_group_key_can_be_filtered_alongside_an_aggregate() {
    let playground = loaded();
    // Group space is `[keys..., aggregates...]`, so a HAVING may name either.
    // The two conjuncts resolve to different halves of that space, which is
    // the thing most likely to be got wrong by an off-by-one.
    let both = ok(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone \
         HAVING count(*) > 3000 AND pickup_zone < 150",
    );
    for row in both["rows"].as_array().unwrap() {
        let zone: u64 = row[0].as_str().unwrap().parse().unwrap();
        let trips: u64 = row[1].as_str().unwrap().parse().unwrap();
        assert!(zone < 150 && trips > 3000, "{row:?} passes neither test");
    }
    assert_eq!(both["returned"], json!(2), "{both}");
}

#[test]
fn having_runs_before_order_by_and_limit() {
    let playground = loaded();
    // The three together, which is the shape of a real question: the busiest
    // zones, in order, capped. If HAVING ran after LIMIT the answer would be
    // the top three of everything filtered down, not the top three of what
    // passed.
    let top = ok(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone \
         HAVING count(*) > 3000 ORDER BY count(*) DESC LIMIT 3",
    );
    let counts: Vec<u64> = top["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[1].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(counts.len(), 3, "{top}");
    assert!(counts.windows(2).all(|w| w[0] >= w[1]), "{counts:?}");
    assert!(counts.iter().all(|c| *c > 3000), "{counts:?}");

    // The same query with a threshold nothing passes returns nothing, rather
    // than the limit's worth of rows.
    let none = ok(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone \
         HAVING count(*) > 99999 ORDER BY count(*) DESC LIMIT 3",
    );
    assert_eq!(none["returned"], json!(0), "{none}");
}

#[test]
fn a_string_key_takes_the_pattern_operators() {
    let playground = loaded();
    // `LIKE` on a group key is free — the operator half of HAVING is the same
    // code WHERE uses, deliberately — and it is worth a test because it is the
    // one path where the literal is *not* parsed into a typed value.
    let cash = ok(
        &playground,
        "SELECT payment, count(*) FROM trips GROUP BY payment HAVING payment like 'c%'",
    );
    for row in cash["rows"].as_array().unwrap() {
        assert!(row[0].as_str().unwrap().starts_with('c'), "{row:?}");
    }
    assert_eq!(cash["returned"], json!(2), "{cash}");
}

#[test]
fn it_refuses_what_it_cannot_answer() {
    let playground = loaded();

    // No grouping: there are no groups to filter, and lowering this onto the
    // rows would answer a different question rather than the one asked.
    let message = refused(&playground, "SELECT * FROM trips HAVING count(*) > 1");
    assert!(message.contains("GROUP BY"), "{message}");
    assert!(message.contains("WHERE"), "unhelpful: {message}");

    // An aggregate the select list does not compute. Computing a second one
    // behind the reader's back would filter on a number that is not in the
    // answer they can see.
    let message = refused(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone HAVING max(tip) > 5",
    );
    assert!(message.contains("does not compute"), "{message}");
    // Named for the clause the reader actually wrote. This said "ORDER BY" at
    // first, because both clauses resolve through one function, and an error
    // naming a clause that is not in the query sends them looking in the
    // wrong place.
    assert!(
        message.contains("HAVING") && !message.contains("ORDER BY"),
        "the error names the wrong clause: {message}"
    );

    // A column that is not a group key. Same rule the select list enforces.
    let message = refused(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone HAVING tip > 5",
    );
    assert!(message.contains("not a group key"), "{message}");
    assert!(message.contains("HAVING"), "{message}");

    // OR, refused in HAVING for the reason it is refused in WHERE.
    let message = refused(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone \
         HAVING count(*) > 1 OR count(*) < 5",
    );
    assert!(message.contains("OR is not supported"), "{message}");
}

#[test]
fn the_spec_shows_the_having_it_ran() {
    let playground = loaded();
    // The Spec tab is the page's claim about what it sent to the kernel. A
    // HAVING missing from it would make that claim false in the one place a
    // reader goes to check.
    let answer = ok(
        &playground,
        "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone HAVING count(*) > 3000",
    );
    let having = &answer["spec"]["having"];
    assert_eq!(
        having.as_array().map(Vec::len),
        Some(1),
        "{}",
        answer["spec"]
    );
    // Ordinal 1: one group key, so the first aggregate sits at position 1.
    assert_eq!(having[0]["column"], json!(1), "{having}");
    assert_eq!(having[0]["op"], json!("gt"), "{having}");
    assert_eq!(having[0]["value"], json!("3000"), "{having}");
}
