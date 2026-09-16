//! `IN (SELECT …)`, and the three things next to it that are refused.
//!
//! The implementation is two reads rather than an operator: the inner query
//! runs once, its single column becomes the `values` of an ordinary
//! `Expr::In`, and the planner's existing `IN` handling — point gets on a key,
//! a range on an index, a hash set on a scan — does the rest. Nothing about
//! subqueries reached the kernel, which is why this file tests the binding and
//! the parser and nothing below them.
//!
//! The oracles recompute the answer from `slate_wasm::fixture` rather than
//! naming a row count. The fixture has a generated tail whose size is a
//! constant somebody may reasonably change, and a hard-coded 612 would then be
//! wrong in a way that reads as a subquery bug.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value as Json;
use slate_wasm::{Playground, fixture};
use std::collections::BTreeSet;

fn first(playground: &Playground, sql: &str) -> Json {
    let out: Vec<Json> = serde_json::from_str(&playground.sql(sql)).expect("json");
    out.into_iter().next().expect("one statement")
}

fn answer(playground: &Playground, sql: &str) -> Json {
    let got = first(playground, sql);
    assert!(got["error"].is_null(), "`{sql}` was refused: {got}");
    got
}

fn refusal(playground: &Playground, sql: &str) -> String {
    let got = first(playground, sql);
    got["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("`{sql}` was not refused: {got}"))
        .to_owned()
}

/// The titles a query came back with.
///
/// Ordinal 2, not 0. A projection does not narrow the row here: `SELECT title`
/// asks the kernel to decode only that column, and the row still arrives four
/// wide with the undecoded ones null. Reading `row[0]` gets `id`, which is a
/// string of digits and compares perfectly happily against nothing.
fn titles(got: &Json) -> BTreeSet<String> {
    got["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row[2].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The `values` the binding filled in, as the strings the spec carries.
///
/// An empty list is *absent* rather than `[]`: `FilterSpec` skips it when it is
/// empty, so a subquery that matched nothing serialises to an `in` filter with
/// no `values` key. It deserialises back to an empty `Vec` either way, and the
/// wire is the same; this helper just has to not read the absence as a bug.
fn values(got: &Json) -> Vec<String> {
    let raw = &got["spec"]["filters"][0]["values"];
    assert!(
        raw.is_array() || raw.is_null(),
        "values is neither a list nor absent: {got}"
    );
    raw.as_array()
        .map(|list| {
            list.iter()
                .map(|v| v.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

// --- the oracle -------------------------------------------------------------

/// The ids of authors from one country, computed from the fixture.
fn author_ids_from(country: &str) -> BTreeSet<u64> {
    fixture::author_rows()
        .iter()
        .filter(|row| matches!(&row.values()[2], slate_tuple::Value::Str(c) if c == country))
        .map(|row| match row.values()[0] {
            slate_tuple::Value::U64(id) => id,
            ref other => panic!("authors.id is {other:?}"),
        })
        .collect()
}

/// The titles of books by any of those authors.
fn titles_by(authors: &BTreeSet<u64>) -> BTreeSet<String> {
    fixture::book_rows()
        .iter()
        .filter(|row| match row.values()[1] {
            slate_tuple::Value::U64(id) => authors.contains(&id),
            ref other => panic!("books.author_id is {other:?}"),
        })
        .map(|row| match &row.values()[2] {
            slate_tuple::Value::Str(t) => t.clone(),
            other => panic!("books.title is {other:?}"),
        })
        .collect()
}

// --- what it answers --------------------------------------------------------

/// The whole point, against an independent computation of the same set.
///
/// Not a count: a count would pass against a build that returned the right
/// number of the wrong books, and "the right number of wrong rows" is exactly
/// what an off-by-one in the candidate list produces.
#[test]
fn a_subquery_answers_what_the_fixture_says_it_should() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE country = 'US')",
    );

    let expected = titles_by(&author_ids_from("US"));
    assert!(
        expected.len() > 10,
        "a handful of rows would not distinguish much"
    );
    assert_eq!(titles(&got), expected);
    assert_eq!(
        got["returned"].as_u64().expect("returned"),
        expected.len() as u64,
        "a title came back twice: {got}"
    );
}

/// And it is not simply every row: the filter has to exclude something, or the
/// test above would pass against a build that ignored the subquery entirely.
#[test]
fn a_subquery_excludes_the_rows_it_should() {
    let playground = Playground::new();
    let narrowed = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE country = 'US')",
    );
    let everything = answer(&playground, "SELECT title FROM books");

    let narrowed = narrowed["returned"].as_u64().expect("returned");
    let everything = everything["returned"].as_u64().expect("returned");
    assert!(narrowed > 0, "the subquery matched nothing");
    assert!(
        narrowed < everything,
        "{narrowed} of {everything} — the subquery narrowed nothing"
    );
}

/// The candidate list is the inner query's answer, exactly.
#[test]
fn the_candidates_are_the_inner_query_s_own_rows() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE country = 'IT')",
    );

    let expected: BTreeSet<String> = author_ids_from("IT").iter().map(u64::to_string).collect();
    assert_eq!(
        values(&got).into_iter().collect::<BTreeSet<_>>(),
        expected,
        "{got}"
    );
}

/// A literal list still works, and lands in the same place in the spec.
///
/// The two forms share `Expr::In`; this is what says the subquery path did not
/// take the literal one with it.
#[test]
fn a_literal_list_is_the_same_filter_with_no_subquery() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (1, 2)",
    );

    assert_eq!(values(&got), vec!["1".to_owned(), "2".to_owned()], "{got}");
    assert!(
        got["spec"]["filters"][0]["subquery"].is_null(),
        "a literal list invented a subquery: {got}"
    );
    assert_eq!(titles(&got), titles_by(&BTreeSet::from([1, 2])));
}

/// The spec keeps *both*: the subquery as written and the candidates it made.
///
/// This is what the Spec panel shows, and it is the thing worth seeing when the
/// answer is not the expected one — a reader can tell "my subquery matched
/// nothing" from "my outer filter matched nothing" without running it again.
#[test]
fn the_spec_carries_the_subquery_beside_the_values_it_produced() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE country = 'PL')",
    );

    let filter = &got["spec"]["filters"][0];
    assert_eq!(filter["op"], "in", "{got}");
    assert_eq!(filter["subquery"]["table"], "authors", "{got}");
    assert_eq!(
        filter["subquery"]["columns"],
        serde_json::json!([0]),
        "{got}"
    );
    assert_eq!(filter["subquery"]["filters"][0]["column"], 2, "{got}");
    assert!(!values(&got).is_empty(), "{got}");
}

/// A subquery over the *same* table is not special-cased into anything.
#[test]
fn a_subquery_may_name_the_table_it_filters() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT author_id FROM books)",
    );
    // Every book has an author_id, so every book qualifies.
    assert_eq!(
        got["returned"].as_u64().expect("returned"),
        fixture::book_rows().len() as u64,
        "{got}"
    );
}

/// An inner query that matches nothing produces an empty list, not everything.
///
/// The failure this guards is the one a `values.is_empty()` shortcut would
/// introduce: an `IN` over no candidates is false for every row, and a build
/// that treated "no candidates" as "no filter" would return the whole table.
#[test]
fn a_subquery_that_matches_nothing_excludes_every_row() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE country = 'ZZ')",
    );
    assert!(values(&got).is_empty(), "{got}");
    assert_eq!(got["returned"].as_u64().expect("returned"), 0, "{got}");
}

/// By the time the planner sees the query the subquery is gone: what is left
/// is an ordinary `IN`, carrying the candidates as values.
///
/// Asserted on the residual rather than on the access method. The planner is
/// cost-based and on this fixture it costs a table scan with an `InSorted`
/// residual below an index scan over fifty-one point gets — correctly, and
/// asserting `by_author` here would be asserting a costing decision this test
/// is not about. What it *is* about is that the candidates arrived: an empty
/// `InSorted` is what a build that dropped them would produce, and it would
/// return nothing while looking exactly like this.
#[test]
fn the_outer_query_is_planned_as_an_ordinary_in() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE country = 'IT')",
    );
    let residual = got["plan"]["residual"]
        .as_str()
        .unwrap_or_else(|| panic!("no residual: {got}"));
    assert!(residual.starts_with("InSorted"), "{residual}");
    assert!(
        residual.contains("Ordinal(1)"),
        "the filter moved column: {residual}"
    );
    // The candidate the fixture guarantees: Italo Calvino is author 2 and is
    // the one named `IT` row, so a dropped or reordered list shows here.
    assert!(residual.contains("U64(2)"), "{residual}");
}

// --- what it refuses --------------------------------------------------------

/// A type that could never match is refused rather than answered with nothing.
///
/// `title IN (SELECT id …)` is not an error anywhere downstream: the binding
/// renders `1` to text and parses it back as the string `"1"`, which no title
/// equals. Zero rows and no complaint is indistinguishable from a fact about
/// the data, which is the worst answer available.
#[test]
fn a_candidate_type_that_could_never_match_is_refused() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE title IN (SELECT id FROM authors)",
    );
    assert!(why.contains("string"), "{why}");
    assert!(why.contains("u64"), "{why}");
}

/// The other direction, so the check is not reading one operand twice.
#[test]
fn the_type_check_runs_in_both_directions() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT name FROM authors)",
    );
    assert!(why.contains("u64") && why.contains("string"), "{why}");
}

/// Mixed integer widths are *not* refused: they round-trip through text, and
/// refusing them would refuse a query that works.
///
/// `books.author_id` is a `u64` and `authors.born` an `i64`, so a rule of
/// strict type equality would stop this — and it is an ordinary thing to
/// write. Neither query matches anything on this fixture (ids run to the low
/// hundreds and years to the high nineteens), which is fine: what is under
/// test is that the parser let them through and the candidates arrived.
#[test]
fn numeric_widths_may_be_mixed() {
    let playground = Playground::new();
    for sql in [
        "SELECT title FROM books WHERE author_id IN (SELECT born FROM authors WHERE country = 'PL')",
        "SELECT title FROM books WHERE year IN (SELECT id FROM authors WHERE country = 'PL')",
    ] {
        let got = answer(&playground, sql);
        assert!(!values(&got).is_empty(), "{sql}: {got}");
    }
}

/// Every filter is resolved, not just the first one.
///
/// A `for` loop over one element and a loop over all of them are the same code
/// until there are two, and the second subquery is the one a `filters[0]`
/// would drop — leaving an `IN` over no candidates, which matches nothing and
/// returns an empty answer that looks like a fact about the data.
#[test]
fn a_second_subquery_in_the_same_where_is_resolved_too() {
    let playground = Playground::new();
    let got = answer(
        &playground,
        "SELECT title FROM books \
         WHERE author_id IN (SELECT id FROM authors WHERE country = 'US') \
         AND year IN (SELECT born FROM authors WHERE country = 'US')",
    );

    let filters = got["spec"]["filters"].as_array().expect("filters");
    assert_eq!(filters.len(), 2, "{got}");
    for (i, filter) in filters.iter().enumerate() {
        assert!(
            filter["values"].as_array().is_some_and(|v| !v.is_empty()),
            "filter {i} has no candidates: {got}"
        );
    }
    // And the second filter narrows: without it the same query returns the 612
    // books by a US author, so this is not two filters one of which is a no-op
    // that a `filters[0]` bug would leave looking correct.
    let one = answer(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE country = 'US')",
    );
    let both = got["returned"].as_u64().expect("returned");
    let one = one["returned"].as_u64().expect("returned");
    assert!(both > 0, "the pair matched nothing, so this proves nothing");
    assert!(
        both < one,
        "{both} of {one} — the second filter narrowed nothing"
    );
}

#[test]
fn a_correlated_subquery_has_no_column_to_resolve() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id FROM authors WHERE id = books.id)",
    );
    assert!(why.contains("books.id"), "{why}");
}

#[test]
fn a_subquery_returns_one_column() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT id, name FROM authors)",
    );
    assert!(why.contains("one column"), "{why}");
}

#[test]
fn an_aggregate_inside_in_is_one_value_not_a_list() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT count(*) FROM authors)",
    );
    assert!(why.contains("one value rather than a list"), "{why}");
}

/// Each refused inner clause by name, because a message naming the wrong
/// clause is how a reader comes to delete the part that was fine.
#[test]
fn every_clause_the_subquery_cannot_take_is_refused_by_its_own_name() {
    let playground = Playground::new();
    for (clause, named) in [
        ("GROUP BY id", "`GROUP BY`"),
        ("ORDER BY id", "`ORDER BY`"),
        ("LIMIT 3", "`LIMIT`"),
        ("OFFSET 3", "`OFFSET`"),
    ] {
        let sql =
            format!("SELECT title FROM books WHERE author_id IN (SELECT id FROM authors {clause})");
        let why = refusal(&playground, &sql);
        assert!(why.contains(named), "{clause}: {why}");
    }
}

#[test]
fn distinct_inside_in_is_refused_as_redundant() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE author_id IN (SELECT DISTINCT id FROM authors)",
    );
    assert!(why.contains("redundant"), "{why}");
}

// --- EXISTS and UNION -------------------------------------------------------

/// Refused, and the message names the form that works.
///
/// Both halves matter. `EXISTS` reaching the column resolver came back as "no
/// such column: `exists`", which sends a reader to their schema looking for a
/// word they got out of SQL.
#[test]
fn exists_is_refused_by_name_and_points_at_in() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE EXISTS (SELECT id FROM authors WHERE id = 1)",
    );
    assert!(why.starts_with("EXISTS is not supported"), "{why}");
    assert!(why.contains("IN (SELECT"), "{why}");
    assert!(!why.contains("no such column"), "{why}");
}

#[test]
fn not_exists_is_refused_as_an_anti_join() {
    let playground = Playground::new();
    let why = refusal(
        &playground,
        "SELECT title FROM books WHERE NOT EXISTS (SELECT id FROM authors)",
    );
    assert!(why.starts_with("NOT EXISTS is not supported"), "{why}");
    assert!(why.contains("anti-join"), "{why}");
}

/// The three set operators, each named in its own refusal.
#[test]
fn the_set_operators_are_refused_by_name() {
    let playground = Playground::new();
    for word in ["UNION", "INTERSECT", "EXCEPT"] {
        let sql = format!("SELECT title FROM books {word} SELECT name FROM authors");
        let why = refusal(&playground, &sql);
        assert!(
            why.starts_with(&format!("{word} is not supported")),
            "{why}"
        );
        assert!(!why.contains("unexpected"), "{why}");
    }
}

/// Only `UNION` is told about the deduplication, because only `UNION` has a
/// two-statement form that gets everything else right.
#[test]
fn only_union_is_offered_the_two_statement_workaround() {
    let playground = Playground::new();
    let union = refusal(
        &playground,
        "SELECT title FROM books UNION SELECT name FROM authors",
    );
    let except = refusal(
        &playground,
        "SELECT title FROM books EXCEPT SELECT name FROM authors",
    );
    assert!(union.contains("deduplication"), "{union}");
    assert!(!except.contains("deduplication"), "{except}");
}

/// And the workaround is real: two statements do run, and both are answered.
#[test]
fn two_statements_separated_by_a_semicolon_both_run() {
    let playground = Playground::new();
    let out: Vec<Json> = serde_json::from_str(
        &playground.sql("SELECT title FROM books LIMIT 2; SELECT name FROM authors LIMIT 3"),
    )
    .expect("json");
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0]["returned"], 2, "{:?}", out[0]);
    assert_eq!(out[1]["returned"], 3, "{:?}", out[1]);
}

/// A bare unknown word after a statement keeps the old message, so the named
/// refusals above did not become a catch-all.
#[test]
fn an_ordinary_typo_after_a_statement_is_still_unexpected() {
    let playground = Playground::new();
    let why = refusal(&playground, "SELECT title FROM books WERE id = 1");
    assert!(why.contains("unexpected") || why.contains("WERE"), "{why}");
}
