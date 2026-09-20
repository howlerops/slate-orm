//! `WITH`, which this front end refuses — and the refusal says which part.
//!
//! Three constructs share one keyword and have three different answers, which
//! is why the message is long and why this file exists to hold it to them:
//! a recursive CTE is a fixpoint and is refused; a CTE read twice has to be
//! computed once and read twice, which is a second plan and is refused for the
//! reason `UNION` is; a non-recursive CTE read once inlines into the outer
//! query, which is a gap rather than a refusal and is the same mechanism a
//! view needs. `docs/ctes.md` works all three through.
//!
//! The message used to be `expected SELECT, INSERT, UPDATE or DELETE, found
//! `WITH`` — the exact failure the set-operator refusal was written against,
//! whose own comment says a bare "unexpected" *"reads as a parser that has not
//! heard of it, when the real answer is that there is nowhere for it to go"*.

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

/// Every spelling of `WITH` reaches the named refusal, not the generic one.
#[test]
fn with_is_refused_by_name_and_not_as_an_unexpected_token() {
    let playground = Playground::new();
    for sql in [
        "WITH recent AS (SELECT id FROM books WHERE year > 2000) SELECT id FROM recent",
        // Lower case, because the dispatcher matches a normalised word and a
        // refusal that only fired for shouting would be worse than none.
        "with recent as (select id from books) select id from recent",
        "WITH RECURSIVE t AS (SELECT id FROM books) SELECT id FROM t",
        // Nothing after the keyword at all. The refusal is chosen on the first
        // word, before any of the rest is parsed, so a `WITH` that is not even
        // a well-formed CTE still gets the explanation rather than a syntax
        // error about the part after it.
        "WITH",
    ] {
        let message = refusal(&playground, sql);
        assert!(
            !message.contains("expected SELECT"),
            "`{sql}` fell through to the generic message: {message}"
        );
        assert!(message.contains("WITH is not supported"), "{message}");
    }
}

/// The message names all three answers, because it is wrong about one of them
/// if it names only "CTEs".
///
/// Asserted piece by piece rather than as one string: the wording will be
/// edited and the *content* is what must survive. A refusal that said only
/// "CTEs are not supported" would be telling somebody that the one shape they
/// could build is impossible.
#[test]
fn the_refusal_separates_the_three_shapes() {
    let playground = Playground::new();
    let message = refusal(
        &playground,
        "WITH recent AS (SELECT id FROM books) SELECT id FROM recent",
    );

    // The recursive case, and why: a fixpoint has nothing to iterate on.
    //
    // `"fixpoint"` rather than `"recursive"`. The first version asserted the
    // latter and was vacuous: the message says "a non-recursive CTE referenced
    // once" further down, so the whole recursive clause could be deleted and
    // the substring would still be there. A mutation that deleted it survived.
    assert!(message.contains("fixpoint"), "{message}");
    // The read-twice case, and the reason it shares with UNION.
    assert!(message.contains("more than once"), "{message}");
    assert!(message.contains("UNION"), "{message}");
    // The buildable case, named as a gap and tied to views rather than lumped
    // in with the refusals.
    assert!(message.contains("view"), "{message}");
    // Where the reasoning lives, since a message cannot hold all of it.
    assert!(message.contains("docs/ctes.md"), "{message}");
    // And what does work instead, which is the part a reader can act on today.
    assert!(message.contains("IN (SELECT"), "{message}");
}

/// The thing the refusal points at actually works.
///
/// Without this, the message is advice nobody checked — and a refusal that
/// sends a reader to a second thing that is also refused is worse than one
/// that says nothing.
#[test]
fn the_subquery_the_refusal_recommends_is_accepted() {
    let playground = Playground::new();
    let out: Vec<Json> = serde_json::from_str(
        &playground
            .sql("SELECT id FROM books WHERE id IN (SELECT id FROM books WHERE year > 2000)"),
    )
    .expect("json");
    let got = out.into_iter().next().expect("one statement");
    assert!(
        got["error"].is_null(),
        "the subquery the WITH refusal recommends was itself refused: {got}"
    );
}

/// A column or table called `with` is still reachable.
///
/// The dispatcher matches `with` only in the *first* position, so this is not
/// a reserved word anywhere else. Worth pinning: the cheap implementation of
/// the refusal is a scan for the token, and that one would break a schema
/// with a `with` column in it.
#[test]
fn with_is_only_special_as_the_first_word() {
    let playground = Playground::new();
    let out: Vec<Json> =
        serde_json::from_str(&playground.sql("SELECT id FROM books WHERE title LIKE 'with%'"))
            .expect("json");
    let got = out.into_iter().next().expect("one statement");
    assert!(got["error"].is_null(), "{got}");
}
