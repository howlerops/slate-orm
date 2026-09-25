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

#[test]
fn or_lowers_to_a_disjunction_the_kernel_understands() {
    // `Expr::Or` has existed in the kernel since expressions arrived and no
    // front end could produce one — four ledger entries recorded that, and
    // each one said the kernel was not the missing part. This is the test
    // that it is reachable.
    let spec = select("SELECT * FROM books WHERE year >= 1970 OR author_id = 3");
    assert!(
        spec.filters.is_empty(),
        "an ORed WHERE does not fill `filters`"
    );
    assert_eq!(spec.any_of.len(), 2);
    let query = lower::build(&spec, &books()).expect("a valid lowering");
    match query.filter {
        Expr::Or(parts) => assert_eq!(parts.len(), 2, "{parts:?}"),
        other => panic!("OR lowered to {other:?}, not a disjunction"),
    }
}

#[test]
fn a_disjunction_admits_a_row_either_arm_admits() {
    // The shape above is not the point; the answer is. Three rows, one
    // matching only the left arm, one only the right, one neither.
    let spec = select("SELECT * FROM books WHERE year >= 1970 OR author_id = 3");
    let filter = lower::build(&spec, &books())
        .expect("a valid lowering")
        .filter;
    let row = |author_id: u64, year: i64| {
        slate_schema::Row::new(vec![
            Value::U64(1),
            Value::U64(author_id),
            Value::Str("t".to_owned()),
            Value::I64(year),
        ])
    };
    use slate_kernel::Truth;
    assert_eq!(
        filter.evaluate(&row(9, 1999)),
        Truth::True,
        "left arm alone"
    );
    assert_eq!(
        filter.evaluate(&row(3, 1950)),
        Truth::True,
        "right arm alone"
    );
    assert_eq!(filter.evaluate(&row(9, 1950)), Truth::False, "neither arm");
}

#[test]
fn and_and_or_cannot_be_mixed_in_one_where() {
    // No parentheses in this grammar, so `a AND b OR c` would have to pick a
    // precedence and be right about it for every reader. It refuses instead,
    // and the message says what to write.
    let tables = [books()];
    let error = parse(
        "SELECT * FROM books WHERE year >= 1970 AND author_id = 3 OR author_id = 4",
        &Schema(&tables),
    )
    .expect_err("a mixed WHERE must be refused");
    assert!(
        error.message.contains("cannot be mixed"),
        "the refusal should name the problem: {}",
        error.message
    );
}

#[test]
fn the_other_order_of_mixing_is_refused_too() {
    // `OR` then `AND`, which takes the other branch of the check. Written
    // because the first version of this only refused one of the two and the
    // asymmetry was invisible from the passing test.
    let tables = [books()];
    let error = parse(
        "SELECT * FROM books WHERE year >= 1970 OR author_id = 3 AND author_id = 4",
        &Schema(&tables),
    )
    .expect_err("a mixed WHERE must be refused whichever order it is written");
    assert!(
        error.message.contains("cannot be mixed"),
        "the refusal should name the problem: {}",
        error.message
    );
}

// --- the grammar block, line by line ---------------------------------------
//
// `sql.rs`'s module documentation opens with the grammar "in full", and a
// reader takes it as the specification. Its *name* lists are checked
// mechanically by `sql::grammar`; its productions cannot be, so each line has
// a case here that parses the thing it describes. Both kinds exist because the
// block had gone stale in both ways at once: the call list was three weeks
// behind `month_start`, and the `WHERE` line said `AND` only on the day after
// `OR` landed.
//
// A case that merely parses is weak, deliberately. The point is not to
// re-test the parser — the suites above and in `slate-wasm` do that — but to
// make a line of the block that stops being true *fail* rather than mislead.

/// Parse against both tables, for the lines that need a join.
fn parse_two(text: &str) -> slate_sql::sql::Parsed {
    let tables = [books(), authors()];
    parse(text, &Schema(&tables)).unwrap_or_else(|e| panic!("{text}: {}", e.message))
}

fn refuse_two(text: &str) -> String {
    let tables = [books(), authors()];
    parse(text, &Schema(&tables))
        .err()
        .unwrap_or_else(|| panic!("{text} was accepted and the grammar says it is refused"))
        .message
}

#[test]
fn the_select_line_takes_a_star_a_list_and_distinct() {
    // `SELECT [DISTINCT] <* | item-list> FROM <table> [AS <alias>]`
    select("SELECT * FROM books");
    select("SELECT id, title FROM books");
    select("SELECT DISTINCT author_id FROM books");

    // The alias, both spellings — `AS` is optional in the grammar and in SQL.
    // On a *join*, because an alias on a single table is refused: an alias
    // exists to tell two readings of one table apart, and one reading has
    // nothing to tell apart. The first version of this case wrote `FROM books
    // AS b` on its own and failed, which is the `AS only where a table is
    // read more than once` line the block did not have until it did.
    parse_two("SELECT * FROM books AS b JOIN authors ON b.author_id = authors.id");
    parse_two("SELECT * FROM books b JOIN authors ON b.author_id = authors.id");
    let message = refuse_two("SELECT b.title FROM books AS b");
    assert!(
        message.contains("alias"),
        "an alias on a lone table should be refused by name: {message}"
    );
}

#[test]
fn an_ungrouped_join_takes_a_star_and_nothing_else() {
    // The `--` note this audit added. A projection on a joined row would be a
    // projection in the *joined* ordinal space, which is not what a reader
    // writing `books.title` means, so it is refused rather than guessed at.
    parse_two("SELECT * FROM books JOIN authors ON books.author_id = authors.id");
    let message =
        refuse_two("SELECT books.title FROM books JOIN authors ON books.author_id = authors.id");
    assert!(
        message.contains("SELECT *"),
        "the refusal should say what to write instead: {message}"
    );
    // And a GROUP BY lifts it, because a grouped answer's select list *is* its
    // shape. Without this the case above would read as "a join cannot
    // project", which is wrong.
    parse_two(
        "SELECT authors.name, count(*) FROM books JOIN authors ON books.author_id = authors.id \
         GROUP BY authors.name",
    );
}

#[test]
fn the_join_line_repeats_and_a_repeat_is_a_chain() {
    // `( JOIN <table> [AS <alias>] ON <ref> = <ref> )*` — the `*` is the part
    // the old block got wrong: it showed one optional JOIN, and a second one
    // has been a chain since `ledger/2026-09-15-three-tables-in-the-front-end.md`.
    let one = parse_two("SELECT * FROM books JOIN authors ON books.author_id = authors.id");
    assert!(
        matches!(one.statement, Statement::Join(_)),
        "one JOIN is a join: {:?}",
        one.statement
    );

    let tables = [books(), authors()];
    let two = parse(
        "SELECT a.name, count(*) FROM books \
         JOIN authors AS a ON books.author_id = a.id \
         JOIN authors AS b ON books.author_id = b.id \
         GROUP BY a.name",
        &Schema(&tables),
    )
    .unwrap_or_else(|e| panic!("a two-JOIN chain: {}", e.message));
    assert!(
        matches!(two.statement, Statement::Chain(_)),
        "two JOINs are a chain: {:?}",
        two.statement
    );
}

#[test]
fn the_order_by_line_needs_a_group_by_on_a_join() {
    // The first `--` note under the block. It had no case here at all, which
    // `ledger/2026-09-25-the-grammar-comment-outlived-the-grammar.md` recorded
    // as a caveat: the note was corrected that morning and still nothing
    // executed it.
    //
    // Grouped, it parses and the sort is over *groups*.
    parse_two(
        "SELECT authors.name, count(*) FROM books JOIN authors ON books.author_id = authors.id \
         GROUP BY authors.name ORDER BY count(*) DESC",
    );
    // Ungrouped, it is refused — and the refusal explains rather than just
    // rejecting, because `Join` has no sort field: the kernel orders groups,
    // not joined rows. `SELECT *`, because an ungrouped join takes nothing
    // else and the projection refusal would otherwise fire first and this
    // case would pass on the wrong error.
    let message = refuse_two(
        "SELECT * FROM books JOIN authors ON books.author_id = authors.id \
         ORDER BY books.id",
    );
    assert!(
        message.to_lowercase().contains("group by"),
        "the refusal should send the reader to GROUP BY: {message}"
    );
}

#[test]
fn a_joins_where_takes_and_only() {
    // The second `--` note, added with the `OR` work and never executed: a
    // join's conditions are split by side so each scan is narrowed before the
    // hash join runs, and a disjunction spanning both sides cannot be split
    // that way.
    parse_two(
        "SELECT * FROM books JOIN authors ON books.author_id = authors.id \
         WHERE books.year >= 1970 AND authors.country = 'US'",
    );
    let message = refuse_two(
        "SELECT * FROM books JOIN authors ON books.author_id = authors.id \
         WHERE books.year >= 1970 OR authors.country = 'US'",
    );
    assert!(
        !message.is_empty(),
        "an ORed WHERE on a join must be refused with a reason"
    );
}

#[test]
fn an_ored_having_reaches_a_chain() {
    // `HAVING ... OR ...` on three inputs. The module documentation claims
    // `OR` in a `HAVING` works "on a single table, a join and a chain alike",
    // and only the first two had a case — recorded as `No chain case` in
    // `ledger/2026-09-25-or-in-having-too.md`.
    let tables = [books(), authors()];
    let parsed = parse(
        "SELECT a.name, count(*) FROM books \
         JOIN authors AS a ON books.author_id = a.id \
         JOIN authors AS b ON books.author_id = b.id \
         GROUP BY a.name HAVING count(*) > 5 OR count(*) < 2",
        &Schema(&tables),
    )
    .unwrap_or_else(|e| panic!("an ORed HAVING on a chain: {}", e.message));
    match parsed.statement {
        Statement::Chain(spec) => {
            assert_eq!(spec.having_any_of.len(), 2, "both arms should be disjuncts");
            assert!(
                spec.having.is_empty(),
                "an all-OR HAVING has no conjuncts: {:?}",
                spec.having
            );
        }
        other => panic!("expected a chain, got {other:?}"),
    }
}

#[test]
fn the_limit_and_offset_line_takes_integers_and_is_optional() {
    // `[ LIMIT <int> ] [ OFFSET <int> ]`
    assert_eq!(select("SELECT * FROM books").limit, None);
    assert_eq!(select("SELECT * FROM books LIMIT 5").limit, Some(5));
    let both = select("SELECT * FROM books LIMIT 5 OFFSET 2");
    assert_eq!((both.limit, both.offset), (Some(5), 2));
}

#[test]
fn every_operator_the_grammar_lists_parses() {
    // `= != <> < <= > >=`, `LIKE`, `ILIKE`, `~`, `IN`, `NOT IN`, `CONTAINS`.
    // A name list again, and one that nothing checks mechanically — the
    // operators are not a table in the parser the way the calls are, so this
    // is a case per spelling rather than a comparison.
    for clause in [
        "year = 1970",
        "year != 1970",
        "year <> 1970",
        "year < 1970",
        "year <= 1970",
        "year > 1970",
        "year >= 1970",
        "title LIKE 'The %'",
        "title ILIKE 'the %'",
        "title ~ '^The'",
        "year IN (1970, 1971)",
        "year NOT IN (1970, 1971)",
    ] {
        select(&format!("SELECT * FROM books WHERE {clause}"));
    }
}

#[test]
fn the_write_statements_parse_as_the_block_writes_them() {
    let tables = [books()];
    for (text, what) in [
        ("INSERT INTO books VALUES (1, 2, 'a', 1970)", "INSERT"),
        ("UPDATE books SET title = 'b' WHERE id = 1", "UPDATE"),
        ("DELETE FROM books WHERE id = 1", "DELETE"),
    ] {
        parse(text, &Schema(&tables))
            .unwrap_or_else(|e| panic!("the block's {what} line does not parse: {}", e.message));
    }
}

#[test]
fn statements_are_separated_by_a_semicolon_by_split_not_by_parse() {
    // The one line of the block that is about the *buffer* rather than about
    // a statement. It said "Statements may be separated by `;`" beside the
    // grammar, which reads as a property of `parse` — and `parse` answers
    // "unexpected `;`". `split` is what separates them, and the difference
    // matters to a caller: the workbench splits then parses each piece so an
    // error can be reported against the editor's buffer, and `slate-serverd`
    // resolving a view parses one statement directly.
    let tables = [books()];
    let text = "SELECT * FROM books; SELECT id FROM books LIMIT 1";
    assert!(
        parse(text, &Schema(&tables)).is_err(),
        "parse takes one statement; the grammar block used to imply otherwise"
    );
    let pieces = slate_sql::sql::split(text);
    assert_eq!(pieces.len(), 2, "split should find two: {pieces:?}");
    for (at, piece) in pieces {
        parse(&piece, &Schema(&tables))
            .unwrap_or_else(|e| panic!("the statement at {at}: {}", e.message));
    }
}

#[test]
fn the_refused_keywords_really_refuse() {
    // The block says `UNION`, `INTERSECT`, `EXCEPT`, `EXISTS` and `NOT EXISTS`
    // are refused by name with a reason. `sql::grammar` checks the
    // documentation mentions them; this checks the parser does.
    for text in [
        "SELECT * FROM books UNION SELECT * FROM books",
        "SELECT * FROM books INTERSECT SELECT * FROM books",
        "SELECT * FROM books EXCEPT SELECT * FROM books",
        "SELECT * FROM books WHERE EXISTS (SELECT id FROM authors)",
        "SELECT * FROM books WHERE NOT EXISTS (SELECT id FROM authors)",
    ] {
        let message = refuse_two(text);
        assert!(
            message.contains("not supported"),
            "{text} should be refused with a reason, got: {message}"
        );
    }
}
