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
    // trips, zones, authors, books. The taxi tables come first because the
    // workbench lists them in this order and the real dataset is the point.
    assert_eq!(tables.len(), 4, "{tables:#?}");

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

/// A write, and the index answering for it without reading the row.
///
/// This is the assertion the whole crate exists to support. A query engine
/// over a key-value store can filter rows; a *record layer* keeps the
/// secondary index in step with every write, inside the same transaction, so
/// that a row inserted a moment ago is reachable through the index alone.
///
/// The control is the index-only plan. Asserting merely that the new book
/// comes back would pass against a store with no index at all — the table scan
/// would find it. Requiring `Index Only Scan` means the answer came from the
/// index entry, which exists only if the write maintained it.
#[test]
fn an_inserted_row_is_reachable_through_the_index_without_reading_it() {
    let playground = Playground::new();

    let covering = json!({
        "table": "books",
        "filter": { "column": 1, "op": "eq", "value": "2" },
        "columns": [1],
    });

    let before = run(&playground, covering.clone());
    assert!(
        before["plan"]["indexOnly"].as_bool().expect("indexOnly"),
        "the premise of this test is an index-only plan: {}",
        before["plan"]["display"]
    );
    let was = before["returned"].as_u64().expect("a count");

    let inserted: Json = serde_json::from_str(&playground.insert(
        "books",
        &json!(["9001", "2", "Numbers in the Dark", "1993"]).to_string(),
    ))
    .expect("JSON");
    assert_eq!(inserted["ok"], json!("inserted"), "got {inserted}");

    let after = run(&playground, covering);
    assert!(
        after["plan"]["indexOnly"].as_bool().expect("indexOnly"),
        "still answered from the index alone"
    );
    assert_eq!(
        after["returned"].as_u64().expect("a count"),
        was + 1,
        "the index gained an entry for the new row, inside the write"
    );
}

#[test]
fn a_deleted_row_leaves_no_index_entry_behind() {
    let playground = Playground::new();
    let covering = json!({
        "table": "books",
        "filter": { "column": 1, "op": "eq", "value": "1" },
        "columns": [1],
    });

    let was = run(&playground, covering.clone())["returned"]
        .as_u64()
        .expect("a count");

    let gone: Json =
        serde_json::from_str(&playground.delete("books", &json!(["1"]).to_string())).expect("JSON");
    assert_eq!(gone["ok"], json!("deleted"), "got {gone}");

    let after = run(&playground, covering);
    assert_eq!(
        after["returned"].as_u64().expect("a count"),
        was - 1,
        "a dangling index entry would still be counted by an index-only scan"
    );
}

#[test]
fn an_update_moves_the_index_entry_rather_than_duplicating_it() {
    let playground = Playground::new();
    let under = |author: &str| {
        json!({
            "table": "books",
            "filter": { "column": 1, "op": "eq", "value": author },
            "columns": [1],
        })
    };

    let first = run(&playground, under("1"))["returned"]
        .as_u64()
        .expect("a count");
    let second = run(&playground, under("2"))["returned"]
        .as_u64()
        .expect("a count");

    // Book 1 belongs to author 1; give it to author 2.
    let moved: Json = serde_json::from_str(&playground.update(
        "books",
        &json!(["1", "2", "A Wizard of Earthsea", "1968"]).to_string(),
    ))
    .expect("JSON");
    assert_eq!(moved["ok"], json!("updated"), "got {moved}");

    assert_eq!(
        run(&playground, under("1"))["returned"]
            .as_u64()
            .expect("a count"),
        first - 1,
        "the old entry has to go, or the row stays findable under an author it left"
    );
    assert_eq!(
        run(&playground, under("2"))["returned"]
            .as_u64()
            .expect("a count"),
        second + 1,
        "and the new one has to exist"
    );
}

#[test]
fn a_duplicate_primary_key_is_refused_and_leaves_nothing_behind() {
    let playground = Playground::new();
    let clash: Json = serde_json::from_str(&playground.insert(
        "books",
        &json!(["1", "3", "Another Earthsea", "1970"]).to_string(),
    ))
    .expect("JSON");
    assert!(
        clash["error"]
            .as_str()
            .unwrap_or_default()
            .contains("primary key"),
        "got {clash}"
    );

    // And the refusal did not half-apply: author 3's index entries are
    // unchanged, so no entry was written before the key check refused.
    let under_three = run(
        &playground,
        json!({
            "table": "books",
            "filter": { "column": 1, "op": "eq", "value": "3" },
            "columns": [1],
        }),
    );
    assert_eq!(under_three["returned"].as_u64().expect("a count"), 4);
}

#[test]
fn a_write_with_a_wrong_typed_value_is_refused_by_column_name() {
    let playground = Playground::new();
    let bad: Json = serde_json::from_str(&playground.insert(
        "books",
        &json!(["9002", "not-an-author", "A Title", "1999"]).to_string(),
    ))
    .expect("JSON");
    let message = bad["error"].as_str().expect("a refusal");
    assert!(
        message.contains("author_id"),
        "the refusal should name the column: {message}"
    );
}

#[test]
fn reset_restores_the_fixture() {
    let mut playground = Playground::new();
    let _ = playground.delete("books", &json!(["1"]).to_string());
    playground.reset();
    let answer = run(&playground, json!({ "table": "books" }));
    assert_eq!(
        rows(&answer).len(),
        4824,
        "a fresh database, not the reader's wreckage"
    );
}

/// Two conditions, and the planner splitting them.
///
/// The interesting property is not that `AND` narrows the result — any filter
/// does. It is that the planner treats the conjuncts *differently*: the one on
/// the indexed column can become a scan bound, and the rest stay a residual
/// predicate evaluated per row. The plan reports the residual, so the split is
/// visible rather than inferred.
#[test]
fn several_conditions_are_anded_and_the_plan_names_what_stays_residual() {
    let playground = Playground::new();

    let both = run(
        &playground,
        json!({
            "table": "books",
            "filters": [
                { "column": 1, "op": "eq", "value": "1" },
                { "column": 3, "op": "gt", "value": "1970" },
            ],
        }),
    );

    // Le Guin's books after 1970: The Dispossessed (1974), The Lathe of
    // Heaven (1971), Tehanu (1990). Not Earthsea (1968) or Left Hand (1969).
    let years: Vec<i64> = rows(&both)
        .iter()
        .map(|row| row[3].as_str().expect("a year").parse().expect("a number"))
        .collect();
    assert_eq!(years.len(), 3, "got {years:?}");
    assert!(
        years.iter().all(|y| *y > 1970),
        "every row satisfies both: {years:?}"
    );

    // The control: each condition alone returns more, so the conjunction is
    // doing work rather than one condition being ignored.
    let author_only = run(
        &playground,
        json!({ "table": "books", "filters": [{ "column": 1, "op": "eq", "value": "1" }] }),
    );
    assert_eq!(
        rows(&author_only).len(),
        5,
        "author 1 has five books in total"
    );

    let residual = both["plan"]["residual"].as_str().expect("a residual");
    assert!(
        !residual.is_empty(),
        "at least one conjunct stays a per-row predicate; the plan should say so"
    );
}

/// The single `filter` form still works, because the panel shipped with it.
#[test]
fn one_filter_and_a_list_of_one_agree() {
    let playground = Playground::new();
    let single = run(
        &playground,
        json!({ "table": "books", "filter": { "column": 1, "op": "eq", "value": "4" } }),
    );
    let listed = run(
        &playground,
        json!({ "table": "books", "filters": [{ "column": 1, "op": "eq", "value": "4" }] }),
    );
    assert_eq!(single["returned"], listed["returned"]);
    assert_eq!(single["plan"]["access"], listed["plan"]["access"]);
}

#[test]
fn a_contradiction_returns_nothing_rather_than_erroring() {
    let playground = Playground::new();
    let answer = run(
        &playground,
        json!({
            "table": "books",
            "filters": [
                { "column": 1, "op": "eq", "value": "1" },
                { "column": 1, "op": "eq", "value": "2" },
            ],
        }),
    );
    assert_eq!(rows(&answer).len(), 0, "no book has two authors");
}

/// A join over the books fixture.
///
/// The spec names its tables now that there are two joins in the database
/// (`trips` joins `zones`), so this fills in the pair these tests are about
/// rather than repeating it in every case.
fn joined(playground: &Playground, mut spec: Json) -> Json {
    let object = spec.as_object_mut().expect("a spec object");
    object.entry("left").or_insert(json!("authors"));
    object.entry("right").or_insert(json!("books"));
    object.entry("leftKey").or_insert(json!(0));
    object.entry("rightKey").or_insert(json!(1));
    let text = playground.join(&spec.to_string());
    let answer: Json = serde_json::from_str(&text).expect("the binding returns JSON");
    assert!(answer.get("error").is_none(), "unexpected error: {answer}");
    answer
}

#[test]
fn a_join_pairs_each_book_with_its_author() {
    let playground = Playground::new();
    let answer = joined(
        &playground,
        json!({ "leftWhere": [{ "column": 0, "op": "eq", "value": "1" }] }),
    );

    let rows = answer["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 5, "author 1 has five books");
    for row in rows {
        // authors.id, authors.name, authors.country, authors.born,
        // then books.id, books.author_id, books.title, books.year.
        assert_eq!(row[0], json!("1"), "every row carries the author asked for");
        assert_eq!(row[5], json!("1"), "and the book's author_id agrees");
        assert_eq!(row[1], json!("Ursula K. Le Guin"));
    }

    assert_eq!(answer["inputs"].as_array().expect("inputs").len(), 2);
    assert!(
        answer["display"].as_str().expect("a plan").contains("Join"),
        "got {}",
        answer["display"]
    );
}

/// Grouping, checked against a fold done here rather than against the kernel.
///
/// An aggregate test that asks the kernel for a count and then asks it again a
/// different way is checking self-consistency. This counts the fixture's own
/// rows in Rust and requires the grouped read to match, which is the shape the
/// kernel's own `aggregate_oracle` uses and the only one that catches a
/// grouping that is wrong in a self-consistent way.
#[test]
fn a_grouped_join_agrees_with_counting_the_fixture_by_hand() {
    use std::collections::BTreeMap;

    let playground = Playground::new();
    let answer = joined(
        &playground,
        json!({
            "leftWhere": [{ "column": 2, "op": "eq", "value": "US" }],
            "groupBy": [2],
            "aggregates": [{ "kind": "count" }],
        }),
    );

    // The oracle: which authors are in the US, then how many books they have.
    let us: std::collections::BTreeSet<u64> = slate_wasm::fixture::author_rows()
        .iter()
        .filter(|row| matches!(row.values().get(2), Some(v) if format!("{v:?}").contains("US")))
        .filter_map(|row| match row.values().first() {
            Some(slate_tuple::Value::U64(id)) => Some(*id),
            _ => None,
        })
        .collect();
    let mut expected: BTreeMap<String, usize> = BTreeMap::new();
    for book in slate_wasm::fixture::book_rows() {
        if let Some(slate_tuple::Value::U64(author)) = book.values().get(1)
            && us.contains(author)
        {
            *expected.entry("US".to_owned()).or_default() += 1;
        }
    }

    let groups = answer["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1, "one country was asked for");
    let counted: usize = groups[0][1]
        .as_str()
        .expect("a count")
        .parse()
        .expect("a number");
    assert_eq!(
        counted, expected["US"],
        "the grouped join disagrees with counting the fixture directly"
    );
}

#[test]
fn a_grouped_join_can_compute_min_and_max_over_the_book_side() {
    let playground = Playground::new();
    let answer = joined(
        &playground,
        json!({
            "leftWhere": [{ "column": 0, "op": "eq", "value": "1" }],
            "groupBy": [1],
            // `input: 1` is the right table, `books`. It used to be implicit
            // and unavoidable: every aggregate's ordinal was shifted past
            // every left column, so an aggregate could only ever read the
            // right side. Now it says which, and the default is the left --
            // which is what caught this test when the default changed, since
            // `authors` column 3 is the birth year and Le Guin was born in
            // 1929 rather than publishing Earthsea then.
            "aggregates": [
                { "kind": "count" },
                { "kind": "min", "input": 1, "column": 3 },
                { "kind": "max", "input": 1, "column": 3 },
            ],
        }),
    );

    let groups = answer["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 1);
    let row = groups[0].as_array().expect("a group");
    assert_eq!(row[0], json!("Ursula K. Le Guin"), "grouped by author name");
    assert_eq!(row[1], json!("5"), "five books");
    // Earthsea 1968 is the earliest, Tehanu 1990 the latest.
    assert_eq!(row[2], json!("1968"), "min(year)");
    assert_eq!(row[3], json!("1990"), "max(year)");
}

/// Grouping changes the plan, not just the shape of the answer.
#[test]
fn grouping_narrows_what_each_input_decodes() {
    let playground = Playground::new();
    let filter = json!([{ "column": 0, "op": "eq", "value": "1" }]);

    let plain = joined(&playground, json!({ "authors": filter.clone() }));
    let grouped = joined(
        &playground,
        json!({ "authors": filter, "groupBy": [0], "aggregates": [{ "kind": "count" }] }),
    );

    let books_plain = plain["inputs"][1]["decodes"].as_array().expect("decodes");
    let books_grouped = grouped["inputs"][1]["decodes"].as_array().expect("decodes");
    assert!(
        books_grouped.len() < books_plain.len(),
        "a count(*) does not need every book column: {books_plain:?} vs {books_grouped:?}"
    );
}

#[test]
fn an_unknown_aggregate_is_refused_by_name() {
    let playground = Playground::new();
    let text = playground.join(
        &json!({
            "left": "authors", "right": "books", "leftKey": 0, "rightKey": 1,
            "groupBy": [0],
            "aggregates": [{ "kind": "median", "column": 3 }],
        })
        .to_string(),
    );
    let answer: Json = serde_json::from_str(&text).expect("JSON");
    assert!(
        answer["error"]
            .as_str()
            .expect("a refusal")
            .contains("median"),
        "got {answer}"
    );
}

/// `SELECT count(*) FROM books` — one group, over every row.
///
/// This was a front-end refusal and nothing else: the kernel has always
/// answered a grouping with no keys, returning a single group whose key is
/// empty. The guard that refused it mistook the usual shape for the only one.
#[test]
fn a_whole_table_aggregate_needs_no_group_by() {
    let playground = Playground::new();
    let answer: Json =
        serde_json::from_str(&playground.sql("SELECT count(*) FROM books")).expect("json");
    let first = &answer.as_array().expect("statements")[0];
    let rows = first["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "one group over the whole table: {first}");
    // The same 4,824 rows `the_fixture_is_seeded_and_readable` counts by hand,
    // which is what makes this a count rather than a shape check. Values come
    // back as strings: the grid renders them and never does arithmetic.
    assert_eq!(
        rows[0].as_array().expect("row")[0],
        json!("4824"),
        "{first}"
    );
    // And the header says what the column is, rather than naming the table's
    // first column over an aggregate.
    assert_eq!(first["columns"], json!(["count(*)"]), "{first}");
    // The spec carries no `groupBy` at all — not an empty one. The workbench
    // example for this says so in its comment, and a claim on the page that
    // nothing checks is how the last one went stale.
    assert!(first["spec"]["groupBy"].is_null(), "{first}");
    assert_eq!(
        first["spec"]["aggregates"].as_array().expect("aggregates").len(),
        1,
        "{first}"
    );
}

/// Several aggregates at once, still with no GROUP BY.
#[test]
fn several_whole_table_aggregates_come_back_in_order() {
    let playground = Playground::new();
    let answer: Json =
        serde_json::from_str(&playground.sql("SELECT count(*), min(year), max(year) FROM books"))
            .expect("json");
    let first = &answer.as_array().expect("statements")[0];
    let rows = first["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "{first}");
    let row = rows[0].as_array().expect("row");
    assert_eq!(row.len(), 3, "{first}");
    assert_eq!(row[0], json!("4824"), "{first}");
    // min before max, in the order they were written — a pair that would read
    // the same either way if the years happened to match.
    let min: i64 = row[1].as_str().expect("min").parse().expect("a year");
    let max: i64 = row[2].as_str().expect("max").parse().expect("a year");
    assert!(min < max, "min should be below max: {first}");
    // One header per value, and the width has to match or the grid mislabels
    // every cell — which is exactly what it did before this commit.
    assert_eq!(
        first["columns"].as_array().expect("columns").len(),
        row.len(),
        "{first}"
    );
}

/// A bare column beside an aggregate is still refused without a GROUP BY.
///
/// The query returns one row over the whole table, and there is no single
/// value for a column to take in it. Lifting the aggregate guard must not lift
/// this one with it.
#[test]
fn a_column_beside_a_whole_table_aggregate_is_refused() {
    let playground = Playground::new();
    let answer: Json =
        serde_json::from_str(&playground.sql("SELECT title, count(*) FROM books")).expect("json");
    let first = &answer.as_array().expect("statements")[0];
    let message = first["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("GROUP BY"), "{first}");
}
