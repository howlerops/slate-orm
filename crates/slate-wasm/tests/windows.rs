//! `OVER`, which this front end refuses — and the refusal is unusual.
//!
//! Every other named refusal here (`WITH`, `UNION`, `CREATE`) says the thing
//! cannot be *done*. This one says the opposite: the kernel has the operator,
//! with `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD` and any aggregate
//! over a partition — what it has no way of doing is being *asked*, because
//! neither `QuerySpec` nor the gRPC protocol has a window. "Built and
//! unreachable" and "not built" are different facts with different next steps,
//! and a message that blurred them would send a reader to write the operator
//! that already exists.
//!
//! Without this the failure is `unknown function ROW_NUMBER` at the wrong
//! token — true, and it answers a question nobody asked.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value as Json;
use slate_wasm::Playground;

fn refusal(playground: &Playground, sql: &str) -> String {
    let out: Vec<Json> = serde_json::from_str(&playground.sql(sql)).expect("json");
    let got = out.into_iter().next().expect("one statement");
    got["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("`{sql}` was not refused: {got}"))
        .to_owned()
}

/// Every spelling reaches the named refusal rather than a token complaint.
#[test]
fn over_is_refused_by_name_and_not_as_an_unknown_function() {
    let playground = Playground::new();
    for sql in [
        "SELECT id, ROW_NUMBER() OVER (PARTITION BY author_id ORDER BY year) FROM books",
        // Lower case, because the match is case-insensitive and a refusal that
        // only fired for shouting would be worse than none.
        "select id, rank() over (order by year) from books",
        // An aggregate `OVER`, which is the shape that looks most like
        // something the grammar already has — `SUM(year)` parses, and it is
        // the `OVER` after it that does not.
        "SELECT id, SUM(year) OVER (PARTITION BY author_id) FROM books",
        // No partition and no order, which is still a window.
        "SELECT COUNT(*) OVER () FROM books",
    ] {
        let message = refusal(&playground, sql);
        assert!(
            !message.contains("expected SELECT") && !message.contains("unexpected"),
            "`{sql}` fell through to the generic message: {message}"
        );
        assert!(
            message.contains("OVER is not supported"),
            "{sql}: {message}"
        );
    }
}

/// The message says the operator exists, names where the gap is, and offers
/// the clause that does work.
///
/// Asserted piece by piece: the wording will be edited and the content is what
/// has to survive.
#[test]
fn the_refusal_says_the_kernel_has_it_and_what_is_missing() {
    let playground = Playground::new();
    let message = refusal(
        &playground,
        "SELECT id, ROW_NUMBER() OVER (ORDER BY year) FROM books",
    );

    // That it exists, which is the part that stops a reader rebuilding it.
    assert!(message.contains("The kernel does have"), "{message}");
    // Which functions, so the reader can tell whether the one they want is
    // among them without reading the kernel.
    assert!(message.contains("ROW_NUMBER"), "{message}");
    assert!(message.contains("LAG"), "{message}");
    // Where the gap actually is — both halves, because fixing one leaves it
    // unreachable.
    assert!(message.contains("query spec"), "{message}");
    assert!(message.contains("protocol"), "{message}");
    // And the thing to reach for instead, with the difference that decides it.
    assert!(message.contains("GROUP BY"), "{message}");
    assert!(message.contains("one per input row"), "{message}");
}

/// A column called `over` is not a window clause.
///
/// The check is two tokens — the word and an open paren — precisely so this
/// keeps working. `over` is not reserved in this grammar, and refusing a query
/// because a column is named after a keyword in a feature the query does not
/// use would be a worse bug than the one the refusal fixes.
#[test]
fn a_bare_word_over_is_left_alone() {
    let playground = Playground::new();
    // No table here has such a column, so this must fail — but on the column,
    // not on the word. Asserting the *message* rather than success is what
    // makes this test possible without changing the demo catalog.
    let message = refusal(&playground, "SELECT over FROM books");
    assert!(
        !message.contains("OVER is not supported"),
        "a column reference was read as a window clause: {message}"
    );
}
