//! `SELECT DISTINCT`, and the phantom `count(*)` that stood in its way.
//!
//! DISTINCT is not an operator here. The kernel's `Grouping` with keys and no
//! aggregates already yields the distinct combinations in key order —
//! `slate-kernel/tests/aggregate.rs` asserts that directly — so the front end
//! lowers `SELECT DISTINCT a, b` to a grouping on `a, b` and nothing else.
//!
//! What was in the way was a default, not a missing feature. Three places in
//! the binding turned an empty aggregate list into `count(*)`, and `labels`
//! and `joined_labels` produced a matching header. It dated from when this was
//! a panel with a group-by picker and no aggregate picker, where a grouping
//! with nothing to compute had no meaning. The cost of it was visible without
//! DISTINCT: `SELECT author_id FROM books GROUP BY author_id` came back two
//! columns wide, one of which the query does not mention.
//!
//! The oracles here count the fixture rows themselves rather than asserting a
//! number, because the fixture's generated tail can change and a hand-written
//! count would then be wrong in a way that reads as a DISTINCT bug.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value as Json;
use slate_wasm::Playground;
use std::collections::BTreeSet;

/// The first statement of a SQL buffer, as JSON.
fn first(playground: &Playground, sql: &str) -> Json {
    let out: Vec<Json> = serde_json::from_str(&playground.sql(sql)).expect("json");
    out.into_iter().next().expect("one statement")
}

fn refusal(playground: &Playground, sql: &str) -> String {
    let answer = first(playground, sql);
    answer["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("`{sql}` was not refused: {answer}"))
        .to_owned()
}

fn columns(answer: &Json) -> Vec<String> {
    answer["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("no columns: {answer}"))
        .iter()
        .map(|c| c.as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Distinct values of a fixture column, counted independently of the database.
///
/// Debug-formatted rather than unwrapped to a type, so one helper serves every
/// column. The formatting is only ever compared against itself.
fn fixture_distinct(rows: &[slate_schema::Row], column: usize) -> BTreeSet<String> {
    rows.iter()
        .map(|r| format!("{:?}", r.values()[column]))
        .collect()
}

/// The distinct values of a fixture *string* column, as the strings.
fn fixture_strings(rows: &[slate_schema::Row], column: usize) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| match &r.values()[column] {
            slate_tuple::Value::Str(s) => Some(s.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn distinct_returns_each_value_once_and_agrees_with_the_fixture() {
    let playground = Playground::new();
    let answer = first(&playground, "SELECT DISTINCT country FROM authors");

    let expected = fixture_strings(&slate_wasm::fixture::author_rows(), 2);
    assert!(expected.len() > 1, "a one-value column proves nothing");

    // The values, not the count: a count alone would pass if one country came
    // back twice and another not at all.
    let got: BTreeSet<String> = answer["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row[0].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(got, expected, "{answer}");
    // And each exactly once, which the set comparison above cannot see.
    assert_eq!(
        answer["returned"].as_u64().expect("returned"),
        expected.len() as u64,
        "a value came back more than once: {answer}"
    );
}

/// Distinct over two columns is the *combination*, which is the thing a
/// per-column implementation would get wrong.
#[test]
fn distinct_over_two_columns_is_the_combination() {
    let playground = Playground::new();
    let answer = first(&playground, "SELECT DISTINCT author_id, year FROM books");

    let books = slate_wasm::fixture::book_rows();
    let pairs: BTreeSet<String> = books
        .iter()
        .map(|r| format!("{:?}/{:?}", r.values()[1], r.values()[3]))
        .collect();
    let authors = fixture_distinct(&books, 1).len();
    let years = fixture_distinct(&books, 3).len();

    assert_eq!(
        answer["returned"].as_u64().expect("returned"),
        pairs.len() as u64,
        "{answer}"
    );
    // And the combination is not either column's own distinct count, which is
    // what makes the assertion above mean something.
    assert_ne!(pairs.len(), authors, "the fixture cannot tell these apart");
    assert_ne!(pairs.len(), years, "the fixture cannot tell these apart");
    assert_eq!(columns(&answer), vec!["author_id", "year"], "{answer}");
}

/// The lowering itself: a grouping on the selected columns, and no aggregate
/// anywhere. Asserted on the spec rather than on the rows, because a `Distinct`
/// node added later would return the same rows and be a different thing.
#[test]
fn distinct_lowers_to_a_grouping_with_no_aggregates() {
    let playground = Playground::new();
    let answer = first(&playground, "SELECT DISTINCT author_id FROM books");

    assert_eq!(answer["spec"]["groupBy"], serde_json::json!([1]), "{answer}");
    assert!(answer["spec"]["aggregates"].is_null(), "{answer}");
    assert_eq!(answer["kind"], "group", "{answer}");
}

/// `SELECT DISTINCT a, a` is one key written twice.
///
/// Keeping both would group on the pair — the same rows, the column repeated,
/// and no error anywhere, which is the shape of wrong answer this repository
/// keeps finding.
#[test]
fn a_column_named_twice_is_one_key() {
    let playground = Playground::new();
    let answer = first(&playground, "SELECT DISTINCT author_id, author_id FROM books");
    assert_eq!(answer["spec"]["groupBy"], serde_json::json!([1]), "{answer}");
    assert_eq!(columns(&answer), vec!["author_id"], "{answer}");
}

/// The same dedup on a join, which has its own copy of the loop.
///
/// A mutation proved this is not covered by the single-table case: deleting
/// the join path's `contains` check left every test green, because every other
/// join test names each column once. That is the whole argument for running
/// the mutation rather than reasoning about which paths are "the same".
#[test]
fn a_column_named_twice_on_a_join_is_one_key() {
    let playground = Playground::new();
    let answer = first(
        &playground,
        "SELECT DISTINCT country, country FROM authors \
         JOIN books ON authors.id = books.author_id",
    );
    assert_eq!(columns(&answer), vec!["country"], "{answer}");
    assert_eq!(
        answer["spec"]["groupBy"].as_array().expect("groupBy").len(),
        1,
        "{answer}"
    );
}

/// A computed column is registered once and grouped on, the same find-or-add
/// `GROUP BY hour(...)` uses.
///
/// `year` is a plain integer here and not a timestamp, so the *values* this
/// produces mean nothing — the assertion is about the spec, which is what the
/// test is for.
#[test]
fn distinct_over_a_computed_column_registers_it_once() {
    let playground = Playground::new();
    let answer = first(&playground, "SELECT DISTINCT year(year) FROM books");
    let compute = answer["spec"]["compute"].as_array().expect("compute");
    assert_eq!(compute.len(), 1, "{answer}");
    // Ordinal 4 is the first computed column: books has four columns.
    assert_eq!(answer["spec"]["groupBy"], serde_json::json!([4]), "{answer}");
}

/// WHERE filters the rows going in; the distinct keys come from what survives.
#[test]
fn where_narrows_the_rows_before_they_are_deduplicated() {
    let playground = Playground::new();
    let all = first(&playground, "SELECT DISTINCT author_id FROM books");
    let some = first(
        &playground,
        "SELECT DISTINCT author_id FROM books WHERE year > 2000",
    );
    let (all, some) = (
        all["returned"].as_u64().expect("returned"),
        some["returned"].as_u64().expect("returned"),
    );
    // Strictly fewer, or the filter is not doing anything and this test would
    // pass against a build that ignored WHERE entirely.
    assert!(some < all, "{some} distinct authors of {all} after a filter");
    assert!(some > 0, "the filter left nothing, so this proves nothing");
}

/// Over a join, the keys are resolved in the joined space.
#[test]
fn distinct_on_a_join_groups_in_the_joined_space() {
    let playground = Playground::new();
    let answer = first(
        &playground,
        "SELECT DISTINCT country FROM authors JOIN books ON authors.id = books.author_id",
    );
    let expected = fixture_distinct(&slate_wasm::fixture::author_rows(), 2);
    // Every country that has at least one book. The fixture gives every author
    // books, so this is every country — and the assertion is the same number
    // the single-table query returns, which is the point: the join must not
    // multiply the keys by the books.
    assert_eq!(
        answer["returned"].as_u64().expect("returned"),
        expected.len() as u64,
        "{answer}"
    );
    assert_eq!(columns(&answer), vec!["country"], "{answer}");
}

// --- the regression the default was hiding --------------------------------

/// `SELECT a FROM t GROUP BY a` returns the key and nothing else.
///
/// It used to return the key *and* a `count(*)` the query never mentions. Not
/// a formatting detail: the header and the row both grew a column, so a caller
/// reading `row[1]` got a number that is not in the query.
#[test]
fn a_grouping_with_no_aggregates_returns_only_the_keys() {
    let playground = Playground::new();
    let answer = first(&playground, "SELECT author_id FROM books GROUP BY author_id");
    assert_eq!(columns(&answer), vec!["author_id"], "{answer}");
    for row in answer["rows"].as_array().expect("rows") {
        assert_eq!(row.as_array().expect("row").len(), 1, "{answer}");
    }
}

/// And asking for the count still gets it, so the fix removed a default rather
/// than a capability.
#[test]
fn asking_for_the_count_still_gets_it() {
    let playground = Playground::new();
    let answer = first(
        &playground,
        "SELECT author_id, count(*) FROM books GROUP BY author_id",
    );
    assert_eq!(columns(&answer), vec!["author_id", "count(*)"], "{answer}");
}

// --- refusals -------------------------------------------------------------

#[test]
fn distinct_star_is_refused_with_the_reason() {
    let playground = Playground::new();
    let message = refusal(&playground, "SELECT DISTINCT * FROM books");
    assert!(message.contains("primary key"), "{message}");
}

#[test]
fn distinct_beside_group_by_is_refused() {
    let playground = Playground::new();
    let message = refusal(
        &playground,
        "SELECT DISTINCT author_id FROM books GROUP BY author_id",
    );
    assert!(message.contains("twice"), "{message}");
}

#[test]
fn distinct_over_an_aggregate_is_refused_and_points_at_count_distinct() {
    let playground = Playground::new();
    let message = refusal(&playground, "SELECT DISTINCT count(*) FROM books");
    assert!(message.contains("count(distinct"), "{message}");
}

#[test]
fn distinct_beside_group_by_on_a_join_is_refused_too() {
    let playground = Playground::new();
    let message = refusal(
        &playground,
        "SELECT DISTINCT country FROM authors JOIN books ON authors.id = books.author_id \
         GROUP BY country",
    );
    assert!(message.contains("twice"), "{message}");
}

#[test]
fn distinct_over_an_aggregate_on_a_join_is_refused() {
    let playground = Playground::new();
    let message = refusal(
        &playground,
        "SELECT DISTINCT count(*) FROM authors JOIN books ON authors.id = books.author_id",
    );
    assert!(message.contains("count(distinct"), "{message}");
}
