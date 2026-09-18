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
use slate_wasm::{FilterSpec, Playground, QuerySpec, SortSpec, fixture, taxi};

fn tables() -> Vec<TableDef> {
    vec![fixture::authors(), fixture::books()]
}

fn parsed(text: &str) -> Result<Statement, String> {
    parse_with_warnings(text).map(|(statement, _)| statement)
}

/// The statement and the warnings the parser attached to it.
fn parse_with_warnings(text: &str) -> Result<(Statement, Vec<String>), String> {
    let tables = tables();
    parse(text, &Schema(&tables))
        .map(|p| (p.statement, p.warnings))
        .map_err(|e| format!("{} (at {})", e.message, e.at))
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
                // `IN` renders as a list rather than `column IN value`, and
                // it is the only op whose text comes from `values`. A
                // subquery is never generated: nothing in production renders
                // a spec back to SQL, this oracle is the only renderer there
                // is, and teaching it to emit `IN (SELECT …)` would be
                // writing the inverse of the parser purely to test it against
                // itself.
                if f.op == "in" {
                    let list: Vec<String> = f.values.iter().map(|v| literal(f.column, v)).collect();
                    return format!("{} IN ({})", name(f.column), list.join(", "));
                }
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

/// A literal `IN (…)`, which the parser and the renderer both have to agree
/// about — a list of one, and a list that needs quoting, included.
fn in_filter_strategy() -> BoxedStrategy<FilterSpec> {
    (0u32..4)
        .prop_flat_map(|column| {
            (
                Just(column),
                proptest::collection::vec(value_for(column), 1..4),
            )
        })
        .prop_map(|(column, values)| FilterSpec {
            column,
            op: "in".to_owned(),
            values,
            ..FilterSpec::default()
        })
        .boxed()
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
            ..FilterSpec::default()
        })
        .boxed()
}

prop_compose! {
    fn spec_strategy()(
        columns in proptest::collection::vec(0u32..4, 0..4),
        filters in proptest::collection::vec(
            prop_oneof![3 => filter_strategy(), 1 => in_filter_strategy()],
            0..3,
        ),
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
            // HAVING is meaningless without a grouping and the parser refuses
            // it, so it is empty here for the same reason `group_by` is.
            having: Vec::new(),
            // Rendering a computed column back to SQL would mean teaching the
            // renderer the call syntax and the find-or-add rule; the time
            // functions have their own suite in `datetime.rs`.
            compute: Vec::new(),
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

// --- the chain ------------------------------------------------------------
//
// Three or more tables, which the front end refused until the parser was
// rewritten over a list of inputs instead of a left and a right. The kernel,
// the wire and all three SDKs had done chains for months; this was the only
// place that could not say one.
//
// The chains here are `authors JOIN books JOIN zones`, joined on `books.id =
// zones.id`. That is a meaningless *question* — a book id is not a taxi zone
// id — and a perfectly well-formed chain, which is the distinction the parser
// is responsible for. It is also the only three-table chain this schema can
// express without aliases: `trips` reaches `zones` twice, through
// `pickup_zone` and `dropoff_zone`, and naming the same table twice needs an
// alias the subset does not have. That gap is recorded rather than papered
// over with a fourth fixture table nothing else would use.

/// Every table the binding's SQL path knows.
///
/// `tables()` above is `authors` and `books`, which by construction cannot
/// hold a chain — a parse test for three tables needs a schema with three.
fn every_table() -> Vec<TableDef> {
    vec![
        fixture::authors(),
        fixture::books(),
        taxi::trips(),
        taxi::zones(),
    ]
}

fn parsed_over_every_table(text: &str) -> Result<Statement, String> {
    let tables = every_table();
    parse(text, &Schema(&tables))
        .map(|p| p.statement)
        .map_err(|e| format!("{} (at {})", e.message, e.at))
}

/// `authors JOIN books JOIN zones`, as SQL and as the spec it should become.
const CHAIN: &str = "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
                     JOIN zones ON books.id = zones.id";

#[test]
fn two_tables_are_a_join_and_three_are_a_chain() {
    // The fork, asserted at the parser rather than through the answer. Both
    // shapes return rows and a plan, so a chain lowered onto `Join` — or a
    // two-table query lowered onto `Chain` — would come back looking right.
    // What distinguishes them is which kernel entry point runs, and only the
    // statement says that.
    let two =
        parsed_over_every_table("SELECT * FROM authors JOIN books ON authors.id = books.author_id")
            .unwrap();
    let Statement::Join(join) = two else {
        panic!("two tables should be a join: {two:?}");
    };
    assert_eq!(
        (join.left.as_str(), join.right.as_str()),
        ("authors", "books")
    );
    assert_eq!((join.left_key, join.right_key), (0, 1));

    let three = parsed_over_every_table(CHAIN).unwrap();
    let Statement::Chain(chain) = three else {
        panic!("three tables should be a chain: {three:?}");
    };
    let names: Vec<&str> = chain.inputs.iter().map(|i| i.table.as_str()).collect();
    assert_eq!(names, ["authors", "books", "zones"]);
    // The first table joins to nothing, and each later one names an earlier
    // input and both columns. Off by one here is a chain whose last table has
    // no key and whose second has two.
    assert!(chain.inputs[0].on.is_none(), "{:?}", chain.inputs[0]);
    let first = chain.inputs[1].on.as_ref().unwrap();
    assert_eq!((first.input, first.column, first.own), (0, 0, 1));
    let second = chain.inputs[2].on.as_ref().unwrap();
    assert_eq!((second.input, second.column, second.own), (1, 0, 0));
}

#[test]
fn a_chain_step_may_join_back_past_the_previous_table() {
    // `JoinKey` is in the joined space and always has been, so a step may key
    // on any table already read. A parser that only allowed the previous one
    // would refuse a star schema — every dimension hanging off one fact table
    // — which is the commonest chain there is.
    let parsed = parsed_over_every_table(
        "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
         JOIN zones ON authors.id = zones.id",
    )
    .unwrap();
    let Statement::Chain(chain) = parsed else {
        panic!("expected a chain");
    };
    let second = chain.inputs[2].on.as_ref().unwrap();
    assert_eq!((second.input, second.column, second.own), (0, 0, 0));
}

#[test]
fn a_chain_through_sql_matches_the_chain_spec() {
    let playground = Playground::new();
    let through_sql = one(&playground, CHAIN);
    let through_spec: Json = serde_json::from_str(
        &playground.chain(
            &json!({
                "inputs": [
                    { "table": "authors" },
                    { "table": "books", "on": { "input": 0, "column": 0, "own": 1 } },
                    { "table": "zones", "on": { "input": 1, "column": 0, "own": 0 } },
                ],
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert!(
        through_spec["error"].is_null(),
        "the spec was refused: {through_spec}"
    );
    assert_eq!(through_sql["rows"], through_spec["rows"], "{through_sql}");
    // The plan text too, not just the rows. Rows alone would pass for a front
    // end that quietly dropped a step and got lucky, which over three tables
    // joined on an id is exactly the kind of luck available.
    assert_eq!(through_sql["message"], through_spec["display"]);
}

#[test]
fn a_chains_header_names_every_table_and_a_chain_row_is_that_wide() {
    let playground = Playground::new();
    let answer = one(&playground, CHAIN);
    let columns: Vec<&str> = answer["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    // Qualified, because three unqualified `id`s side by side name nothing.
    assert_eq!(
        columns,
        [
            "authors.id",
            "authors.name",
            "authors.country",
            "authors.born",
            "books.id",
            "books.author_id",
            "books.title",
            "books.year",
            "books.price",
            "zones.id",
            "zones.borough",
            "zones.zone",
            "zones.service_zone",
        ]
    );
    let rows = answer["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "no rows: {answer}");
    assert!(
        rows.iter()
            .all(|row| row.as_array().unwrap().len() == columns.len()),
        "a row is not as wide as the header: {answer}"
    );
}

#[test]
fn a_grouped_chain_agrees_with_the_join_the_chain_narrows() {
    let playground = Playground::new();
    // The zone ids are 1..=n with no holes, which this checks rather than
    // assumes -- the differential below depends on it, and a source file that
    // grew a gap would otherwise make this test quietly compare two different
    // questions.
    let zones = one(&playground, "SELECT * FROM zones");
    let count = zones["returned"].as_u64().unwrap();
    let highest: u64 = zones["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().parse::<u64>().unwrap())
        .max()
        .unwrap();
    assert_eq!(count, highest, "the zone ids have a hole in them");

    // So `books.id = zones.id` keeps exactly the books whose id is at most
    // `highest`, and the chain's counts per country must equal the two-table
    // join's counts with that as a filter. An independent query as the oracle,
    // not a hand-written table of numbers: a fixture change moves both.
    let chained = one(
        &playground,
        "SELECT count(*) FROM authors JOIN books ON authors.id = books.author_id \
         JOIN zones ON books.id = zones.id GROUP BY country",
    );
    let joined = one(
        &playground,
        &format!(
            "SELECT count(*) FROM authors JOIN books ON authors.id = books.author_id \
             WHERE books.id <= {highest} GROUP BY country"
        ),
    );
    assert_eq!(chained["kind"], "group");
    assert_eq!(chained["columns"], json!(["country", "count(*)"]));
    assert_eq!(chained["rows"], joined["rows"], "{chained} vs {joined}");
    assert!(
        !chained["rows"].as_array().unwrap().is_empty(),
        "no groups, so this compared nothing: {chained}"
    );
}

#[test]
fn a_chains_computed_value_sits_past_every_table() {
    // The arithmetic that is easiest to get wrong and hardest to see: a
    // computed value on a join sits after *both* tables, and on a chain after
    // *every* one. Landing it after the first would make the group key some
    // later table's column -- not an error, a different answer.
    let parsed = parsed_over_every_table(
        "SELECT year(books.author_id), count(*) FROM authors \
         JOIN books ON authors.id = books.author_id \
         JOIN zones ON books.id = zones.id GROUP BY year(books.author_id)",
    )
    .unwrap();
    let Statement::Chain(chain) = parsed else {
        panic!("expected a chain");
    };
    assert_eq!(chain.compute.len(), 1, "{:?}", chain.compute);
    assert_eq!(chain.compute[0].function, "year");
    // Named on the table it reads, which is `books` -- input 1.
    assert_eq!((chain.compute[0].input, chain.compute[0].column), (1, 1));
    // 4 + 5 + 4 columns, so the first computed value is ordinal 13. Written
    // out rather than derived, because deriving it here from the same widths
    // the parser used would be the parser checking its own arithmetic.
    assert_eq!(chain.group_by, vec![13]);
    // And the same call written twice is one computed column, not two: the
    // select list and the GROUP BY find-or-add into the same list.
    assert_eq!(chain.compute.len(), 1);
}

#[test]
fn a_computed_value_is_named_from_the_table_it_reads() {
    // This was wrong, and only in the header. The join arm resolved every
    // computed column's name against the *left* table whatever its `input`
    // said, so `year(books.author_id)` -- ordinal 1 of `books` -- came back
    // labelled `year(name)`, which is ordinal 1 of `authors`. The values
    // underneath were right, which is why no test asserting on cells caught it.
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT year(books.author_id), count(*) FROM authors \
         JOIN books ON authors.id = books.author_id GROUP BY year(books.author_id)",
    );
    assert_eq!(answer["columns"][0], json!("year(author_id)"), "{answer}");
}

#[test]
fn an_aggregate_is_labelled_with_the_column_it_reads() {
    // The same defect, in the aggregate labels: they were resolved against the
    // right table, from when an aggregate could only read the right table.
    // `max(born)` -- ordinal 3 of `authors` -- was labelled `max(year)`, which
    // is ordinal 3 of `books`.
    //
    // The value is checked against the single-table query as an oracle, so
    // this test fails either way round: a right label over a wrong column, or
    // a wrong label over a right one.
    let playground = Playground::new();
    let joined = one(
        &playground,
        "SELECT count(*), max(born) FROM authors JOIN books ON authors.id = books.author_id \
         GROUP BY country",
    );
    assert_eq!(
        joined["columns"],
        json!(["country", "count(*)", "max(born)"]),
        "{joined}"
    );
    let alone = one(
        &playground,
        "SELECT max(born) FROM authors GROUP BY country",
    );
    let born_by_country = |answer: &Json, which: usize| -> Vec<(String, String)> {
        answer["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row[0].as_str().unwrap().to_owned(),
                    row[which].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    // Every author has at least one book in this fixture, so grouping the join
    // by country cannot lose a country, and the latest birth year per country
    // is the same either way.
    assert_eq!(
        born_by_country(&joined, 2),
        born_by_country(&alone, 1),
        "{joined} vs {alone}"
    );
}

#[test]
fn a_query_with_no_alias_carries_no_alias_in_its_spec() {
    // The backward-compatibility claim, which is written in `ChainInputSpec`'s
    // own doc comment: a query that names no alias serialises exactly as it did
    // before aliases existed. Both fields are
    // `skip_serializing_if = "String::is_empty"`, and `alias_of` returns an
    // empty string when the name is the table's own -- so the claim rests on
    // two things agreeing, and nothing checked either.
    //
    // This test exists because a mutation survived. Making `alias_of` always
    // return the name changed no answer, no header and no refusal: every spec
    // simply grew a field. That is invisible to every other test here and
    // visible in the Spec tab of the workbench, in the JSON a reader is being
    // told is what the SDKs send.
    let spec_of = |text: &str| -> Json {
        match parsed_over_every_table(text).unwrap() {
            Statement::Join(join) => serde_json::to_value(join).unwrap(),
            Statement::Chain(chain) => serde_json::to_value(chain).unwrap(),
            other => panic!("not a join or a chain: {other:?}"),
        }
    };

    let plain_join = spec_of("SELECT * FROM authors JOIN books ON authors.id = books.author_id");
    assert!(
        plain_join.get("leftAlias").is_none() && plain_join.get("rightAlias").is_none(),
        "a join with no alias carries one: {plain_join}"
    );
    let plain_chain = spec_of(CHAIN);
    for input in plain_chain["inputs"].as_array().unwrap() {
        assert!(
            input.get("alias").is_none(),
            "a chain input with no alias carries one: {input}"
        );
    }

    // And the other direction, so this cannot pass by never writing the field
    // at all: an alias that *was* written reaches the spec, under the name the
    // reader gave it.
    let aliased_join = spec_of("SELECT * FROM authors JOIN books AS b ON authors.id = b.author_id");
    assert_eq!(aliased_join["rightAlias"], json!("b"), "{aliased_join}");
    assert!(aliased_join.get("leftAlias").is_none(), "{aliased_join}");

    let aliased_chain = spec_of(
        "SELECT * FROM trips JOIN zones AS pickup ON trips.pickup_zone = pickup.id \
         JOIN zones AS dropoff ON trips.dropoff_zone = dropoff.id",
    );
    let inputs = aliased_chain["inputs"].as_array().unwrap();
    assert!(inputs[0].get("alias").is_none(), "{aliased_chain}");
    assert_eq!(inputs[1]["alias"], json!("pickup"), "{aliased_chain}");
    assert_eq!(inputs[2]["alias"], json!("dropoff"), "{aliased_chain}");
}

#[test]
fn a_group_key_written_twice_is_one_key() {
    // `GROUP BY borough, borough` is one key written twice. Keeping both
    // returns the same groups with the column repeated in every row and the
    // header -- the same answer, wider, and no error anywhere, which is why
    // this needs a test rather than an eye.
    //
    // It exists because a mutation survived: removing the `contains` check
    // changed no count, no group and no refusal. The rule it enforces is
    // already documented beside it, and documentation is not a test.
    //
    // The rule is also not arbitrary. `join_value_ordinal` deduplicates a
    // *computed* key by find-or-add -- `hour(t)` in the select list and in the
    // GROUP BY is one computed column -- so a stored key behaving differently
    // would make two spellings of one mistake behave two ways.
    let Statement::Join(join) = parsed_over_every_table(
        "SELECT borough, count(*) FROM trips JOIN zones ON trips.pickup_zone = zones.id \
         GROUP BY borough, borough",
    )
    .unwrap() else {
        panic!("expected a join");
    };
    assert_eq!(join.group_by.len(), 1, "{:?}", join.group_by);

    // And the computed twin, which takes the other branch: the same call in
    // the select list and in the GROUP BY registers one computed column, so
    // naming it twice in the GROUP BY must not register a second.
    let Statement::Join(computed) = parsed_over_every_table(
        "SELECT hour(pickup_time), count(*) FROM trips \
         JOIN zones ON trips.pickup_zone = zones.id \
         GROUP BY hour(pickup_time), hour(pickup_time)",
    )
    .unwrap() else {
        panic!("expected a join");
    };
    assert_eq!(computed.group_by.len(), 1, "{:?}", computed.group_by);
    assert_eq!(computed.compute.len(), 1, "{:?}", computed.compute);
}

#[test]
fn a_chain_refuses_what_it_cannot_answer() {
    let playground = Playground::new();
    let cases: Vec<(&str, &str)> = vec![
        // A table twice under one name. With aliases this is no longer a dead
        // end, so the refusal names the way out rather than only the problem.
        (
            "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
             JOIN authors ON books.id = authors.id",
            "is read twice under one name",
        ),
        // A step whose ON reaches no earlier table. The two-table message
        // names both tables; past two it names the new one and lists the
        // others, because "one column of `authors` and one of `zones`" would
        // be leaving out the table in the middle.
        (
            "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
             JOIN zones ON nosuch = zones.id",
            "does not name one column of `zones` and one of a table read before it",
        ),
        // The same shape as the two-table case above, on a step: both sides
        // of the ON on the table being joined. A step has to reach a table
        // already read, and the one it is adding is not one.
        (
            "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
             JOIN zones ON zones.id = zones.borough",
            "does not name one column of `zones` and one of a table read before it",
        ),
        // Whole rows or one row per group, and the refusal says "chain"
        // rather than "join" -- naming a construct the reader did not write
        // sends them looking at the wrong line.
        (
            "SELECT title FROM authors JOIN books ON authors.id = books.author_id \
             JOIN zones ON books.id = zones.id",
            "a chain returns whole rows",
        ),
        (
            "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
             JOIN zones ON books.id = zones.id ORDER BY books.id",
            "ORDER BY on a chain needs a GROUP BY",
        ),
        // An aggregate over a column no input has. The list is all three
        // tables, which is the message a two-table `left`/`right` signature
        // could not produce.
        (
            "SELECT count(*), max(nosuch) FROM authors JOIN books ON authors.id = books.author_id \
             JOIN zones ON books.id = zones.id GROUP BY country",
            "`max()` reads a column of `authors`, `books` or `zones`; `nosuch` is none of them",
        ),
        // Not about the column at all, and it used to be reported as if it
        // were: this said "`year` is neither" about a column both tables have.
        (
            "SELECT count(*), nosuchagg(year) FROM authors \
             JOIN books ON authors.id = books.author_id GROUP BY country",
            "no such aggregate",
        ),
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
fn a_bare_name_three_tables_share_says_which_one_it_read() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT * FROM authors JOIN books ON authors.id = books.author_id \
         JOIN zones ON books.id = zones.id WHERE id = 1",
    );
    let warnings = answer["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{answer}");
    let warning = warnings[0].as_str().unwrap();
    // All three of them listed, not "both" of two. The two-table version said
    // "a column of both `a` and `b`", which a third table makes wrong rather
    // than merely long.
    assert!(
        warning.contains("`authors`, `books` and `zones`"),
        "{warning}"
    );
    assert!(warning.contains("this read `authors.id`"), "{warning}");
    assert!(!warning.contains("both"), "still says `both`: {warning}");
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
        "INSERT INTO books VALUES (9001, 3, 'Something New', 2024, 21.50)",
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
        "INSERT INTO books VALUES (9100, 1, 'One', 2001, 10.00);\n\
         SELECT * FROM books WHERE id = 9100;\n\
         SELECT * FROM nosuch;\n\
         INSERT INTO books VALUES (9101, 1, 'Two', 2002, 11.00)",
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
        ("INSERT INTO books VALUES (1, 2)", "takes 5 values"),
        ("INSERT INTO books (id) VALUES (1)", "no column list"),
        // `SELECT count(*) FROM books` used to be here. It is not a refusal
        // any more: a grouping with no keys is one group over every row, which
        // the kernel has always answered, and refusing it was a front-end
        // guard that mistook the usual shape for the only one. What is still
        // refused is a *column* beside the aggregate, because the single row
        // it returns has no one value for that column to take.
        ("SELECT title, count(*) FROM books", "needs a GROUP BY"),
        // These two used to be refusals, when the parser knew one join and
        // checked the `ON` clause against it. Any pair of tables and columns
        // is legal now — `books.id = authors.id` is a meaningless join and a
        // valid one, and every SQL engine will run it — so what is left to
        // refuse is a key that does not name one column of each side.
        (
            "SELECT * FROM trips JOIN zones ON trips.nosuch = zones.id",
            "does not name one column of `trips` and one of `zones`",
        ),
        // Both sides of the ON naming the *same* table. This one is here
        // because a mutation survived without it, and what the mutation
        // caused was not a refusal but a wrong answer: resolving the far side
        // against every table rather than the earlier ones accepts
        // `books.id = books.author_id`, and then `left_key` is an ordinal of
        // `books` used as an ordinal of `authors` -- joining `authors.name`
        // to `books.id`, with nothing erring anywhere.
        (
            "SELECT * FROM authors JOIN books ON books.id = books.author_id",
            "does not name one column of `authors` and one of `books`",
        ),
        // One table twice under one name. It used to be "cannot be joined to
        // itself", which was the whole truth while there were no aliases; now
        // the fix exists, so the message names it.
        (
            "SELECT * FROM trips JOIN trips ON trips.id = trips.id",
            "is read twice under one name",
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
    assert_eq!(ungrouped["plan"]["decodes"], json!([0, 1, 2, 3, 4]));
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
        // `SELECT count(*) FROM books` used to be here. It is not a refusal
        // any more: a grouping with no keys is one group over every row, which
        // the kernel has always answered, and refusing it was a front-end
        // guard that mistook the usual shape for the only one. What is still
        // refused is a *column* beside the aggregate, because the single row
        // it returns has no one value for that column to take.
        ("SELECT title, count(*) FROM books", "needs a GROUP BY"),
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

// ------------------------------------------------------------------ decimals

/// `WHERE price > 19.99` compares a decimal to a decimal, at the column's scale.
///
/// Before this the literal fell through to `Value::Str("19.99")`, which sorts
/// below every decimal in the cross-type order — so the query returned *no
/// rows*, with nothing anywhere saying why. A query that silently answers
/// nothing is the worst outcome available, and it is the one this had.
///
/// The seeded prices for ids 1..24 run 8.95 up to 24.70 in 68-cent steps, so
/// the answer is checkable by hand: the first price over 19.99 is id 17 at
/// 19.83… no — 8.95 + 16 × 0.68 = 19.83, and id 18 is 20.51. So `> 19.99`
/// starts at id 18.
#[test]
fn a_decimal_literal_is_read_at_the_columns_scale() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT id, price FROM books WHERE id <= 24 AND price > 19.99 ORDER BY id",
    );
    let rows = answer["rows"].as_array().expect("rows");
    assert_eq!(
        rows.first().and_then(|r| r[0].as_str()),
        Some("18"),
        "8.95 + 17 * 0.68 = 20.51 is the first over 19.99: {answer}"
    );
    assert_eq!(rows.len(), 7, "ids 18..=24: {answer}");
    // And the value comes back written the way it was typed, not as its units
    // and not as `Decimal(2051)`. Ordinal 4, because the binding returns the
    // whole row with the unprojected columns as `null` and `columns` as the
    // full header — a `SELECT` list narrows what is *decoded*, not the width.
    assert_eq!(rows[0][4].as_str(), Some("20.51"), "{answer}");
}

/// The boundary, which is what says the comparison is exact rather than
/// approximately right.
#[test]
fn a_decimal_compares_at_the_boundary_not_near_it() {
    let playground = Playground::new();
    // id 17 is 8.95 + 16 * 0.68 = 19.83.
    let exact = one(
        &playground,
        "SELECT id FROM books WHERE id <= 24 AND price = 19.83",
    );
    assert_eq!(exact["rows"].as_array().map(Vec::len), Some(1), "{exact}");
    // The same number with a trailing zero is the same number, so it matches
    // the same row: `19.830` at scale 2 is 1983 units and loses nothing.
    let padded = one(
        &playground,
        "SELECT id FROM books WHERE id <= 24 AND price = 19.830",
    );
    assert_eq!(padded["rows"].as_array().map(Vec::len), Some(1), "{padded}");

    // A cent either side of it matches nothing, which a float comparison
    // could not promise.
    for near in ["19.82", "19.84"] {
        let miss = one(
            &playground,
            &format!("SELECT id FROM books WHERE id <= 24 AND price = {near}"),
        );
        assert_eq!(
            miss["rows"].as_array().map(Vec::len),
            Some(0),
            "{near} matched something: {miss}"
        );
    }
}

/// A number with more places than the column holds is refused, not rounded.
#[test]
fn more_decimal_places_than_the_column_has_is_refused() {
    let playground = Playground::new();
    let message = refusal(&playground, "SELECT id FROM books WHERE price = 19.999");
    assert!(message.contains("places after the point"), "{message}");
}

/// `INSERT` takes the same literal, so a price written once reads back the same.
#[test]
fn a_decimal_round_trips_through_insert_and_select() {
    let playground = Playground::new();
    let _ = one(
        &playground,
        "INSERT INTO books VALUES (9300, 1, 'Priced', 2020, 7.05)",
    );
    let back = one(&playground, "SELECT price FROM books WHERE id = 9300");
    assert_eq!(
        back["rows"][0][4].as_str(),
        Some("7.05"),
        "written as 7.05 and read back as something else: {back}"
    );
}

/// `UPDATE` reads the row, renders it, edits one cell and writes it back — so
/// a decimal in *another* column has to survive that round trip.
///
/// It did not: `text` rendered a decimal as `Decimal(705)`, which the parser
/// then refused, so every `UPDATE` on a table with a decimal column failed
/// whatever it set. The fixture had no decimal column, so nothing caught it
/// until one was added.
#[test]
fn updating_another_column_leaves_the_price_alone() {
    let playground = Playground::new();
    let _ = one(
        &playground,
        "INSERT INTO books VALUES (9301, 1, 'Before', 2020, 3.25)",
    );
    let _ = one(
        &playground,
        "UPDATE books SET title = 'After' WHERE id = 9301",
    );
    let back = one(
        &playground,
        "SELECT title, price FROM books WHERE id = 9301",
    );
    assert_eq!(back["rows"][0][2].as_str(), Some("After"), "{back}");
    assert_eq!(back["rows"][0][4].as_str(), Some("3.25"), "{back}");
}

/// `HAVING sum(price) > ...` reads its literal at the aggregate's scale.
///
/// `Total::sum` over a decimal returns a decimal, and the front end was typing
/// the `HAVING` literal from a table of "integer or float" — so the comparison
/// was `I64` against `Decimal`, which differ by class rank and admit nothing.
/// Same failure as `WHERE`, one level up, and it needed the same fix in the
/// one function that decides a group column's type.
#[test]
fn having_over_a_summed_decimal_compares_as_a_decimal() {
    let playground = Playground::new();
    let answer = one(
        &playground,
        "SELECT author_id, sum(price) FROM books WHERE id <= 24 \
         GROUP BY author_id HAVING sum(price) > 60.00 ORDER BY author_id",
    );
    let rows = answer["rows"].as_array().expect("rows");
    assert!(
        !rows.is_empty(),
        "admitted nothing, which is the old bug: {answer}"
    );
    // Every admitted group's total is over sixty, read as a number rather than
    // as a count of cents.
    for row in rows {
        let total: f64 = row[1]
            .as_str()
            .expect("a rendered total")
            .parse()
            .expect("a number");
        assert!(total > 60.0, "{row:?} in {answer}");
    }
}
