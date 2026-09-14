//! The playground, tested as an ordinary Rust crate.
//!
//! Everything here runs on the host. That is deliberate: a browser is a slow,
//! awkward place to assert on behaviour, and none of these properties are
//! about the browser — they are about the kernel answering correctly through
//! the binding layer. The browser only has to load the module, which the
//! site's own check covers.
//!
//! The assertions that matter are the ones about the *plan*. A playground
//! whose rows are right and whose plan is decorative would be worse than no
//! playground, because the plan is the thing it exists to show.

// Same allowances the rest of the suite carries: a test that cannot index or
// unwrap is a test written around the lints rather than around the behaviour,
// and a panic here is a failure report.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value as Json, json};
use slate_wasm::Playground;

fn run(playground: &Playground, spec: Json) -> Json {
    let text = playground.run(&spec.to_string());
    serde_json::from_str(&text).expect("the binding returns JSON")
}

fn rows(answer: &Json) -> &Vec<Json> {
    assert!(answer.get("error").is_none(), "unexpected error: {answer}");
    answer["rows"].as_array().expect("rows is an array")
}

#[test]
fn the_fixture_is_seeded_and_readable() {
    let playground = Playground::new();
    let answer = run(&playground, json!({ "table": "books" }));
    // 24 named books plus the generated tail; the exact number matters less
    // than that every seeded row is readable.
    assert_eq!(rows(&answer).len(), 4824, "every seeded book comes back");

    let authors = run(&playground, json!({ "table": "authors" }));
    assert_eq!(rows(&authors).len(), 406);
}

/// A secondary index does **not** automatically win, and that is the single
/// most useful thing this playground can show.
///
/// The first version of this test asserted the opposite — that filtering on
/// the indexed `author_id` reaches the index — and failed. The planner was
/// right and the test was carrying OLTP intuition that does not survive
/// contact with object storage: `SCAN_ROW_COST` is 0.000125 and
/// `POINT_READ_COST` is 3.0, so one point read costs as much as scanning
/// twenty-four thousand rows. Five matching rows means five point reads, and a
/// full scan of the whole fixture is cheaper by an order of magnitude.
///
/// Which is why the *next* test matters so much: an index that avoids the row
/// reads entirely is a different proposition.
#[test]
fn an_index_scan_does_not_beat_a_table_scan_at_this_size() {
    let playground = Playground::new();

    let indexed = run(
        &playground,
        json!({ "table": "books", "filter": { "column": 1, "op": "eq", "value": "1" } }),
    );
    let access = indexed["plan"]["access"].as_str().expect("an access path");
    assert_eq!(
        access, "Table Scan",
        "a handful of point reads costs more than scanning the table; \
         if this changed, the cost constants changed"
    );
    assert_eq!(
        rows(&indexed).len(),
        5,
        "Le Guin has five books among the named rows"
    );

    // The estimate is still right even though the access path is a scan —
    // the planner knows how many rows come back, it just does not need an
    // index to get them cheaply.
    let estimate = indexed["plan"]["estimatedRows"]
        .as_f64()
        .expect("an estimate");
    assert!(
        (1.0..100.0).contains(&estimate),
        "the filter's selectivity should be estimated small, got {estimate}"
    );
}

/// Projecting down to an indexed column should avoid reading rows entirely.
#[test]
fn a_projection_onto_the_index_is_index_only() {
    let playground = Playground::new();
    let answer = run(
        &playground,
        json!({
            "table": "books",
            "filter": { "column": 1, "op": "eq", "value": "3" },
            "columns": [1],
        }),
    );
    assert!(
        answer["plan"]["indexOnly"]
            .as_bool()
            .expect("indexOnly is a bool"),
        "reading only the indexed column should not touch a row: {}",
        answer["plan"]["display"]
    );

    // And the control: asking for a column the index does not carry has to
    // read the row, so the same filter stops being index-only.
    let wider = run(
        &playground,
        json!({
            "table": "books",
            "filter": { "column": 1, "op": "eq", "value": "3" },
            "columns": [1, 2],
        }),
    );
    assert!(
        !wider["plan"]["indexOnly"]
            .as_bool()
            .expect("indexOnly is a bool"),
        "reading the title cannot be index-only: {}",
        wider["plan"]["display"]
    );

    // And the number that explains the whole thing. Avoiding the row reads is
    // what makes the index worth having here; the previous test shows that an
    // index which still has to fetch rows loses to a plain scan.
    let covering = answer["plan"]["estimatedCost"].as_f64().expect("a cost");
    let fetching = wider["plan"]["estimatedCost"].as_f64().expect("a cost");
    assert!(
        covering < fetching,
        "the covering plan should cost less: {covering} vs {fetching}"
    );
}

#[test]
fn the_plan_describes_the_query_that_ran() {
    let playground = Playground::new();
    let answer = run(
        &playground,
        json!({
            "table": "books",
            "sort": [{ "column": 3, "descending": true }],
            "limit": 3,
        }),
    );
    let returned = answer["returned"].as_u64().expect("a count");
    assert_eq!(
        returned, 3,
        "the limit is applied to the rows, not only to the plan"
    );
    assert!(answer["plan"]["sorts"].as_bool().expect("sorts is a bool"));

    // Newest first: 2019 (Exhalation) leads.
    let first = &rows(&answer)[0];
    assert_eq!(
        first[3],
        json!("2019"),
        "descending by year starts at the newest"
    );
}

#[test]
fn literals_are_parsed_to_the_column_type_rather_than_coerced() {
    let playground = Playground::new();
    // `author_id` is a u64. A non-numeric literal has to be refused, not
    // silently compared as a string — the kernel's value order is type-first,
    // so a `Str` against a `U64` column matches nothing and looks like an
    // empty result rather than a mistake.
    let answer = run(
        &playground,
        json!({ "table": "books", "filter": { "column": 1, "op": "eq", "value": "Le Guin" } }),
    );
    let message = answer["error"]
        .as_str()
        .expect("a refusal, not an empty result set");
    assert!(
        message.contains("whole number"),
        "the refusal should say what was wrong: {message}"
    );
}

#[test]
fn a_bad_regular_expression_is_refused_rather_than_panicking() {
    let playground = Playground::new();
    let answer = run(
        &playground,
        json!({ "table": "books", "filter": { "column": 2, "op": "matches", "value": "a(b" } }),
    );
    let message = answer["error"].as_str().expect("a refusal");
    assert!(message.contains("regular expression"), "got {message}");
}

#[test]
fn a_pattern_filter_answers_and_names_its_residual() {
    let playground = Playground::new();
    let answer = run(
        &playground,
        json!({ "table": "books", "filter": { "column": 2, "op": "like", "value": "The %" } }),
    );
    let titles: Vec<&str> = rows(&answer)
        .iter()
        .map(|row| row[2].as_str().expect("a title"))
        .collect();
    assert!(!titles.is_empty(), "some titles begin with 'The '");
    assert!(
        titles.iter().all(|t| t.starts_with("The ")),
        "every row matches the pattern: {titles:?}"
    );
}

#[test]
fn the_schema_comes_from_the_catalog() {
    let playground = Playground::new();
    let schema: Json = serde_json::from_str(&playground.schema()).expect("schema JSON");
    let tables = schema.as_array().expect("an array of tables");
    assert_eq!(tables.len(), 2);

    let books = tables
        .iter()
        .find(|t| t["name"] == json!("books"))
        .expect("books");
    assert_eq!(books["columns"].as_array().expect("columns").len(), 4);
    // The UI marks indexed columns from this, so it has to be the real one.
    assert_eq!(
        books["indexed"],
        json!([1]),
        "author_id is the indexed column"
    );

    let authors = tables
        .iter()
        .find(|t| t["name"] == json!("authors"))
        .expect("authors");
    assert_eq!(
        authors["indexed"],
        json!([]),
        "authors has no secondary index"
    );
}

#[test]
fn an_unknown_table_is_refused_by_name() {
    let playground = Playground::new();
    let answer = run(&playground, json!({ "table": "sales" }));
    assert!(
        answer["error"]
            .as_str()
            .expect("a refusal")
            .contains("sales")
    );
}
