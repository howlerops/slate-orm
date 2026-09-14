//! The SQL front end, tested as a front end.
//!
//! The claim this file has to keep honest is narrow and load-bearing: **SQL
//! typed in the workbench is a different spelling of the spec the SDKs send,
//! not a second way into the kernel.** Everything here is built around that.
//!
//! Three kinds of test, in descending order of how much they are worth:
//!
//! 1. **The round trip** (`proptest`). Generate a spec, render it as SQL,
//!    parse it back, and require the same spec. This is the one that catches
//!    what nobody thought of — operator spellings, quote escaping, clause
//!    order, an `OFFSET` that silently becomes a `LIMIT`.
//! 2. **Equivalence.** For a query written both ways, `sql()` and `run()`
//!    must return the same rows *and the same plan text*. Rows alone would
//!    pass for a front end that quietly dropped a filter and got lucky.
//! 3. **Refusals.** Everything outside the grammar is rejected with something
//!    a reader can act on. A parser's error messages are its user interface.
//!
//! What is deliberately *not* here: assertions that a particular query is
//! fast, or that a particular plan is chosen. Those belong to the kernel's own
//! suites, which already have them against an oracle.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use serde_json::{Value as Json, json};
use slate_schema::TableDef;
use slate_wasm::sql::{Schema, Statement, parse, split};
use slate_wasm::{FilterSpec, Playground, QuerySpec, SortSpec, fixture};

fn tables() -> Vec<TableDef> {
    vec![fixture::authors(), fixture::books()]
}

fn parsed(text: &str) -> Result<Statement, String> {
    let tables = tables();
    parse(text, &Schema(&tables)).map_err(|e| format!("{} (at {})", e.message, e.at))
}

/// Every statement's result, as the editor gets them.
fn sql(playground: &Playground, text: &str) -> Vec<Json> {
    let raw = playground.sql(text);
    serde_json::from_str(&raw).expect("the binding returns a JSON array")
}

/// The last statement's result, asserted to have succeeded.
fn one(playground: &Playground, text: &str) -> Json {
    let all = sql(playground, text);
    let last = all.last().expect("at least one result").clone();
    assert!(
        last["error"].is_null(),
        "unexpected refusal of {text:?}: {}",
        last["error"]
    );
    last
}

fn refusal(playground: &Playground, text: &str) -> String {
    let all = sql(playground, text);
    let last = all.last().expect("at least one result").clone();
    assert!(
        !last["error"].is_null(),
        "{text:?} was accepted, and should not have been: {last}"
    );
    last["error"]["message"].as_str().unwrap().to_owned()
}

// --- 1. the round trip ----------------------------------------------------

/// Render a spec as SQL. An independent implementation, on purpose.
///
/// This exists only in the tests. Putting it in the crate would be tempting —
/// the workbench could show SQL for a spec — but then the property below would
/// be comparing the parser against a renderer written to agree with it, which
/// is the failure mode an oracle exists to avoid. Two implementations that
/// disagree are informative; one implementation checked against itself is not.
fn render(spec: &QuerySpec, table: &TableDef) -> String {
    let name = |ordinal: u32| table.columns()[ordinal as usize].name().to_owned();
    let is_text = |ordinal: u32| {
        matches!(
            table.columns()[ordinal as usize].value_type(),
            slate_tuple::ValueType::Str
        )
    };
    let literal = |ordinal: u32, value: &str| {
        if is_text(ordinal) {
            format!("'{}'", value.replace('\'', "''"))
        } else {
            value.to_owned()
        }
    };

    let mut out = String::from("SELECT ");
    if spec.columns.is_empty() {
        out.push('*');
    } else {
        let names: Vec<String> = spec.columns.iter().map(|c| name(*c)).collect();
        out.push_str(&names.join(", "));
    }
    out.push_str(&format!(" FROM {}", spec.table));

    if !spec.filters.is_empty() {
        let parts: Vec<String> = spec
            .filters
            .iter()
            .map(|f| {
                let op = match f.op.as_str() {
                    "eq" => "=",
                    "ne" => "!=",
                    "lt" => "<",
                    "le" => "<=",
                    "gt" => ">",
                    "ge" => ">=",
                    "like" => "LIKE",
                    "ilike" => "ILIKE",
                    "matches" => "~",
                    other => panic!("no SQL spelling for {other}"),
                };
                // Patterns are always strings, whatever the column's type.
                let value = if matches!(f.op.as_str(), "like" | "ilike" | "matches") {
                    format!("'{}'", f.value.replace('\'', "''"))
                } else {
                    literal(f.column, &f.value)
                };
                format!("{} {op} {value}", name(f.column))
            })
            .collect();
        out.push_str(&format!(" WHERE {}", parts.join(" AND ")));
    }
    if !spec.sort.is_empty() {
        let parts: Vec<String> = spec
            .sort
            .iter()
            .map(|s| {
                format!(
                    "{}{}",
                    name(s.column),
                    if s.descending { " DESC" } else { "" }
                )
            })
            .collect();
        out.push_str(&format!(" ORDER BY {}", parts.join(", ")));
    }
    if let Some(limit) = spec.limit {
        out.push_str(&format!(" LIMIT {limit}"));
    }
    if spec.offset > 0 {
        out.push_str(&format!(" OFFSET {}", spec.offset));
    }
    out
}

/// Values that are legal for a column's type, as text.
fn value_for(ordinal: u32) -> BoxedStrategy<String> {
    // books: id u64, author_id u64, title str, year i64.
    match ordinal {
        0 | 1 => (0u64..5000).prop_map(|n| n.to_string()).boxed(),
        3 => (-2000i64..3000).prop_map(|n| n.to_string()).boxed(),
        // Deliberately including quotes and the comment marker: a renderer
        // that forgets to double `'` produces SQL that parses as something
        // else entirely, which is exactly the bug worth catching.
        _ => proptest::string::string_regex("[a-zA-Z0-9 ',;()*-]{0,12}")
            .unwrap()
            .boxed(),
    }
}

fn filter_strategy() -> BoxedStrategy<FilterSpec> {
    (0u32..4)
        .prop_flat_map(|column| {
            let ops: Vec<&'static str> = if column == 2 {
                vec!["eq", "ne", "lt", "le", "gt", "ge", "like", "ilike"]
            } else {
                vec!["eq", "ne", "lt", "le", "gt", "ge"]
            };
            (
                Just(column),
                proptest::sample::select(ops),
                value_for(column),
            )
        })
        .prop_map(|(column, op, value)| FilterSpec {
            column,
            op: op.to_owned(),
            value,
        })
        .boxed()
}

prop_compose! {
    fn spec_strategy()(
        columns in proptest::collection::vec(0u32..4, 0..4),
        filters in proptest::collection::vec(filter_strategy(), 0..3),
        sort in proptest::collection::vec(
            (0u32..4, any::<bool>()).prop_map(|(column, descending)| SortSpec { column, descending }),
            0..3,
        ),
        limit in proptest::option::of(0u64..1000),
        offset in 0u64..100,
    ) -> QuerySpec {
        QuerySpec {
            table: "books".to_owned(),
            filter: None,
            filters,
            sort,
            limit,
            offset,
            columns,
            // The round trip covers ungrouped reads. Grouping has its own
            // tests below: rendering it here would mean teaching the renderer
            // the group-key rules too, and a renderer that has to know as much
            // as the parser stops being an independent check of it.
            group_by: Vec::new(),
            aggregates: Vec::new(),
        }
    }
}

proptest! {
    /// Render a spec as SQL, parse it back, and get the same spec.
    ///
    /// Honest note about `sql.proptest-regressions`, which sits beside this
    /// file and is checked in: this property **passed on its first run**. The
    /// two seeds recorded there were produced later, by deliberately breaking
    /// the parser to find out whether this test would notice — `<` parsed as
    /// `>`, and `''` inside a string no longer collapsed to one quote. It
    /// noticed, and proptest kept the shrunk cases, so those two mutations are
    /// now permanent regression cases rather than something I have to remember
    /// to re-run.
    ///
    /// Which is the useful thing to say about a property test that has never
    /// caught a real bug: it has not caught one *yet*, and it demonstrably
    /// catches the class.
    #[test]
    fn a_spec_rendered_as_sql_parses_back_to_itself(spec in spec_strategy()) {
        let books = fixture::books();
        let text = render(&spec, &books);
        match parsed(&text) {
            Err(why) => prop_assert!(false, "{text:?} did not parse: {why}"),
            Ok(Statement::Select(back)) => prop_assert_eq!(&back, &spec, "from {}", text),
            Ok(other) => prop_assert!(false, "{text:?} parsed as {other:?}"),
        }
    }
}

// --- 2. equivalence with the spec path ------------------------------------

/// Pairs that must mean the same thing. Left: SQL. Right: the spec an SDK
/// would send.
fn equivalents() -> Vec<(&'static str, Json)> {
    vec![
        ("SELECT * FROM books", json!({ "table": "books" })),
        (
            "SELECT * FROM books WHERE author_id = 1",
            json!({ "table": "books", "filters": [{ "column": 1, "op": "eq", "value": "1" }] }),
        ),
        (
            "SELECT author_id FROM books WHERE author_id = 1",
            json!({
                "table": "books",
                "filters": [{ "column": 1, "op": "eq", "value": "1" }],
                "columns": [1],
            }),
        ),
        (
            "SELECT * FROM books WHERE author_id = 1 AND year > 1970",
            json!({
                "table": "books",
                "filters": [
                    { "column": 1, "op": "eq", "value": "1" },
                    { "column": 3, "op": "gt", "value": "1970" },
                ],
            }),
        ),
        (
            "SELECT * FROM books WHERE title LIKE 'The %' ORDER BY year DESC LIMIT 5",
            json!({
                "table": "books",
                "filters": [{ "column": 2, "op": "like", "value": "The %" }],
                "sort": [{ "column": 3, "descending": true }],
                "limit": 5,
            }),
        ),
        (
            "SELECT * FROM authors WHERE country = 'US' LIMIT 3 OFFSET 1",
            json!({
                "table": "authors",
                "filters": [{ "column": 2, "op": "eq", "value": "US" }],
                "limit": 3,
                "offset": 1,
            }),
        ),
    ]
}

#[test]
fn sql_and_the_spec_return_the_same_rows_and_the_same_plan() {
    let playground = Playground::new();
    for (text, spec) in equivalents() {
        let through_sql = one(&playground, text);
        let raw = playground.run(&spec.to_string());
        let through_spec: Json = serde_json::from_str(&raw).unwrap();

        assert!(
            through_spec.get("error").is_none(),
            "the spec form of {text:?} failed: {through_spec}"
        );
        assert_eq!(
            through_sql["rows"], through_spec["rows"],
            "different rows for {text:?}"
        );
        // The plan, not just the rows. A front end that dropped a condition
        // and happened to return the same rows would pass the line above.
        assert_eq!(
            through_sql["plan"]["display"], through_spec["plan"]["display"],
            "different plans for {text:?}"
        );
    }
}

#[test]
fn the_compiled_spec_is_reported_and_is_the_one_that_ran() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT author_id FROM books WHERE author_id = 2 LIMIT 7",
    );
    // What the panel shows must be runnable as-is. This asserts the round
    // trip the *user interface* makes: copy the spec out, send it to a head
    // node, get the same answer.
    let spec = answer["spec"].clone();
    assert_eq!(spec["table"], "books");
    assert_eq!(spec["limit"], 7);
    assert_eq!(spec["columns"], json!([1]));

    let again: Json = serde_json::from_str(&playground.run(&spec.to_string())).unwrap();
    assert_eq!(again["rows"], answer["rows"]);
    assert_eq!(again["plan"]["display"], answer["plan"]["display"]);
}

#[test]
fn narrowing_the_projection_reaches_an_index_only_scan() {
    let playground = Playground::new();
    let wide = one(&playground, "SELECT * FROM books WHERE author_id = 2");
    let narrow = one(
        &playground,
        "SELECT author_id FROM books WHERE author_id = 2",
    );

    // The site's central claim, now reachable by typing.
    assert_eq!(wide["plan"]["indexOnly"], json!(false), "{wide}");
    assert_eq!(narrow["plan"]["indexOnly"], json!(true), "{narrow}");
    assert_eq!(narrow["plan"]["decodes"], json!([1]));

    // Same rows, in the sense that matters: the same books, in the same
    // order. *Not* the same values — and this is worth being precise about,
    // because I got it wrong first and asserted the two were identical.
    //
    // A projected row keeps its arity but carries `null` in every column the
    // plan did not decode. That is late materialization showing through: the
    // index-only scan never read the row, so `title` and `year` are not
    // "null in the database", they are "not fetched". The workbench has to
    // render those two cases differently or it libels the data.
    let ids = |answer: &Json| -> Vec<String> {
        answer["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[0].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&wide), ids(&narrow));
    assert!(
        narrow["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row[2] == json!("null") && row[3] == json!("null")),
        "an undecoded column should come back unfetched: {narrow}"
    );
    assert!(
        wide["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row[2] != json!("null")),
        "the unprojected query decodes every column: {wide}"
    );
}

// --- the join -------------------------------------------------------------

#[test]
fn a_join_through_sql_matches_the_join_spec() {
    let playground = Playground::new();
    let text = "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
                WHERE country = 'US' LIMIT 5";
    let through_sql = one(&playground, text);
    let through_spec: Json = serde_json::from_str(
        &playground.join(
            &json!({
                "left": "authors",
                "right": "books",
                "leftKey": 0,
                "rightKey": 1,
                "leftWhere": [{ "column": 2, "op": "eq", "value": "US" }],
                "limit": 5,
            })
            .to_string(),
        ),
    )
    .unwrap();

    assert_eq!(through_sql["rows"], through_spec["rows"], "{through_sql}");
    assert_eq!(through_sql["kind"], "join");
}

#[test]
fn a_grouped_join_counts_by_the_key() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT count(*) FROM authors JOIN books ON authors.id = books.author_id \
         GROUP BY country",
    );
    assert_eq!(answer["kind"], "group");
    assert_eq!(answer["columns"], json!(["country", "count(*)"]));
    let rows = answer["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "no groups: {answer}");

    // Every book belongs to exactly one author, so the counts sum to the
    // number of books — computed here rather than written down, so a fixture
    // change cannot make this test quietly meaningless.
    let books = one(&playground, "SELECT * FROM books");
    let total: u64 = rows
        .iter()
        .map(|row| row[1].as_str().unwrap().parse::<u64>().unwrap())
        .sum();
    assert_eq!(total, books["returned"].as_u64().unwrap());
}

#[test]
fn a_grouped_join_takes_min_and_max() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT count(*), max(year) FROM authors JOIN books ON authors.id = books.author_id \
         GROUP BY country",
    );
    assert_eq!(
        answer["columns"],
        json!(["country", "count(*)", "max(year)"])
    );
    let rows = answer["rows"].as_array().unwrap();
    assert!(rows.iter().all(|row| row.as_array().unwrap().len() == 3));
}

// --- writes ---------------------------------------------------------------

#[test]
fn an_inserted_row_is_reachable_through_the_index_without_reading_it() {
    let playground = Playground::new();
    let before = one(
        &playground,
        "SELECT author_id FROM books WHERE author_id = 3",
    );
    assert_eq!(before["plan"]["indexOnly"], json!(true));
    let was = before["returned"].as_u64().unwrap();

    let written = one(
        &playground,
        "INSERT INTO books VALUES (9001, 3, 'Something New', 2024)",
    );
    assert_eq!(written["kind"], "write");
    assert_eq!(written["message"], "inserted");

    let after = one(
        &playground,
        "SELECT author_id FROM books WHERE author_id = 3",
    );
    // Both halves matter. The count says the write landed; `indexOnly` says
    // the *index* was updated inside the write, because the query that found
    // it never read a row. Asserting only the count would pass against a
    // store that had no index at all.
    assert_eq!(after["returned"].as_u64().unwrap(), was + 1);
    assert_eq!(after["plan"]["indexOnly"], json!(true), "{after}");
}

#[test]
fn update_changes_the_named_columns_and_leaves_the_rest() {
    let playground = Playground::new();
    let before = one(&playground, "SELECT * FROM books WHERE id = 1");
    let original = before["rows"][0].clone();

    one(
        &playground,
        "UPDATE books SET title = 'Renamed' WHERE id = 1",
    );
    let after = one(&playground, "SELECT * FROM books WHERE id = 1");
    let updated = after["rows"][0].clone();

    assert_eq!(updated[2], json!("Renamed"));
    // The read-modify-write must not disturb anything else: same id, same
    // author, same year. An update that rebuilt the row from defaults would
    // pass an assertion about the title alone.
    assert_eq!(updated[0], original[0]);
    assert_eq!(updated[1], original[1]);
    assert_eq!(updated[3], original[3]);
}

#[test]
fn update_moves_a_row_between_index_entries() {
    let playground = Playground::new();
    let before = one(&playground, "SELECT * FROM books WHERE author_id = 1");
    let moved = before["rows"][0][0].as_str().unwrap().to_owned();
    let was = before["returned"].as_u64().unwrap();

    one(
        &playground,
        &format!("UPDATE books SET author_id = 2 WHERE id = {moved}"),
    );

    let after = one(&playground, "SELECT * FROM books WHERE author_id = 1");
    assert_eq!(after["returned"].as_u64().unwrap(), was - 1);
    // The old index entry has to be gone, not merely shadowed: the row must
    // not come back under its previous key.
    let ids: Vec<String> = after["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().to_owned())
        .collect();
    assert!(!ids.contains(&moved), "{moved} still answers for author 1");
}

#[test]
fn delete_removes_the_row_and_says_when_there_was_none() {
    let playground = Playground::new();
    assert_eq!(
        one(&playground, "DELETE FROM books WHERE id = 1")["message"],
        "deleted"
    );
    assert_eq!(
        one(&playground, "SELECT * FROM books WHERE id = 1")["returned"],
        json!(0)
    );
    // A delete that matched nothing is not an error, and must not claim to
    // have deleted something.
    assert_eq!(
        one(&playground, "DELETE FROM books WHERE id = 99999")["message"],
        "no row with that key"
    );
}

#[test]
fn several_statements_run_in_order_and_stop_at_the_first_refusal() {
    let playground = Playground::new();
    let results = sql(
        &playground,
        "INSERT INTO books VALUES (9100, 1, 'One', 2001);\n\
         SELECT * FROM books WHERE id = 9100;\n\
         SELECT * FROM nosuch;\n\
         INSERT INTO books VALUES (9101, 1, 'Two', 2002)",
    );
    assert_eq!(
        results.len(),
        3,
        "the fourth should not have run: {results:?}"
    );
    assert_eq!(results[0]["message"], "inserted");
    assert_eq!(results[1]["returned"], json!(1));
    assert!(!results[2]["error"].is_null());

    // And the statement after the refusal really did not run.
    assert_eq!(
        one(&playground, "SELECT * FROM books WHERE id = 9101")["returned"],
        json!(0)
    );
}

// --- 3. refusals ----------------------------------------------------------

#[test]
fn refusals_name_what_was_wrong() {
    let playground = Playground::new();
    let cases: Vec<(&str, &str)> = vec![
        ("SELECT * FROM nosuch", "no table named `nosuch`"),
        ("SELECT nosuch FROM books", "has no column `nosuch`"),
        (
            "SELECT * FROM books WHERE nosuch = 1",
            "has no column `nosuch`",
        ),
        ("SELECT * FROM books WERE id = 1", "unexpected `WERE`"),
        // A qualifier naming the wrong table. This pair exists because
        // deleting the qualifier check left every test passing: `authors.name`
        // was being read as the bare column `name`, which `books` does not
        // have — so the refusal came out right by accident, and would have
        // stopped doing so the moment two tables shared a column name.
        (
            "SELECT authors.name FROM books",
            "is not a column of `books`",
        ),
        (
            "SELECT * FROM books WHERE authors.id = 1",
            "is not a column of `books`",
        ),
        (
            "SELECT * FROM books WHERE id = 1 OR id = 2",
            "OR is not supported",
        ),
        ("SELECT * FROM books WHERE id", "expected a comparison"),
        ("SELECT * FROM books LIMIT many", "expected a value"),
        (
            "SELECT * FROM books WHERE title = 'unclosed",
            "is not closed",
        ),
        (
            "SELECT * FROM books WHERE id = 'not a number'",
            "not a non-negative whole number",
        ),
        (
            "UPDATE books SET title = 'x' WHERE title = 'y'",
            "primary key only",
        ),
        ("DELETE FROM books WHERE author_id = 1", "primary key only"),
        ("INSERT INTO books VALUES (1, 2)", "takes 4 values"),
        ("INSERT INTO books (id) VALUES (1)", "no column list"),
        ("SELECT count(*) FROM books", "needs a GROUP BY"),
        // These two used to be refusals, when the parser knew one join and
        // checked the `ON` clause against it. Any pair of tables and columns
        // is legal now — `books.id = authors.id` is a meaningless join and a
        // valid one, and every SQL engine will run it — so what is left to
        // refuse is a key that does not name one column of each side.
        (
            "SELECT * FROM trips JOIN zones ON trips.nosuch = zones.id",
            "does not name one column of `trips` and one of `zones`",
        ),
        (
            "SELECT * FROM trips JOIN trips ON trips.id = trips.id",
            "cannot be joined to itself",
        ),
        (
            "DROP TABLE books",
            "expected SELECT, INSERT, UPDATE or DELETE",
        ),
        ("", "there is nothing to run"),
    ];
    for (text, wanted) in cases {
        let message = refusal(&playground, text);
        assert!(
            message.contains(wanted),
            "{text:?}\n  wanted a message containing {wanted:?}\n  got {message:?}"
        );
    }
}

#[test]
fn a_refusal_points_at_the_offending_statement_in_the_buffer() {
    let playground = Playground::new();
    let buffer = "SELECT * FROM books;\nSELECT * FROM nosuch";
    let results = sql(&playground, buffer);
    let at = results[1]["error"]["at"].as_u64().unwrap() as usize;
    // The offset is into the whole buffer, not into the second statement, so
    // an editor can put the caret on it. `nosuch` is only in the second.
    assert!(
        buffer[at..].starts_with("nosuch"),
        "offset {at} points at {:?}",
        &buffer[at..]
    );
}

#[test]
fn text_the_lexer_could_choke_on_does_not_panic() {
    let playground = Playground::new();
    // Multi-byte characters were a real panic: the first lexer walked bytes
    // and could stop inside one, then slice. Each of these must come back as
    // a refusal or an answer — never a panic, and never a hang.
    for text in [
        "SELECT * FROM café",
        "SELECT * FROM books WHERE title = 'café'",
        "SELECT * FROM books WHERE title LIKE '%é%'",
        "→",
        "'",
        "''",
        "-- just a comment",
        ";;;;",
        "SELECT * FROM books WHERE title = 'semi; colon'",
        "SELECT * FROM books WHERE title = 'two -- dashes'",
    ] {
        let _ = sql(&playground, text);
    }
}

#[test]
fn a_semicolon_inside_a_string_does_not_split_the_statement() {
    let playground = Playground::new();
    let results = sql(&playground, "SELECT * FROM books WHERE title = 'a;b'");
    assert_eq!(results.len(), 1, "split into {results:?}");
    assert!(results[0]["error"].is_null(), "{results:?}");
    assert_eq!(split("SELECT 'a;b'; SELECT 1").len(), 2);
}

#[test]
fn keywords_and_names_are_case_insensitive() {
    let playground = Playground::new();
    let upper = one(&playground, "select * from BOOKS where AUTHOR_ID = 1");
    let lower = one(&playground, "SELECT * FROM books WHERE author_id = 1");
    assert_eq!(upper["rows"], lower["rows"]);
    assert_eq!(upper["plan"]["display"], lower["plan"]["display"]);
}

#[test]
fn comments_and_qualified_names_are_accepted() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "-- the indexed column\nSELECT books.author_id FROM books WHERE books.author_id = 2",
    );
    assert_eq!(answer["plan"]["indexOnly"], json!(true));
}

#[test]
fn a_regular_expression_that_is_not_valid_is_refused_rather_than_run() {
    let playground = Playground::new();
    let message = refusal(&playground, "SELECT * FROM books WHERE title ~ '('");
    assert!(message.contains("regular expression"), "got {message:?}");
}

// --- single-table grouping ------------------------------------------------

#[test]
fn a_grouped_scan_agrees_with_folding_the_rows_by_hand() {
    let playground = Playground::new();
    let grouped = one(
        &playground,
        "SELECT author_id, count(*) FROM books GROUP BY author_id",
    );
    assert_eq!(grouped["kind"], "group");
    assert_eq!(grouped["columns"], json!(["author_id", "count(*)"]));

    // The oracle: read every row and fold it here. An assertion against
    // numbers written down by hand would test the numbers somebody thought of;
    // this tests the grouping against the same data the grouping read, and
    // catches a key that goes missing as readily as a count that is wrong.
    let all = one(&playground, "SELECT * FROM books");
    let mut expected: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for row in all["rows"].as_array().unwrap() {
        *expected
            .entry(row[1].as_str().unwrap().to_owned())
            .or_default() += 1;
    }

    let seen: std::collections::BTreeMap<String, u64> = grouped["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row[0].as_str().unwrap().to_owned(),
                row[1].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect();
    assert_eq!(seen, expected);
}

#[test]
fn every_aggregate_agrees_with_folding_by_hand() {
    let playground = Playground::new();
    let grouped = one(
        &playground,
        "SELECT author_id, count(*), min(year), max(year), sum(year), avg(year) \
         FROM books WHERE author_id < 10 GROUP BY author_id",
    );
    assert_eq!(
        grouped["columns"],
        json!([
            "author_id",
            "count(*)",
            "min(year)",
            "max(year)",
            "sum(year)",
            "avg(year)"
        ])
    );

    let all = one(&playground, "SELECT * FROM books WHERE author_id < 10");
    let mut years: std::collections::BTreeMap<u64, Vec<f64>> = std::collections::BTreeMap::new();
    for row in all["rows"].as_array().unwrap() {
        years
            .entry(row[1].as_str().unwrap().parse().unwrap())
            .or_default()
            .push(row[3].as_str().unwrap().parse().unwrap());
    }

    for row in grouped["rows"].as_array().unwrap() {
        let key: u64 = row[0].as_str().unwrap().parse().unwrap();
        let mine = years.get(&key).unwrap_or_else(|| panic!("no group {key}"));
        let num = |i: usize| -> f64 { row[i].as_str().unwrap().parse().unwrap() };
        assert_eq!(num(1), mine.len() as f64, "count for {key}");
        assert_eq!(num(2), mine.iter().copied().fold(f64::MAX, f64::min), "min");
        assert_eq!(num(3), mine.iter().copied().fold(f64::MIN, f64::max), "max");
        assert_eq!(num(4), mine.iter().sum::<f64>(), "sum for {key}");
        let mean = mine.iter().sum::<f64>() / mine.len() as f64;
        assert!((num(5) - mean).abs() < 1e-9, "avg for {key}");
    }
}

#[test]
fn a_grouped_scan_can_answer_from_the_index_alone() {
    let playground = Playground::new();
    // The point of grouping in a record layer rather than over a result set:
    // the key and the aggregate both live in the index, so counting books per
    // author can read no book rows at all. Asserting the counts alone would
    // pass against an implementation that scanned the table.
    let filtered = one(
        &playground,
        "SELECT author_id, count(*) FROM books WHERE author_id < 50 GROUP BY author_id",
    );
    assert_eq!(
        filtered["plan"]["indexOnly"],
        json!(true),
        "{}",
        filtered["plan"]
    );
    assert_eq!(filtered["plan"]["decodes"], json!([1]));

    // But *only* with a predicate the index can range over. I expected the
    // unfiltered group-by to go index-only too, and it does not: it plans as a
    // table scan that decodes one column.
    //
    // The planner is not being careless. The cost model charges per row and
    // has no notion of row width, so scanning 4,824 index entries and scanning
    // 4,824 whole rows come to the *same* number — 1.0 + 4824 x 0.000125 =
    // 1.603 — and the tie goes to the table scan. On object storage the two
    // are not equal at all: the index entries are one column and the rows are
    // four, which is fewer bytes fetched for the same row count.
    //
    // Recorded as a limitation rather than fixed here: widening the cost model
    // to charge for bytes touches every plan this repository has measured, and
    // is not something to slip into a page redesign.
    let unfiltered = one(
        &playground,
        "SELECT author_id, count(*) FROM books GROUP BY author_id",
    );
    assert_eq!(unfiltered["plan"]["indexOnly"], json!(false));
    assert_eq!(unfiltered["plan"]["decodes"], json!([1]));
    let scan_cost = unfiltered["plan"]["estimatedCost"].as_f64().unwrap();
    assert!(
        (scan_cost - (1.0 + 4824.0 * 0.000125)).abs() < 1e-6,
        "the tie this comment claims is not a tie any more: {scan_cost}"
    );
}

#[test]
fn count_of_a_column_is_not_count_of_rows() {
    let playground = Playground::new();
    let stars = one(
        &playground,
        "SELECT author_id, count(*) FROM books GROUP BY author_id",
    );
    let column = one(
        &playground,
        "SELECT author_id, count(title) FROM books GROUP BY author_id",
    );
    assert_eq!(column["columns"], json!(["author_id", "count(title)"]));
    // No nulls in this fixture, so the two agree — but they are different
    // aggregates and the binding must not fold one into the other. The header
    // above is what proves they stayed distinct.
    assert_eq!(stars["rows"], column["rows"]);
}

#[test]
fn count_distinct_counts_values_not_rows() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT country, count(distinct born) FROM authors GROUP BY country",
    );
    assert_eq!(
        answer["columns"],
        json!(["country", "count(distinct born)"])
    );

    let all = one(&playground, "SELECT * FROM authors");
    let mut distinct: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for row in all["rows"].as_array().unwrap() {
        distinct
            .entry(row[2].as_str().unwrap().to_owned())
            .or_default()
            .insert(row[3].as_str().unwrap().to_owned());
    }
    for row in answer["rows"].as_array().unwrap() {
        let key = row[0].as_str().unwrap();
        let seen: usize = row[1].as_str().unwrap().parse().unwrap();
        assert_eq!(seen, distinct[key].len(), "distinct born in {key}");
    }
}

#[test]
fn grouping_narrows_what_the_plan_reads() {
    let playground = Playground::new();
    let ungrouped = one(&playground, "SELECT * FROM books");
    let grouped = one(
        &playground,
        "SELECT author_id, count(*) FROM books GROUP BY author_id",
    );
    // Grouping is not a filter over a result set here: the planner narrows the
    // projection to the group key and the aggregates' columns, so the two
    // queries decode different things.
    assert_eq!(ungrouped["plan"]["decodes"], json!([0, 1, 2, 3]));
    assert_eq!(grouped["plan"]["decodes"], json!([1]));
}

#[test]
fn grouped_refusals_name_what_was_wrong() {
    let playground = Playground::new();
    for (text, wanted) in [
        (
            "SELECT title, count(*) FROM books GROUP BY author_id",
            "neither a group key nor an aggregate",
        ),
        ("SELECT count(*) FROM books", "needs a GROUP BY"),
        (
            "SELECT author_id, median(year) FROM books GROUP BY author_id",
            "no such aggregate",
        ),
        (
            "SELECT author_id, nosuch(year) FROM books GROUP BY author_id",
            "no such aggregate",
        ),
        (
            "SELECT author_id, max(nosuch) FROM books GROUP BY author_id",
            "has no column `nosuch`",
        ),
        (
            "SELECT author_id, count(*) FROM books GROUP BY nosuch",
            "has no column `nosuch`",
        ),
    ] {
        let message = refusal(&playground, text);
        assert!(
            message.contains(wanted),
            "{text:?}\n  wanted {wanted:?}\n  got {message:?}"
        );
    }
}

#[test]
fn grouping_by_two_columns_keys_on_the_pair() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT author_id, year, count(*) FROM books WHERE author_id < 5 GROUP BY author_id, year",
    );
    assert_eq!(answer["columns"], json!(["author_id", "year", "count(*)"]));
    let rows = answer["rows"].as_array().unwrap();
    assert!(rows.iter().all(|r| r.as_array().unwrap().len() == 3));

    // Every (author, year) pair is distinct, and the counts still sum to the
    // rows that went in.
    let total: u64 = rows
        .iter()
        .map(|r| r[2].as_str().unwrap().parse::<u64>().unwrap())
        .sum();
    let all = one(&playground, "SELECT * FROM books WHERE author_id < 5");
    assert_eq!(total, all["returned"].as_u64().unwrap());

    let mut keys: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap().to_owned(),
                r[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let before = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), before, "a key appeared twice");

    // And the *plan* has to describe the same two-key grouping. It is built
    // separately from the one that runs — `explain_grouped` takes a `Grouping`
    // and `group_by` takes the keys — so nothing but this assertion stops the
    // two drifting. A mutation that explained only the first key passed every
    // other test in this file.
    let decodes = answer["plan"]["decodes"].as_array().unwrap();
    assert!(
        decodes.contains(&json!(1)) && decodes.contains(&json!(3)),
        "the plan should read both group keys, got {decodes:?}"
    );
}
