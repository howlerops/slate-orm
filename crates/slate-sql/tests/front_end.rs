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
use slate_sql::sql::{COMPARISONS, Schema, Statement, parse};
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

/// Every spelling in `COMPARISONS` reaches the parser as one operator.
///
/// The table is the parser's vocabulary — `comparison_tail` loops over it and
/// `crates/slate-wasm/tests/sql.rs` generates from it — and until this test
/// nothing ran a query for each row. That left one list able to drift from
/// the code: `lex` used to decide which symbols are two characters with a
/// hand-written `matches!("<=" | ">=" | "!=" | "<>")`, so a fifth
/// two-character spelling added to the table would lex as two tokens and
/// `eat_symbol` would match its first character against some other row.
/// `lex` now reads the table, and this is what says the reading works:
/// dropping a spelling from `lex`'s arm used to change nothing here because
/// nothing here existed.
#[test]
fn every_comparison_the_table_declares_parses_as_the_operator_it_names() {
    for one in COMPARISONS {
        // The string operators need the string column; `year >= 'a'` would
        // fail for a reason that is not this test's.
        let (column, ordinal, value) = match one.op {
            "like" | "ilike" | "matches" | "contains" => ("title", 2, "'a'"),
            _ => ("year", 3, "1970"),
        };
        let text = format!(
            "SELECT * FROM books WHERE {column} {} {value}",
            one.spelling
        );
        let spec = select(&text);
        assert_eq!(spec.filters.len(), 1, "{text}");
        assert_eq!(spec.filters[0].column, ordinal, "{text}");
        assert_eq!(spec.filters[0].op, one.op, "{text}");
    }
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

// --- parentheses -----------------------------------------------------------

/// Evaluate a lowered `WHERE` against one row of `books`.
fn admits(text: &str, author_id: u64, year: i64) -> bool {
    use slate_kernel::Truth;
    let spec = select(text);
    let filter = lower::build(&spec, &books())
        .unwrap_or_else(|e| panic!("{text}: {e}"))
        .filter;
    let row = slate_schema::Row::new(vec![
        Value::U64(1),
        Value::U64(author_id),
        Value::Str("t".to_owned()),
        Value::I64(year),
    ]);
    filter.evaluate(&row) == Truth::True
}

#[test]
fn a_bracketed_disjunction_is_anded_with_what_follows_it() {
    // The query the front end could not write: two arms that are not a single
    // column's values, so `IN (…)` does not cover it, ANDed with a third
    // condition. `ledger/2026-09-25-the-disjunction-the-kernel-always-had.md`
    // recorded it as `No parentheses, so no nesting`.
    let text = "SELECT * FROM books WHERE (author_id = 1 OR year >= 1990) AND year < 2000";

    assert!(admits(text, 1, 1970), "left arm, and inside the range");
    assert!(admits(text, 9, 1995), "right arm, and inside the range");
    assert!(!admits(text, 1, 2005), "left arm, outside the range");
    assert!(!admits(text, 9, 1970), "neither arm");
    // And the whole thing is not just the trailing conjunct, which is what a
    // lowering that dropped the bracket would produce.
    assert!(
        !admits(text, 9, 1980),
        "a row the trailing conjunct alone admits"
    );
}

#[test]
fn the_other_nesting_works_too() {
    // `a AND (b OR c)` — the reading the refusal says half of everyone takes,
    // now writable. Distinguished from the case above by a row the two
    // disagree on: author 1 in 2005 is admitted by `(a OR b) AND c` reading
    // nothing and by this one only if `c` holds.
    let text = "SELECT * FROM books WHERE author_id = 1 AND (year >= 1990 OR year < 1950)";
    assert!(admits(text, 1, 1995));
    assert!(admits(text, 1, 1940));
    assert!(!admits(text, 1, 1970), "between the arms");
    assert!(!admits(text, 2, 1995), "wrong author");
}

#[test]
fn a_flat_where_never_lands_in_the_nested_field() {
    // The invariant that makes three fields safe rather than three ways to
    // say one thing. A redundant bracket is the case that would break it
    // if the parser switched on having seen a `(` rather than flattening.
    for text in [
        "SELECT * FROM books WHERE year >= 1970",
        "SELECT * FROM books WHERE (year >= 1970)",
        "SELECT * FROM books WHERE ((year >= 1970))",
        "SELECT * FROM books WHERE year >= 1970 AND author_id = 1",
        "SELECT * FROM books WHERE (year >= 1970) AND (author_id = 1)",
        "SELECT * FROM books WHERE year >= 1970 OR author_id = 1",
        "SELECT * FROM books WHERE (year >= 1970 OR author_id = 1)",
    ] {
        let spec = select(text);
        assert!(
            spec.predicate.is_none(),
            "{text} produced a nested predicate: {:?}",
            spec.predicate
        );
        assert!(
            spec.filters.is_empty() || spec.any_of.is_empty(),
            "{text} populated both flat lists"
        );
    }
    // And a bracket that really nests populates exactly the one field.
    let spec = select("SELECT * FROM books WHERE (author_id = 1 OR year >= 1990) AND year < 2000");
    assert!(spec.predicate.is_some());
    assert!(spec.filters.is_empty(), "{:?}", spec.filters);
    assert!(spec.any_of.is_empty(), "{:?}", spec.any_of);
}

#[test]
fn a_redundant_bracket_produces_the_same_spec_as_no_bracket() {
    // Stronger than "not nested": byte for byte the same spec, so the Spec
    // tab shows one thing and a view stored as text resolves to one query
    // whichever way its author wrote it.
    assert_eq!(
        select("SELECT * FROM books WHERE year >= 1970"),
        select("SELECT * FROM books WHERE (year >= 1970)"),
    );
    assert_eq!(
        select("SELECT * FROM books WHERE year >= 1970 AND author_id = 1"),
        select("SELECT * FROM books WHERE (year >= 1970) AND (author_id = 1)"),
    );
}

#[test]
fn mixing_without_brackets_is_still_refused_and_the_message_says_to_write_them() {
    // The refusal predates parentheses and is kept: `a AND b OR c` reads two
    // ways and a reader should not have to know which this front end picked.
    // What changed is that the fix now exists, so the message names it.
    let tables = [books()];
    for text in [
        "SELECT * FROM books WHERE year >= 1970 AND author_id = 3 OR author_id = 4",
        "SELECT * FROM books WHERE year >= 1970 OR author_id = 3 AND author_id = 4",
    ] {
        let error = parse(text, &Schema(&tables)).expect_err("a bare mixture must be refused");
        assert!(
            error.message.contains("cannot be mixed"),
            "{text}: {}",
            error.message
        );
        assert!(
            error.message.contains("parentheses") || error.message.contains("brackets"),
            "the refusal should point at the fix that now exists: {}",
            error.message
        );
    }
}

#[test]
fn an_unclosed_bracket_is_refused_at_the_bracket_and_not_swallowed() {
    let tables = [books()];
    let error = parse(
        "SELECT * FROM books WHERE (year >= 1970 AND author_id = 1",
        &Schema(&tables),
    )
    .expect_err("an unclosed bracket must be refused");
    assert!(error.message.contains(')'), "{}", error.message);
}

#[test]
fn a_nested_predicate_survives_the_json_round_trip() {
    // The Spec tab serializes and the browser deserializes, and `predicate` is
    // the first recursive field in this shape.
    let spec = select("SELECT * FROM books WHERE (author_id = 1 OR year >= 1990) AND year < 2000");
    let json = serde_json::to_string(&spec).expect("a serializable spec");
    assert!(json.contains("predicate"), "{json}");
    let back: QuerySpec = serde_json::from_str(&json).expect("a deserializable spec");
    assert_eq!(spec, back);
}

#[test]
fn a_flat_spec_serializes_exactly_as_it_did_before_predicate_existed() {
    // `skip_serializing_if` earns its place here: every spec anyone has stored
    // is flat, and a new always-present `"predicate": null` would change every
    // one of them and every test that compares JSON.
    let json = serde_json::to_string(&select("SELECT * FROM books WHERE year >= 1970"))
        .expect("a serializable spec");
    assert!(!json.contains("predicate"), "{json}");
}

#[test]
fn two_bracketed_conjunctions_ored_together() {
    // A disjunction whose *parts* nest, which is the other arm of the
    // flattener: the cases above all nest under an `AND`, so the `Any` branch
    // was unexercised in this crate and a mutation that made it return an
    // empty list survived. `slate-wasm`'s round trip had a case; this crate
    // did not, and "covered somewhere" is not covered here.
    let text = "SELECT * FROM books WHERE (author_id = 1 AND year >= 1990) \
                OR (author_id = 2 AND year < 1950)";
    let spec = select(text);
    match spec.predicate.as_ref().expect("this nests") {
        slate_sql::PredicateSpec::Any(parts) => {
            assert_eq!(parts.len(), 2, "two arms: {parts:?}");
            for part in parts {
                assert!(
                    matches!(part, slate_sql::PredicateSpec::All(inner) if inner.len() == 2),
                    "each arm is a two-part conjunction: {part:?}"
                );
            }
        }
        other => panic!("expected a disjunction of conjunctions, got {other:?}"),
    }

    assert!(admits(text, 1, 1995), "the first arm");
    assert!(admits(text, 2, 1940), "the second arm");
    assert!(
        !admits(text, 1, 1940),
        "author of one arm, year of the other"
    );
    assert!(!admits(text, 2, 1995), "and the other way round");
    assert!(!admits(text, 3, 1970), "neither");
}

// --- WITH: a non-recursive CTE referenced once -----------------------------
//
// `docs/ctes.md` splits `WITH` into three shapes with three answers, and the
// one that is buildable is a non-recursive CTE read exactly once: it inlines
// into the outer spec, which is the same expansion `docs/views.md` says a view
// must be. These cases pin the expansion and the four questions the note left
// open — a name that collides with a table, a column the body projected away,
// a body that joins, and a second CTE in one `WITH` — each of which is
// answered by a refusal and has to stay one.
//
// The property worth the most is the first: the CTE's spec and the inlined
// query's spec are *equal*, not merely equivalent. Anything weaker would let
// the two drift into two plans for one query, which is the whole thing this
// front end exists not to do.

/// The message a statement is refused with, against `books` alone.
fn refuse(text: &str) -> String {
    let tables = [books()];
    parse(text, &Schema(&tables))
        .err()
        .unwrap_or_else(|| panic!("{text} was accepted and it must be refused"))
        .message
}

#[test]
fn a_single_reference_cte_is_the_spec_the_inlined_query_compiles_to() {
    // `docs/ctes.md` §3, verbatim: the note asserts these two are the same
    // query and could not run it. They are the same *spec*, which is stronger
    // than "the same answer" — one plan, byte for byte.
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) \
             SELECT title FROM recent WHERE id > 5"
        ),
        select("SELECT title FROM books WHERE year > 2000 AND id > 5"),
    );
}

#[test]
fn the_expanded_spec_names_the_base_table_and_never_the_cte() {
    // The security property, checked rather than argued. A `QuerySpec` is what
    // leaves this crate, and `slate-serverd` resolves `spec.table` against
    // `Catalog::table_by_name` — so if the CTE's name could reach that field,
    // a grant and a policy would key on a name the catalog does not have.
    // `docs/ctes.md` names registering one under a synthetic `TableId` as the
    // hazard; this is the assertion that the expansion never gets near it.
    let spec = select(
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) \
         SELECT title FROM recent WHERE id > 5",
    );
    assert_eq!(spec.table, "books");
    let json = serde_json::to_string(&spec).expect("a serializable spec");
    assert!(
        !json.contains("recent"),
        "the CTE's name reached the spec: {json}"
    );
}

#[test]
fn the_expanded_spec_lowers_to_a_kernel_query_with_both_filters() {
    // "Nothing new reaches the kernel" — the note's claim, run. The expansion
    // adds no plan node: it is the `Query` the inlined statement builds.
    let spec = select(
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) \
         SELECT title FROM recent WHERE id > 5",
    );
    let query = lower::build(&spec, &books()).expect("a valid lowering");
    let inlined = lower::build(
        &select("SELECT title FROM books WHERE year > 2000 AND id > 5"),
        &books(),
    )
    .expect("a valid lowering");
    assert_eq!(query.filter, inlined.filter);
    assert_eq!(
        query.filter,
        Expr::all([
            Expr::compare(Ordinal(3), CmpOp::Gt, Value::I64(2000)),
            Expr::compare(Ordinal(0), CmpOp::Gt, Value::U64(5)),
        ])
    );
}

#[test]
fn a_cte_body_with_no_where_contributes_nothing_to_the_filter() {
    assert_eq!(
        select(
            "WITH all_books AS (SELECT id, title FROM books) SELECT title FROM all_books WHERE id > 5"
        ),
        select("SELECT title FROM books WHERE id > 5"),
    );
}

#[test]
fn an_outer_query_with_no_where_of_its_own_keeps_the_ctes() {
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) SELECT title FROM recent"
        ),
        select("SELECT title FROM books WHERE year > 2000"),
    );
}

#[test]
fn a_disjunction_in_a_cte_body_is_bracketed_by_the_merge() {
    // The merge is a conjunction and the CTE's `WHERE` may be a disjunction,
    // so the result has to be `(a OR b) AND c` and not `a OR b AND c` — which
    // this front end refuses to read at all, and which SQL reads the other
    // way. Equality against the bracketed spelling is what says the merge
    // bracketed rather than concatenated.
    assert_eq!(
        select(
            "WITH odd AS (SELECT id, title, year FROM books WHERE author_id = 1 OR year >= 1990) \
             SELECT title FROM odd WHERE year < 2000"
        ),
        select("SELECT title FROM books WHERE (author_id = 1 OR year >= 1990) AND year < 2000"),
    );
    // And the other way round: a flat CTE body under a bracketed outer clause.
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title, year, author_id FROM books WHERE year > 2000) \
             SELECT title FROM recent WHERE author_id = 1 OR year >= 1990"
        ),
        select("SELECT title FROM books WHERE year > 2000 AND (author_id = 1 OR year >= 1990)"),
    );
}

#[test]
fn the_outer_query_may_order_group_and_limit_through_a_cte() {
    // Everything after the `WHERE` belongs to the outer query and is untouched
    // by the merge, which is only true because a CTE body may not itself sort,
    // group or limit — see the refusal below.
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title, author_id FROM books WHERE year > 2000) \
             SELECT author_id, count(*) FROM recent GROUP BY author_id ORDER BY count(*) DESC LIMIT 3"
        ),
        select(
            "SELECT author_id, count(*) FROM books WHERE year > 2000 \
             GROUP BY author_id ORDER BY count(*) DESC LIMIT 3"
        ),
    );
}

#[test]
fn a_star_over_a_cte_is_the_columns_the_cte_projects() {
    // `SELECT *` from a CTE is the CTE's columns, which are not the base
    // table's. Expanding it to an empty projection — "every column" — would
    // hand back two columns the CTE had removed, silently.
    let spec = select(
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) SELECT * FROM recent",
    );
    assert_eq!(
        spec.columns,
        vec![0, 2],
        "id and title, and not year or author_id"
    );
    assert_eq!(
        spec,
        select("SELECT id, title FROM books WHERE year > 2000"),
    );
    // A body that projects nothing means every column there too, so the outer
    // star is still an empty projection.
    let star = select("WITH everything AS (SELECT * FROM books) SELECT * FROM everything");
    assert!(star.columns.is_empty(), "{:?}", star.columns);
}

#[test]
fn a_column_the_cte_projected_away_is_not_in_scope() {
    // Decision 2. The CTE's projection *is* the outer query's namespace, which
    // is what SQL says and the only reading under which `SELECT *` above has
    // an answer. Refused rather than resolved against the base table, because
    // resolving it would make the projection a suggestion.
    for text in [
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) SELECT year FROM recent",
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) SELECT title FROM recent WHERE year > 2010",
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) SELECT title FROM recent ORDER BY year",
    ] {
        let message = refuse(text);
        assert!(
            message.contains("recent") && message.contains("year"),
            "the refusal should name the CTE and the column: {message}"
        );
        // And it lists what the CTE *does* have, rather than the base table's
        // columns — naming `year` as available is exactly the confusion.
        assert!(
            !message.contains("author_id"),
            "the refusal listed a column the CTE does not project: {message}"
        );
    }
    // A name that is nowhere at all is still refused against the CTE's name.
    let message = refuse("WITH recent AS (SELECT id FROM books) SELECT nosuch FROM recent");
    assert!(message.contains("recent"), "{message}");
}

#[test]
fn the_ctes_name_is_the_qualifier_and_the_base_tables_is_not() {
    // After `FROM recent`, `recent` is what is in scope: `recent.title` is the
    // column and `books.title` is a table this statement never named.
    assert_eq!(
        select("WITH recent AS (SELECT id, title FROM books) SELECT recent.title FROM recent"),
        select("SELECT title FROM books"),
    );
    let message =
        refuse("WITH recent AS (SELECT id, title FROM books) SELECT books.title FROM recent");
    assert!(message.contains("books.title"), "{message}");
}

#[test]
fn a_cte_name_that_is_already_a_table_is_refused() {
    // Decision 1. SQL would shadow the table; here the expansion leaves
    // `spec.table` saying `books` either way, so a reader looking at the one
    // artifact this front end shows them could not tell which they got.
    let message =
        refuse("WITH books AS (SELECT id FROM books WHERE year > 2000) SELECT id FROM books");
    assert!(message.contains("books"), "{message}");
    assert!(
        message.contains("table"),
        "the refusal should say the name is a table's: {message}"
    );
}

#[test]
fn a_cte_whose_body_is_a_join_is_refused() {
    // Decision 3, which `docs/ctes.md` left explicitly unexamined. An
    // ungrouped join returns whole rows and takes `SELECT *` and nothing else
    // — so a projection over a joined body is a projection in the joined row's
    // ordinal space, which is the refusal the join path already makes.
    let message = refuse_two(
        "WITH pairs AS (SELECT * FROM books JOIN authors ON books.author_id = authors.id) \
         SELECT * FROM pairs",
    );
    assert!(message.contains("one table"), "{message}");
    assert!(message.contains("docs/ctes.md"), "{message}");
}

#[test]
fn a_second_cte_in_one_with_is_refused() {
    // Decision 4. The outer query reads one table, so at most one CTE can be
    // the one it reads: a second is either never read, or read by the first —
    // which is a CTE over a CTE, the nesting `docs/views.md` §5 refuses for
    // views and for the same reasons.
    for text in [
        "WITH a AS (SELECT id FROM books), b AS (SELECT id FROM books) SELECT id FROM a",
        "WITH a AS (SELECT id FROM books) WITH b AS (SELECT id FROM books) SELECT id FROM b",
    ] {
        let message = refuse(text);
        assert!(
            message.contains("one CTE"),
            "the refusal should say only one CTE is read: {message}"
        );
        assert!(message.contains("docs/ctes.md"), "{message}");
    }
}

#[test]
fn a_cte_nobody_reads_is_refused() {
    // The same argument decision 4 rests on, in its smallest form: a `WITH`
    // whose binding the statement never names is dead text that reads as
    // meaningful. Silently ignoring it is how a typo in the reference becomes
    // a full-table scan that looks deliberate.
    let message =
        refuse("WITH recent AS (SELECT id FROM books WHERE year > 2000) SELECT id FROM books");
    assert!(message.contains("recent"), "{message}");
    assert!(
        message.contains("never read") || message.contains("not read"),
        "{message}"
    );
}

#[test]
fn with_recursive_is_still_refused_and_the_reason_is_the_fixpoint() {
    // The refusal `docs/ctes.md` §1 argues, kept intact: a fixpoint's trip
    // count is in the data and a statement compiles to one plan.
    let message = refuse("WITH RECURSIVE t AS (SELECT id FROM books) SELECT id FROM t");
    assert!(message.contains("fixpoint"), "{message}");
    assert!(message.contains("docs/ctes.md"), "{message}");
}

#[test]
fn a_cte_read_anywhere_but_the_statements_own_from_is_refused() {
    // The other refusal kept: §2's read-it-twice, and the reads that are once
    // but not from the top-level `FROM`. Both would need the CTE materialised
    // or its `WHERE` merged into a side of a join, which is a second plan.
    for text in [
        // Read twice — the note's own example.
        "WITH recent AS (SELECT id FROM books WHERE year > 2000) \
         SELECT * FROM recent JOIN recent AS other ON recent.id = other.id",
        // Read once, from a join input on the right.
        "WITH recent AS (SELECT id FROM books WHERE year > 2000) \
         SELECT * FROM books JOIN recent ON books.id = recent.id",
        // And on the *left*, which is the case that reaches no other guard:
        // `first_input` has already consumed the name, so `table` never sees
        // it, and without the refusal in `select` the statement would be
        // planned as an ordinary join with the CTE's WHERE silently dropped.
        "WITH recent AS (SELECT id FROM books WHERE year > 2000) \
         SELECT * FROM recent JOIN authors ON recent.author_id = authors.id",
        // Read once, from a subquery.
        "WITH recent AS (SELECT id FROM books WHERE year > 2000) \
         SELECT id FROM books WHERE id IN (SELECT id FROM recent)",
    ] {
        // Two tables, so the join cases are refused for being a CTE read
        // rather than for naming a table this schema does not have.
        let message = refuse_two(text);
        assert!(message.contains("recent"), "{text}: {message}");
        assert!(message.contains("docs/ctes.md"), "{text}: {message}");
    }
}

#[test]
fn a_subquery_beside_a_cte_resolves_against_its_own_table() {
    // The CTE's namespace is the *outer* query's and does not reach inside an
    // `IN (SELECT …)`, even when the subquery reads the CTE's own base table.
    // Without that, `year` below would be checked against what the CTE
    // projects and refused for being a column of a table the subquery named
    // directly.
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) \
             SELECT title FROM recent WHERE id IN (SELECT year FROM books)"
        ),
        select("SELECT title FROM books WHERE year > 2000 AND id IN (SELECT year FROM books)"),
    );
}

#[test]
fn a_cte_body_may_not_sort_group_or_page() {
    // Everything the merge cannot carry, refused by name. A `LIMIT` is the one
    // that would be wrong rather than merely unsupported: the outer `WHERE`
    // merges *into* the body, so it would filter before the limit rather than
    // after it, and the query would quietly mean something else.
    for clause in [
        "WITH recent AS (SELECT id FROM books WHERE year > 2000 LIMIT 10) SELECT id FROM recent",
        "WITH recent AS (SELECT id FROM books ORDER BY year) SELECT id FROM recent",
        "WITH recent AS (SELECT author_id FROM books GROUP BY author_id) SELECT author_id FROM recent",
        "WITH recent AS (SELECT id FROM books OFFSET 4) SELECT id FROM recent",
        "WITH recent AS (SELECT count(*) FROM books) SELECT * FROM recent",
    ] {
        let message = refuse(clause);
        assert!(message.contains("docs/ctes.md"), "{clause}: {message}");
    }
}

#[test]
fn the_with_line_parses_what_the_grammar_block_describes() {
    // The grammar block's `WITH` production, exercised the way every other
    // line of it is: a case that parses the thing the prose claims.
    select(
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) SELECT title FROM recent WHERE id > 5",
    );
    select("WITH everything AS (SELECT * FROM books) SELECT id FROM everything");
    // Lower case, because the dispatcher matches a normalised word.
    select("with recent as (select id from books) select id from recent");
}

#[test]
fn an_aggregate_over_a_cte_resolves_in_the_ctes_namespace_too() {
    // The one path that went round `resolve`: the single-table aggregate
    // resolved its argument against the table's name with no visibility, so
    // `count(year)` would have read a column the body projected away while
    // the bare `year` beside it was refused. A namespace with one hole in it
    // is worse than none, because the hole is where somebody stops looking.
    let message = refuse(
        "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) \
         SELECT count(year) FROM recent",
    );
    assert!(message.contains("recent"), "{message}");
    assert!(message.contains("year"), "{message}");
    // And the aggregate over a column the CTE *does* project is the inlined
    // query's spec, so the fix did not close the path along with the hole.
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title FROM books WHERE year > 2000) \
             SELECT count(id) FROM recent"
        ),
        select("SELECT count(id) FROM books WHERE year > 2000"),
    );
}

#[test]
fn a_cte_that_reads_itself_is_a_recursive_cte_without_the_word() {
    // Refused before the name reaches the catalog, which would report a
    // declared binding as an unknown table — the one thing it certainly is
    // not. `docs/views.md` §5 makes the same refusal for a view that reads
    // itself and for the same reason.
    let message = refuse("WITH loop_ AS (SELECT id FROM loop_) SELECT id FROM loop_");
    assert!(message.contains("reads itself"), "{message}");
    assert!(message.contains("fixpoint"), "{message}");
}

#[test]
fn a_cte_reference_takes_no_alias_and_the_name_takes_no_column_list() {
    // Both refusals name what the reader gets instead, because both are
    // things SQL allows and this does not.
    let aliased = refuse("WITH recent AS (SELECT id FROM books) SELECT id FROM recent AS r");
    assert!(aliased.contains("alias"), "{aliased}");
    let renamed = refuse("WITH recent(a) AS (SELECT id FROM books) SELECT a FROM recent");
    assert!(renamed.contains("select list"), "{renamed}");
}

#[test]
fn a_with_in_front_of_a_write_is_refused_and_says_why_there_is_nothing_to_read() {
    let message = refuse("WITH recent AS (SELECT id FROM books) DELETE FROM books WHERE id = 1");
    assert!(message.contains("one SELECT"), "{message}");
}

#[test]
fn a_conjunction_on_either_side_of_the_merge_stays_flat() {
    // Found by a surviving mutation, and it is the mutation the merge exists
    // for. Every case above has a single condition on each side, so the merge
    // was only ever joining two leaves — and a version that wrapped each side
    // instead of splicing it produced the same specs and passed all of them.
    // Two ANDed conditions on either side is what tells the two apart: spliced
    // they are one `filters` list, wrapped they are an `All` of `All`s, which
    // is the one shape `flatten` cannot flatten and which lands in
    // `predicate` instead. Same query, different spec, and this front end's
    // whole claim is that there is one spec per query.
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title, year, author_id FROM books \
             WHERE year > 2000 AND author_id = 1) \
             SELECT title FROM recent WHERE id > 5"
        ),
        select("SELECT title FROM books WHERE year > 2000 AND author_id = 1 AND id > 5"),
    );
    assert_eq!(
        select(
            "WITH recent AS (SELECT id, title, year, author_id FROM books WHERE year > 2000) \
             SELECT title FROM recent WHERE id > 5 AND author_id = 1"
        ),
        select("SELECT title FROM books WHERE year > 2000 AND id > 5 AND author_id = 1"),
    );
    // And the flat list really is flat, rather than the two nesting the same
    // way and comparing equal.
    let spec = select(
        "WITH recent AS (SELECT id, title, year, author_id FROM books \
         WHERE year > 2000 AND author_id = 1) \
         SELECT title FROM recent WHERE id > 5",
    );
    assert_eq!(spec.filters.len(), 3, "{:?}", spec.filters);
    assert!(spec.predicate.is_none(), "{:?}", spec.predicate);
}

// A repeated column in a CTE's projection is accepted, and projects that column
// twice — which is what the same projection does without a CTE. `SELECT id, id`
// is legal SQL returning two columns, so the expansion agreeing with the direct
// form is the answer, not a shape that needed refusing.
//
// The implementation left this undecided and nothing had established what it
// did. Establishing it is what made the question answerable: the two forms are
// equal, so there is one rule here rather than two.
#[test]
fn a_repeated_column_in_a_cte_projects_it_twice_just_as_it_does_directly() {
    let through_cte = select("WITH d AS (SELECT id, id FROM books) SELECT * FROM d");
    assert_eq!(through_cte.columns, vec![0, 0], "id, twice");
    assert_eq!(
        through_cte,
        select("SELECT id, id FROM books"),
        "the expansion is the query it inlines into, repeated column and all"
    );
}

// --- brackets in a HAVING ----------------------------------------------------

/// Evaluate a lowered `HAVING` against one group of `books` by author.
///
/// `SELECT author_id, count(*) ... GROUP BY author_id` puts the key at group
/// ordinal 0 and the count at 1, which is the space a `HAVING` names. Built
/// from the parsed spec's own keys and aggregates rather than from constants,
/// so a lowering that resolved the ordinals differently would be caught here
/// rather than agreeing with a hand-written copy of its own mistake.
fn group_admits(text: &str, author_id: u64, count: i64) -> bool {
    use slate_kernel::Truth;
    let spec = select(text);
    let keys: Vec<Ordinal> = spec.group_by.iter().map(|c| Ordinal(*c as usize)).collect();
    let aggregates: Vec<_> = spec
        .aggregates
        .iter()
        .map(|a| {
            lower::aggregate_of(&a.kind, Ordinal(a.column as usize))
                .unwrap_or_else(|e| panic!("{text}: {e}"))
        })
        .collect();
    let table = books();
    let expr = lower::group_predicate(
        &spec.having,
        &spec.having_any_of,
        spec.having_predicate.as_ref(),
        &keys,
        &aggregates,
        &[&table],
    )
    .unwrap_or_else(|e| panic!("{text}: {e}"));
    let group = slate_schema::Row::new(vec![Value::U64(author_id), Value::I64(count)]);
    expr.evaluate(&group) == Truth::True
}

/// The grouped statement these cases all filter, so each one is its `HAVING`.
const BY_AUTHOR: &str = "SELECT author_id, count(*) FROM books GROUP BY author_id HAVING ";

#[test]
fn a_bracketed_disjunction_in_a_having_is_anded_with_what_follows_it() {
    // The `WHERE` case's twin, in group space. `ledger/2026-09-25-or-in-having-too.md`
    // recorded this as `a HAVING still takes no brackets`, and the reason given
    // was that the group-condition lists have no nested form to lower — which
    // was true of the spec and never of the kernel: `Grouping::having` has been
    // an `Expr` tree since it was written.
    let text = format!("{BY_AUTHOR}(count(*) > 5 OR author_id = 1) AND count(*) < 100");

    assert!(group_admits(&text, 9, 40), "left arm, under the ceiling");
    assert!(group_admits(&text, 1, 2), "right arm, under the ceiling");
    assert!(!group_admits(&text, 9, 200), "left arm, over the ceiling");
    assert!(!group_admits(&text, 9, 2), "neither arm");
    // And not merely the trailing conjunct, which is what dropping the bracket
    // would leave.
    assert!(
        !group_admits(&text, 7, 3),
        "a group the trailing conjunct alone admits"
    );
}

#[test]
fn the_other_having_nesting_works_too() {
    // `a AND (b OR c)` over groups — the reading the mixing refusal says half
    // of everyone takes, now writable in a HAVING as it already was in a WHERE.
    let text = format!("{BY_AUTHOR}author_id = 1 AND (count(*) > 100 OR count(*) < 3)");
    assert!(group_admits(&text, 1, 500));
    assert!(group_admits(&text, 1, 1));
    assert!(!group_admits(&text, 1, 50), "between the arms");
    assert!(!group_admits(&text, 2, 500), "wrong key");
}

#[test]
fn a_flat_having_never_lands_in_the_nested_field() {
    // The invariant that makes three fields safe rather than three ways to say
    // one thing, asserted for the HAVING exactly as
    // `a_flat_where_never_lands_in_the_nested_field` asserts it for the WHERE.
    for tail in [
        "count(*) > 5",
        "(count(*) > 5)",
        "((count(*) > 5))",
        "count(*) > 5 AND author_id = 1",
        "(count(*) > 5) AND (author_id = 1)",
        "count(*) > 5 OR author_id = 1",
        "(count(*) > 5 OR author_id = 1)",
    ] {
        let text = format!("{BY_AUTHOR}{tail}");
        let spec = select(&text);
        assert!(
            spec.having_predicate.is_none(),
            "{tail} produced a nested HAVING: {:?}",
            spec.having_predicate
        );
        assert!(
            spec.having.is_empty() || spec.having_any_of.is_empty(),
            "{tail} populated both flat HAVING lists"
        );
    }
    // And a bracket that really nests populates exactly the one field.
    let spec = select(&format!(
        "{BY_AUTHOR}(count(*) > 5 OR author_id = 1) AND count(*) < 100"
    ));
    assert!(spec.having_predicate.is_some());
    assert!(spec.having.is_empty(), "{:?}", spec.having);
    assert!(spec.having_any_of.is_empty(), "{:?}", spec.having_any_of);
}

#[test]
fn a_redundant_bracket_in_a_having_produces_the_same_spec_as_no_bracket() {
    // Byte for byte, so a view stored as text resolves to one query whichever
    // way its author bracketed it.
    assert_eq!(
        select(&format!("{BY_AUTHOR}count(*) > 5")),
        select(&format!("{BY_AUTHOR}(count(*) > 5)")),
    );
    assert_eq!(
        select(&format!("{BY_AUTHOR}count(*) > 5 AND author_id = 1")),
        select(&format!("{BY_AUTHOR}(count(*) > 5) AND (author_id = 1)")),
    );
}

#[test]
fn mixing_connectives_in_a_having_without_brackets_is_still_refused() {
    // Unchanged, and now the message's advice is actionable: before this the
    // refusal told the reader to write brackets a HAVING would also refuse.
    let error = parse(
        &format!("{BY_AUTHOR}count(*) > 5 AND author_id = 1 OR count(*) < 2"),
        &Schema(&[books()]),
    )
    .expect_err("a bare mixture in a HAVING must be refused");
    assert!(
        error.message.contains("HAVING"),
        "the refusal must name the clause it came from: {}",
        error.message
    );
    assert!(
        error.message.contains("parentheses") || error.message.contains("brackets"),
        "and point at the fix: {}",
        error.message
    );
}

#[test]
fn an_unclosed_bracket_in_a_having_says_having_and_not_where() {
    // The bracket parser is now shared between the two clauses, which is the
    // whole point — and is exactly how the unclosed-bracket message comes to
    // name the wrong one. It is parameterised rather than hard-coded.
    let error = parse(
        &format!("{BY_AUTHOR}(count(*) > 5 OR author_id = 1"),
        &Schema(&[books()]),
    )
    .expect_err("an unclosed bracket must be refused");
    assert!(
        error.message.contains("HAVING"),
        "an unclosed bracket in a HAVING must say HAVING: {}",
        error.message
    );
}

#[test]
fn a_nested_having_survives_the_json_round_trip() {
    // The Spec tab serializes and the browser deserializes.
    let spec = select(&format!(
        "{BY_AUTHOR}(count(*) > 5 OR author_id = 1) AND count(*) < 100"
    ));
    let json = serde_json::to_string(&spec).expect("serializable");
    assert!(json.contains("havingPredicate"), "{json}");
    let back: QuerySpec = serde_json::from_str(&json).expect("deserializable");
    assert_eq!(spec, back);
}

#[test]
fn a_flat_having_serializes_exactly_as_it_did_before_having_predicate_existed() {
    // A new always-present `"havingPredicate": null` would change every spec
    // JSON on disk and every fixture that compares one.
    let json =
        serde_json::to_string(&select(&format!("{BY_AUTHOR}count(*) > 5"))).expect("serializable");
    assert!(!json.contains("havingPredicate"), "{json}");
}

#[test]
fn a_bracketed_having_reaches_a_join_and_a_chain() {
    // The module documentation claims a HAVING behaves the same "on a single
    // table, a join and a chain alike". Both wider shapes parse through a
    // different function from the single-table one, which is where the two
    // came to disagree about `OR` and would come to disagree about brackets.
    let tables = [books(), authors()];
    let join = parse(
        "SELECT a.name, count(*) FROM books JOIN authors AS a ON books.author_id = a.id \
         GROUP BY a.name HAVING (count(*) > 5 OR count(*) < 2) AND count(*) <> 3",
        &Schema(&tables),
    )
    .unwrap_or_else(|e| panic!("a bracketed HAVING on a join: {}", e.message));
    match join.statement {
        Statement::Join(spec) => assert!(
            spec.having_predicate.is_some(),
            "the join's HAVING should have nested"
        ),
        other => panic!("expected a join, got {other:?}"),
    }

    let chain = parse(
        "SELECT a.name, count(*) FROM books \
         JOIN authors AS a ON books.author_id = a.id \
         JOIN authors AS b ON books.author_id = b.id \
         GROUP BY a.name HAVING (count(*) > 5 OR count(*) < 2) AND count(*) <> 3",
        &Schema(&tables),
    )
    .unwrap_or_else(|e| panic!("a bracketed HAVING on a chain: {}", e.message));
    match chain.statement {
        Statement::Chain(spec) => assert!(
            spec.having_predicate.is_some(),
            "the chain's HAVING should have nested"
        ),
        other => panic!("expected a chain, got {other:?}"),
    }
}

#[test]
fn an_ored_single_table_having_is_a_disjunction_and_not_a_conjunction() {
    // `an_ored_having_reaches_a_chain` covers the chain and the browser's
    // suite covers the binding; this crate's own surface had no single-table
    // case, and a mutation putting the ORed terms into the ANDed field
    // survived the whole file. It is the difference between "either" and
    // "both", which no assertion about *which list is empty* can see —
    // `a_flat_having_never_lands_in_the_nested_field` passes either way.
    let text = format!("{BY_AUTHOR}count(*) > 100 OR author_id = 1");
    let spec = select(&text);
    assert_eq!(spec.having_any_of.len(), 2, "both arms should be disjuncts");
    assert!(spec.having.is_empty(), "{:?}", spec.having);

    // And it means either, in the lowering rather than only in the spec.
    assert!(group_admits(&text, 1, 2), "the key arm alone");
    assert!(group_admits(&text, 9, 500), "the count arm alone");
    assert!(!group_admits(&text, 9, 2), "neither arm");
}
