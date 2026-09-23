//! `OVER` in the SQL front end, and the ordinal arithmetic underneath it.
//!
//! This file used to assert a refusal. The kernel had the operator, the gRPC
//! protocol carried it, the three SDKs could ask for one, and this editor said
//! no — because `QuerySpec`, the shape the parser compiles to, had no window
//! field. It has one now and this file asserts what it does instead.
//!
//! # Where the interesting mistake is
//!
//! A window lands at `columns + compute.len() + i`, and `compute.len()` is not
//! final until the whole statement is read: `ORDER BY round(year)` registers a
//! computed column *after* the select list has been walked. So a window
//! reference handed out while parsing the select list cannot be a real
//! ordinal — it is parked at `WINDOW_SLOT + i` and rewritten at the end.
//! `a_window_shifts_when_a_later_clause_computes_a_column` is that, and it is
//! the one case where getting it wrong reads a neighbouring column instead of
//! failing.
//!
//! # The oracle
//!
//! The numbers are checked against `GROUP BY` through the same playground:
//! `sum(year) OVER (PARTITION BY author_id)` and `SELECT author_id, sum(year)
//! ... GROUP BY author_id` share the accumulator arithmetic and nothing else,
//! so agreeing is evidence rather than a restatement.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value as Json, json};
use slate_wasm::Playground;
use std::collections::BTreeMap;

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

/// Every cell of one column, as text, in the order the rows came back.
fn column(answer: &Json, at: usize) -> Vec<String> {
    answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[at].as_str().unwrap().to_owned())
        .collect()
}

/// Which column the header calls `name`, so a test names a column the way the
/// reader does rather than counting.
fn header(answer: &Json, name: &str) -> usize {
    answer["columns"]
        .as_array()
        .unwrap()
        .iter()
        .position(|h| h == name)
        .unwrap_or_else(|| panic!("no column headed `{name}`: {}", answer["columns"]))
}

/// `books` has five columns, so the first computed or window value is ordinal
/// five. Written out rather than hard-coded, because the fixture grew a
/// `price` column once already and every literal ordinal in the tests moved.
fn width() -> usize {
    slate_wasm::fixture::books().columns().len()
}

#[test]
fn a_row_number_restarts_in_each_partition() {
    let playground = Playground::new();
    let answer = ok(
        &playground,
        "SELECT id, author_id, ROW_NUMBER() OVER (PARTITION BY author_id ORDER BY id) \
         FROM books WHERE author_id IN (1, 2)",
    );

    // Author 1 has five books and author 2 has four, so a result numbered
    // 1..=9 straight through — the mistake a partition exists to prevent —
    // would still be nine rows and would still look like an answer.
    let at = header(&answer, "row_number() over");
    let author = header(&answer, "author_id");
    let mut seen: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (n, who) in column(&answer, at).into_iter().zip(column(&answer, author)) {
        seen.entry(who).or_default().push(n.parse().unwrap());
    }
    for (who, mut numbers) in seen {
        numbers.sort_unstable();
        assert_eq!(
            numbers,
            (1..=numbers.len() as i64).collect::<Vec<_>>(),
            "author {who} numbered wrong"
        );
    }
}

#[test]
fn rank_leaves_the_gap_that_dense_rank_closes() {
    let playground = Playground::new();
    // Authors 2 and 6 both have a book from 1965, which is the only tie the
    // fixture offers and is the whole point: over distinct values the two
    // functions agree everywhere and either could be the other.
    let answer = ok(
        &playground,
        "SELECT year, RANK() OVER (ORDER BY year), DENSE_RANK() OVER (ORDER BY year) \
         FROM books WHERE author_id IN (2, 6) ORDER BY year, id",
    );

    let years: Vec<String> = column(&answer, header(&answer, "year"));
    assert_eq!(
        years,
        [
            "1961", "1965", "1965", "1968", "1971", "1972", "1979", "1983"
        ],
        "the fixture stopped having a tie, which makes this test vacuous"
    );
    assert_eq!(
        column(&answer, header(&answer, "rank() over")),
        ["1", "2", "2", "4", "5", "6", "7", "8"]
    );
    assert_eq!(
        column(&answer, header(&answer, "dense_rank() over")),
        ["1", "2", "2", "3", "4", "5", "6", "7"]
    );
}

#[test]
fn a_running_total_gives_peers_the_same_value() {
    let playground = Playground::new();
    let answer = ok(
        &playground,
        "SELECT year, COUNT(*) OVER (ORDER BY year) FROM books \
         WHERE author_id IN (2, 6) ORDER BY year, id",
    );

    // SQL's default frame with an ORDER BY runs to the end of the current
    // row's *peer group*, so both 1965s see 3. Under ROWS — a one-character
    // difference in the kernel's loop — they would see 2 and 3.
    assert_eq!(
        column(&answer, header(&answer, "count(*) over")),
        ["1", "3", "3", "4", "5", "6", "7", "8"]
    );
}

#[test]
fn a_partition_aggregate_agrees_with_the_same_group_by() {
    let playground = Playground::new();
    let windowed = ok(
        &playground,
        "SELECT author_id, SUM(year) OVER (PARTITION BY author_id) FROM books \
         WHERE author_id IN (1, 2, 3)",
    );
    let grouped = ok(
        &playground,
        "SELECT author_id, SUM(year) FROM books WHERE author_id IN (1, 2, 3) \
         GROUP BY author_id",
    );

    let mut oracle: BTreeMap<String, String> = BTreeMap::new();
    for row in grouped["rows"].as_array().unwrap() {
        oracle.insert(
            row[0].as_str().unwrap().to_owned(),
            row[1].as_str().unwrap().to_owned(),
        );
    }
    assert_eq!(oracle.len(), 3, "{}", grouped["rows"]);

    let author = header(&windowed, "author_id");
    let sum = header(&windowed, "sum(year) over");
    let rows = windowed["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 13, "five books, then four and four");
    for row in rows {
        let who = row[author].as_str().unwrap();
        assert_eq!(
            row[sum].as_str().unwrap(),
            oracle[who],
            "author {who}: the window and GROUP BY disagree"
        );
    }
}

#[test]
fn lag_and_lead_step_through_the_partition() {
    let playground = Playground::new();
    let answer = ok(
        &playground,
        "SELECT id, LAG(year) OVER (PARTITION BY author_id ORDER BY id), \
                    LEAD(year) OVER (PARTITION BY author_id ORDER BY id) \
         FROM books WHERE author_id = 1 ORDER BY id",
    );

    // Author 1's books, by id: 1968, 1969, 1974, 1971, 1990. The first row of
    // the partition has no predecessor and the last no successor, and both
    // come back null rather than as a neighbouring partition's value.
    assert_eq!(
        column(&answer, header(&answer, "lag(year, 1) over")),
        ["null", "1968", "1969", "1974", "1971"]
    );
    assert_eq!(
        column(&answer, header(&answer, "lead(year, 1) over")),
        ["1969", "1974", "1971", "1990", "null"]
    );
}

#[test]
fn a_sort_can_name_a_window_and_names_the_same_one() {
    let playground = Playground::new();
    let answer = ok(
        &playground,
        "SELECT id, ROW_NUMBER() OVER (PARTITION BY author_id ORDER BY id) FROM books \
         WHERE author_id IN (1, 2) \
         ORDER BY ROW_NUMBER() OVER (PARTITION BY author_id ORDER BY id) DESC, id",
    );

    // One window, not two: the same specification written twice is find-or-add
    // on the whole clause, exactly as a computed column is. Two would compute
    // the same numbers twice and sort by the second, with no error anywhere.
    assert_eq!(
        answer["spec"]["window"].as_array().map(Vec::len),
        Some(1),
        "{}",
        answer["spec"]
    );
    let numbers = column(&answer, header(&answer, "row_number() over"));
    assert_eq!(numbers, ["5", "4", "4", "3", "3", "2", "2", "1", "1"]);
}

#[test]
fn a_window_shifts_when_a_later_clause_computes_a_column() {
    let playground = Playground::new();
    // `round(year)` is registered by ORDER BY, which the parser reaches
    // *after* the select list has parked the window. If the window kept the
    // ordinal it was given while parsing the list, it would land on the
    // computed column and the reader would silently get a rounded price under
    // a `row_number() over` header.
    let answer = ok(
        &playground,
        "SELECT id, ROW_NUMBER() OVER (ORDER BY id) FROM books WHERE author_id = 1 \
         ORDER BY round(year)",
    );

    assert_eq!(answer["spec"]["compute"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        answer["spec"]["window"].as_array().map(Vec::len),
        Some(1),
        "{}",
        answer["spec"]
    );
    // The projection carries the shifted ordinal, past the table's columns and
    // past the one computed value.
    assert_eq!(
        answer["spec"]["columns"],
        json!([0, width() + 1]),
        "{}",
        answer["spec"]
    );
    let numbers = column(&answer, header(&answer, "row_number() over"));
    let mut sorted = numbers.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, ["1", "2", "3", "4", "5"], "got {numbers:?}");
}

#[test]
fn a_window_and_a_group_by_is_refused() {
    let playground = Playground::new();
    let message = refused(
        &playground,
        "SELECT author_id, ROW_NUMBER() OVER (ORDER BY author_id) FROM books \
         GROUP BY author_id",
    );
    // Not "add it to GROUP BY", which is what the message for a bare column
    // says and would be advice that cannot be taken.
    assert!(message.contains("one value per input row"), "{message}");
    assert!(message.contains("Keep one"), "{message}");
}

#[test]
fn a_window_over_a_join_is_refused_with_the_kernels_reason() {
    let playground = Playground::new();
    let message = refused(
        &playground,
        "SELECT books.id, ROW_NUMBER() OVER (ORDER BY books.id) \
         FROM books JOIN authors ON books.author_id = authors.id",
    );
    assert!(message.contains("a join has no order"), "{message}");
}

#[test]
fn an_unordered_rank_is_refused_by_the_kernel() {
    let playground = Playground::new();
    // The parser does not check this — `Window::new` does, so there is one
    // statement of the rule. What this asserts is that the refusal survives
    // the trip out rather than being swallowed into an empty result.
    let message = refused(&playground, "SELECT RANK() OVER () FROM books");
    assert!(message.to_uppercase().contains("RANK"), "{message}");
}

#[test]
fn lag_at_offset_zero_is_refused() {
    let playground = Playground::new();
    let message = refused(
        &playground,
        "SELECT LAG(year, 0) OVER (ORDER BY id) FROM books",
    );
    assert!(message.to_uppercase().contains("LAG"), "{message}");
}

#[test]
fn a_running_distinct_count_is_refused_and_a_whole_partition_is_not() {
    let playground = Playground::new();
    let message = refused(
        &playground,
        "SELECT COUNT(DISTINCT year) OVER (ORDER BY id) FROM books WHERE author_id = 1",
    );
    assert!(message.to_lowercase().contains("distinct"), "{message}");

    // Unordered it is one set for the partition and is served: author 1's five
    // books have five different years.
    let answer = ok(
        &playground,
        "SELECT COUNT(DISTINCT year) OVER () FROM books WHERE author_id = 1",
    );
    let counts = column(&answer, header(&answer, "count(distinct year) over"));
    assert_eq!(counts, ["5", "5", "5", "5", "5"]);
}

#[test]
fn a_ranking_function_with_an_argument_is_refused() {
    let playground = Playground::new();
    let message = refused(
        &playground,
        "SELECT ROW_NUMBER(year) OVER (ORDER BY id) FROM books",
    );
    assert!(message.contains("takes no argument"), "{message}");
}

#[test]
fn a_ranking_function_without_an_over_says_what_is_missing() {
    let playground = Playground::new();
    // Without the `OVER` there is no window, and `row_number()` is not a
    // function this grammar has anywhere else. The message says which clause
    // is missing rather than that the name is unknown, because the name is not
    // the mistake.
    let message = refused(&playground, "SELECT ROW_NUMBER() FROM books");
    assert!(message.contains("OVER"), "{message}");
}

#[test]
fn a_call_inside_over_is_refused_by_name() {
    let playground = Playground::new();
    let message = refused(
        &playground,
        "SELECT ROW_NUMBER() OVER (PARTITION BY round(year) ORDER BY id) FROM books",
    );
    // A limit of this parser and not of the spec underneath, and the message
    // says which — a reader who is told "expected `)`" concludes the database
    // cannot partition on a computed value, which is false.
    assert!(message.contains("opens a call"), "{message}");
    assert!(message.contains("computed value"), "{message}");
}

/// A column called `over` is not a window clause.
///
/// `over` is not reserved in this grammar and never was: the clause is
/// recognised by the word appearing where a clause may follow a call, so a
/// bare reference is still a column. Refusing a query because a column is
/// named after a keyword it does not use would be a worse bug than the one
/// this grammar fixes.
#[test]
fn a_bare_word_over_is_left_alone() {
    let playground = Playground::new();
    // No table here has such a column, so this must fail — but on the column,
    // not on the word. Asserting the *message* rather than success is what
    // makes this test possible without changing the demo catalog.
    let message = refused(&playground, "SELECT over FROM books");
    assert!(
        message.contains("no column `over`"),
        "a column reference was read as a window clause: {message}"
    );
}

/// The binding refuses a window beside a grouping too, not only the parser.
///
/// Two statements of one rule, deliberately. The parser's version says which
/// clause to delete and is the one a reader meets; this one catches a spec
/// that never passed through the parser — the panel builds one, and so does a
/// JSON literal. Without it the kernel's `narrowed` would *drop* the window on
/// the way into the grouped read, which is correct there and silent here: the
/// reader asks for both and one of them stops happening.
#[test]
fn the_spec_path_refuses_a_window_beside_a_grouping() {
    let playground = Playground::new();
    let answer: Json = serde_json::from_str(
        &playground.run(
            &json!({
                "table": "books",
                "groupBy": [1],
                "aggregates": [{ "kind": "count", "input": 0, "column": 0 }],
                "window": [{
                    "function": "row_number",
                    "order": [{ "column": 0, "descending": false }],
                }],
            })
            .to_string(),
        ),
    )
    .unwrap();
    let message = answer["error"]
        .as_str()
        .unwrap_or_else(|| panic!("the spec was accepted: {answer}"));
    assert!(message.contains("one value per input row"), "{message}");
}
