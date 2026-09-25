//! The crate's own surface, exercised without a browser.
//!
//! # Why this file exists
//!
//! Every other test of this code — 226 of them — lives in
//! `crates/slate-wasm/tests/` and reaches it through `Playground`. They are
//! real tests and they are not *this crate's*: they exercise the parser and
//! the lowering as the browser binding happens to call them, so a change that
//! broke a **non-browser** caller would be caught only if the browser happened
//! to care about the same thing.
//!
//! That mattered little while the browser was the only caller. `slate-serverd`
//! is about to be the second, because a view is SQL written down in a TOML
//! file (`docs/views.md`), and the two entry points it will use are the two
//! this file pins:
//!
//! 1. `sql::parse(text, &Schema(&tables))` — text to a `QuerySpec`.
//! 2. `lower::build(&spec, &table)` — that spec to a `slate_kernel::Query`.
//!
//! Composed, those are "SQL in, `Query` out", which is the whole of what a
//! daemon resolving a view needs.
//!
//! # The other thing it pins, by compiling
//!
//! This file builds a `TableDef` by hand and calls both entry points. It does
//! not mention `slate_wasm`, `Playground`, a fixture or a browser. A future
//! change that made the crate depend on any of those would break this file,
//! which is the point — the extraction claimed the front end is portable, and
//! a test that could only run inside the binding would not be evidence of it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use slate_kernel::{CmpOp, Expr};
use slate_schema::{IndexDef, IndexId, Ordinal, TableDef, TableId};
use slate_sql::sql::{Schema, Statement, parse};
use slate_sql::{QuerySpec, lower};
use slate_tuple::{Value, ValueType};

/// One table, built here rather than imported.
///
/// `slate-wasm` has a fixture and this crate deliberately does not depend on
/// it: a test that reached for the browser's tables would be the coupling this
/// file exists to disprove.
fn books() -> TableDef {
    TableDef::builder("books", TableId(1))
        .column("id", ValueType::U64)
        .column("author_id", ValueType::U64)
        .column("title", ValueType::Str)
        .column("year", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_author", IndexId(1)).column("author_id"))
        .build()
        .expect("a valid schema")
}

/// A second table, so a join can be written.
///
/// A view is single-table today (`docs/views.md` §5), so this exists for the
/// grammar test below rather than for the view path.
fn authors() -> TableDef {
    TableDef::builder("authors", TableId(2))
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .column("country", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("a valid schema")
}

/// The one statement kind a view is, parsed.
fn select(text: &str) -> QuerySpec {
    let tables = [books()];
    let parsed = parse(text, &Schema(&tables)).unwrap_or_else(|e| panic!("{text}: {}", e.message));
    match parsed.statement {
        Statement::Select(spec) => spec,
        other => panic!("{text} parsed as {other:?}, not a single-table read"),
    }
}

#[test]
fn a_select_parses_into_a_spec_naming_the_table_and_the_filter() {
    let spec = select("SELECT * FROM books WHERE year >= 1970");
    assert_eq!(spec.table, "books");
    assert_eq!(spec.filters.len(), 1);
    // Ordinal 3 is `year`, and the parser resolved the name to it. That
    // resolution is the reason a view is parsed against the catalog at load
    // time rather than stored as text and parsed per query.
    assert_eq!(spec.filters[0].column, 3);
    assert_eq!(spec.filters[0].op, "ge");
    assert_eq!(spec.filters[0].value, "1970");
}

#[test]
fn a_spec_lowers_to_a_kernel_query_with_that_filter() {
    let spec = select("SELECT * FROM books WHERE year >= 1970");
    let query = lower::build(&spec, &books()).expect("a valid lowering");
    assert_eq!(
        query.filter,
        Expr::Compare {
            column: Ordinal(3),
            op: CmpOp::Ge,
            value: Value::I64(1970),
        }
    );
}

#[test]
fn text_becomes_a_query_with_nothing_in_between() {
    // The two calls above, composed — which is exactly what resolving a view
    // will be, and the reason both are public.
    let table = books();
    let tables = [table.clone()];
    let parsed = parse(
        "SELECT * FROM books WHERE title = 'Kindred'",
        &Schema(&tables),
    )
    .expect("a valid statement");
    let Statement::Select(spec) = parsed.statement else {
        panic!("not a select");
    };
    let query = lower::build(&spec, &table).expect("a valid lowering");
    assert_eq!(
        query.filter,
        Expr::Compare {
            column: Ordinal(2),
            op: CmpOp::Eq,
            value: Value::Str("Kindred".to_owned()),
        }
    );
}

#[test]
fn two_conditions_lower_to_a_conjunction() {
    // A view's predicate and a caller's will be `AND`ed the same way
    // (`views.md` §3), so the shape this produces is the shape that
    // composition will have to extend.
    let spec = select("SELECT * FROM books WHERE year >= 1970 AND author_id = 3");
    let query = lower::build(&spec, &books()).expect("a valid lowering");
    match query.filter {
        Expr::And(parts) => assert_eq!(parts.len(), 2, "{parts:?}"),
        other => panic!("two conditions lowered to {other:?}, not a conjunction"),
    }
}

#[test]
fn a_parse_failure_carries_the_offset_and_not_only_a_message() {
    let tables = [books()];
    let error = parse("SELECT * FROM books WHERE nosuch = 1", &Schema(&tables))
        .expect_err("an unknown column must be refused");
    assert!(
        error.message.contains("nosuch"),
        "the refusal should name the column: {}",
        error.message
    );
    // The offset is what lets an editor underline the word, and what a TOML
    // loader will turn into "line 12 of your schema file". A message alone
    // would make the second of those impossible.
    assert!(error.at > 0, "expected a byte offset, got {}", error.at);
}

#[test]
fn a_table_the_schema_does_not_have_is_refused_by_name() {
    let tables = [books()];
    let error = parse("SELECT * FROM shelves", &Schema(&tables))
        .expect_err("an unknown table must be refused");
    assert!(
        error.message.contains("shelves"),
        "the refusal should name the table: {}",
        error.message
    );
}

#[test]
fn the_spec_round_trips_through_json() {
    // A view will be stored as the spec the SQL compiled to, so the spec has
    // to survive serialisation. `QuerySpec` derives both halves for the
    // workbench's benefit; this checks the property a daemon would rely on.
    let spec = select("SELECT * FROM books WHERE year >= 1970");
    let text = serde_json::to_string(&spec).expect("serialisable");
    let back: QuerySpec = serde_json::from_str(&text).expect("deserialisable");
    assert_eq!(back, spec);
}

#[test]
fn a_join_groups_by_more_than_one_key() {
    // The grammar comment in `sql.rs` said "on a join: one GROUP BY key" long
    // after `JoinSpec::group_by` became a `Vec<u32>` and the parser started
    // accepting a list. Nothing executed the claim either way, so it survived
    // the change that falsified it.
    //
    // This is the test that would have caught it. It is here rather than in
    // `slate-wasm`'s suite for the reason at the top of this file: the claim
    // is about *the grammar*, not about what the browser happens to send.
    let tables = [books(), authors()];
    let text = "SELECT books.author_id, books.year, count(*) \
                FROM books JOIN authors ON books.author_id = authors.id \
                GROUP BY books.author_id, books.year";
    let parsed = parse(text, &Schema(&tables))
        .unwrap_or_else(|e| panic!("two group keys on a join were refused: {}", e.message));
    match parsed.statement {
        Statement::Join(spec) => assert_eq!(
            spec.group_by.len(),
            2,
            "both keys should reach the spec, got {:?}",
            spec.group_by
        ),
        other => panic!("parsed as {other:?}, not a join"),
    }
}

#[test]
fn a_single_table_groups_by_more_than_one_key() {
    // The join case above had a sibling that no test in *this* crate covered:
    // mutating the single-table `GROUP BY` loop to stop after one key survived
    // `cargo test -p slate-sql`. The coverage existed, in `slate-wasm`'s 226
    // tests, which is the coupling the header of this file argues against —
    // a non-browser caller breaking would be caught only if the browser
    // happened to care.
    let tables = [books()];
    let text = "SELECT author_id, year, count(*) FROM books GROUP BY author_id, year";
    let parsed = parse(text, &Schema(&tables))
        .unwrap_or_else(|e| panic!("two group keys were refused: {}", e.message));
    match parsed.statement {
        Statement::Select(spec) => assert_eq!(
            spec.group_by.len(),
            2,
            "both keys should reach the spec, got {:?}",
            spec.group_by
        ),
        other => panic!("parsed as {other:?}, not a single-table read"),
    }
}
