//! Every example the workbench offers, run.
//!
//! The sidebar's "Try one" list is the first thing most readers click, and it
//! lives in JavaScript where nothing type-checks it against the grammar. An
//! example that stops parsing — because a column was renamed, or the parser
//! got stricter — would ship silently and greet a reader with a refusal.
//!
//! So the list is read out of `site/workbench.js` here and every query in it
//! is executed. Extracting it with a regex is ugly and is the point: the test
//! reads the file the page ships rather than a copy that could drift from it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value as Json;
use slate_wasm::Playground;
use std::io::Read;
use std::path::PathBuf;

fn site(name: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", "..", "site", name]
        .iter()
        .collect()
}

/// `[["label", "sql"], …]` out of `const EXAMPLES = [...]`.
///
/// The literals are JavaScript, so they are read with a small unescaper rather
/// than a JSON parser: the file uses `\n` and string concatenation, which JSON
/// does not have. Anything this cannot read is a panic, not a skip — a silent
/// zero-example run would pass forever.
fn examples() -> Vec<(String, String)> {
    let source = std::fs::read_to_string(site("workbench.js")).expect("workbench.js");
    let start = source
        .find("const EXAMPLES = [")
        .expect("the examples list");
    let end = source[start..].find("\n];").expect("the list ends") + start;
    let body = &source[start..end];

    let mut out = Vec::new();
    let mut chars = body.chars().peekable();
    let mut strings: Vec<String> = Vec::new();
    // Every string literal in the block, in order. Pairs of them are
    // (label, sql) — except that a long sql is written as several literals
    // joined by `+`, so a literal that follows a `+` continues the previous.
    let mut concatenating = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut value = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => match chars.next() {
                            Some('n') => value.push('\n'),
                            Some('\\') => value.push('\\'),
                            Some('"') => value.push('"'),
                            Some(other) => value.push(other),
                            None => break,
                        },
                        '"' => break,
                        other => value.push(other),
                    }
                }
                if concatenating {
                    if let Some(last) = strings.last_mut() {
                        last.push_str(&value);
                    }
                } else {
                    strings.push(value);
                }
                concatenating = false;
            }
            '+' => concatenating = true,
            c if c.is_whitespace() => {}
            _ => concatenating = false,
        }
    }
    for pair in strings.chunks(2) {
        if let [label, sql] = pair {
            out.push((label.clone(), sql.clone()));
        }
    }
    assert!(out.len() >= 8, "only found {} examples", out.len());
    out
}

fn loaded() -> Playground {
    let mut playground = Playground::new();
    let file = std::fs::File::open(site("data/trips.bin.gz")).expect("the trip file");
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(file)
        .read_to_end(&mut bytes)
        .expect("gzip");
    let outcome: Json = serde_json::from_str(&playground.load_trips(&bytes)).unwrap();
    assert_eq!(outcome["ok"], serde_json::json!(100_000));
    playground
}

#[test]
fn every_example_the_page_offers_runs() {
    let playground = loaded();
    for (label, sql) in examples() {
        let results: Vec<Json> = serde_json::from_str(&playground.sql(&sql)).unwrap();
        assert!(!results.is_empty(), "{label}: nothing ran");
        for (i, result) in results.iter().enumerate() {
            assert!(
                result["error"].is_null(),
                "{label}, statement {}: {}\n{sql}",
                i + 1,
                result["error"]["message"]
            );
        }
        // A write example legitimately returns no rows for its INSERT, but
        // every buffer has to end with something to look at, or the button
        // clears the grid and looks broken.
        let last = results.last().unwrap();
        assert!(
            last["returned"].as_u64().unwrap_or(0) > 0 || last["kind"] == "write",
            "{label}: the last statement returned nothing"
        );
    }
}

#[test]
fn the_kitchen_sink_uses_everything_it_claims_to() {
    let playground = loaded();
    let (_, sql) = examples()
        .into_iter()
        .find(|(label, _)| label.contains("Kitchen sink"))
        .expect("a Kitchen sink example");

    let results: Vec<Json> = serde_json::from_str(&playground.sql(&sql)).unwrap();
    assert_eq!(results.len(), 3, "three statements");

    // One: the grouped join, with conditions on both sides.
    let join = &results[0];
    assert_eq!(join["kind"], "group");
    assert_eq!(join["columns"][0], "borough");
    assert_eq!(join["spec"]["leftWhere"].as_array().unwrap().len(), 1);
    assert_eq!(join["spec"]["rightWhere"].as_array().unwrap().len(), 2);

    // Three: the disjunction. Checked here rather than left to the "every
    // example runs" test, because running is not the claim — the claim in the
    // comment is that this is an `OR`, and a parser that quietly ANDed it
    // would return fewer rows and still run.
    let disjunction = &results[2];
    assert_eq!(
        disjunction["spec"]["anyOf"].as_array().map(Vec::len),
        Some(3),
        "three ORed conditions: {}",
        disjunction["spec"]
    );
    assert!(
        disjunction["spec"]["filters"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "an ORed WHERE fills `anyOf`, not `filters`: {}",
        disjunction["spec"]
    );

    // Two: the monster. Every claim in its comment, checked.
    let sink = &results[1];
    assert_eq!(
        sink["columns"],
        serde_json::json!([
            "pickup_zone",
            "passengers",
            "count(*)",
            "count(passengers)",
            "count(distinct dropoff_zone)",
            "min(fare)",
            "max(tip)",
            "sum(total)",
            "avg(distance)"
        ])
    );
    let spec = &sink["spec"];
    assert_eq!(
        spec["filters"].as_array().unwrap().len(),
        6,
        "six conditions"
    );
    assert_eq!(
        spec["groupBy"].as_array().unwrap().len(),
        2,
        "two group keys"
    );
    assert_eq!(
        spec["aggregates"].as_array().unwrap().len(),
        7,
        "seven aggregates"
    );
    assert_eq!(spec["limit"], 20);
    assert_eq!(spec["offset"], 5);

    // The operators the comment claims: a pattern and a regular expression
    // beside the comparisons.
    let ops: Vec<String> = spec["filters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["op"].as_str().unwrap().to_owned())
        .collect();
    assert!(ops.contains(&"like".to_owned()), "{ops:?}");
    assert!(ops.contains(&"matches".to_owned()), "{ops:?}");

    let rows = sink["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 20, "LIMIT applies to the groups");
    let counts: Vec<u64> = rows
        .iter()
        .map(|r| r[2].as_str().unwrap().parse().unwrap())
        .collect();
    assert!(
        counts.windows(2).all(|w| w[0] >= w[1]),
        "not ordered by count descending: {counts:?}"
    );
    for row in rows {
        let all: u64 = row[2].as_str().unwrap().parse().unwrap();
        let some: u64 = row[3].as_str().unwrap().parse().unwrap();
        assert!(some <= all, "count(passengers) {some} > count(*) {all}");
        let distinct: u64 = row[4].as_str().unwrap().parse().unwrap();
        assert!(distinct <= all, "more distinct dropoffs than trips");
    }
}
