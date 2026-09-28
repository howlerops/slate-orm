//! `WITH`, whose three constructs have three different answers.
//!
//! A recursive CTE is a fixpoint and is refused; a CTE read more than once —
//! or read from anywhere but the statement's own `FROM` — has to be computed
//! once and read twice, which is a second plan, and is refused for the reason
//! `UNION` is; a non-recursive CTE read once **inlines into the outer query**
//! and is answered. `docs/ctes.md` works all three through.
//!
//! This file held the refusal for all three while none of them was built. The
//! third is built now, so what it holds is the split: that the two refusals
//! still refuse, with their reasons, and that the accepted shape reaches the
//! executor through the binding rather than only through `slate-sql`'s own
//! tests. The expansion itself — that the CTE's spec *equals* the inlined
//! query's — is pinned in `crates/slate-sql/tests/front_end.rs`, which is
//! where the parser lives.
//!
//! The refusals used to be `expected SELECT, INSERT, UPDATE or DELETE, found
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

/// Every `WITH` this front end refuses reaches a named refusal, not the
/// generic one.
#[test]
fn a_refused_with_is_refused_by_name_and_not_as_an_unexpected_token() {
    let playground = Playground::new();
    for sql in [
        "WITH RECURSIVE t AS (SELECT id FROM books) SELECT id FROM t",
        // Lower case, because the dispatcher matches a normalised word and a
        // refusal that only fired for shouting would be worse than none.
        "with recursive t as (select id from books) select id from t",
        // Read twice: the shape `docs/ctes.md` §2 refuses.
        "WITH recent AS (SELECT id FROM books WHERE year > 2000) \
         SELECT * FROM recent JOIN recent AS other ON recent.id = other.id",
        // Nothing after the keyword at all. A `WITH` that is not even a
        // well-formed CTE still gets the shape written out rather than a
        // syntax error about the part after it.
        "WITH",
    ] {
        let message = refusal(&playground, sql);
        assert!(
            !message.contains("expected SELECT"),
            "`{sql}` fell through to the generic message: {message}"
        );
        assert!(
            message.contains("WITH") || message.contains("CTE"),
            "{message}"
        );
        assert!(message.contains("docs/ctes.md"), "`{sql}`: {message}");
    }
}

/// And the shape that is built runs, through the binding, to an answer.
///
/// `slate-sql`'s own suite proves the *spec* is the inlined query's. This
/// proves the browser's path reaches the executor with it: a refusal that had
/// merely stopped being produced, with nothing behind it, would pass that
/// suite and fail here.
#[test]
fn a_non_recursive_cte_read_once_is_answered() {
    let playground = Playground::new();
    let out: Vec<Json> = serde_json::from_str(&playground.sql(
        "WITH recent AS (SELECT id, year FROM books WHERE year > 2000) \
         SELECT id FROM recent WHERE id > 5",
    ))
    .expect("json");
    let got = out.into_iter().next().expect("one statement");
    assert!(got["error"].is_null(), "{got}");
    // The plan names the base table and nothing in it names the CTE, which is
    // `docs/ctes.md`'s security point made where a caller can see it: the name
    // lives for one statement and never reaches anything that resolves a
    // table.
    assert_eq!(got["plan"]["table"], "books", "{got}");
    assert!(
        !got["plan"].to_string().contains("recent"),
        "the CTE's name reached the plan: {got}"
    );
}

/// The two refusals name their own reason, and neither claims the third.
///
/// Asserted piece by piece rather than as one string: the wording will be
/// edited and the *content* is what must survive. A refusal that said only
/// "CTEs are not supported" would be telling somebody that the one shape that
/// works is impossible — which is what the single message this replaces had to
/// be careful about, and is now impossible to write by accident, because the
/// two refusals are reached by two different statements.
#[test]
fn each_refusal_carries_the_reason_that_is_its_own() {
    let playground = Playground::new();

    // The recursive case, and why: a fixpoint has nothing to iterate on.
    //
    // `"fixpoint"` rather than `"recursive"` — the word `recursive` is in the
    // statement and would be echoed by a message that explained nothing.
    let recursive = refusal(
        &playground,
        "WITH RECURSIVE t AS (SELECT id FROM books) SELECT id FROM t",
    );
    assert!(recursive.contains("fixpoint"), "{recursive}");
    assert!(recursive.contains("docs/ctes.md"), "{recursive}");

    // The read-twice case, and the reason it shares with UNION.
    let twice = refusal(
        &playground,
        "WITH recent AS (SELECT id FROM books) \
         SELECT * FROM recent JOIN recent AS other ON recent.id = other.id",
    );
    assert!(twice.contains("computed once and read twice"), "{twice}");
    assert!(twice.contains("UNION"), "{twice}");
    assert!(twice.contains("docs/ctes.md"), "{twice}");
}

/// The uncorrelated subquery `docs/ctes.md` contrasts a CTE with still works.
///
/// It was the thing the old blanket refusal pointed at, and the reason this
/// case was written: a refusal that sends a reader to a second thing that is
/// also refused is worse than one that says nothing. The refusal no longer
/// points at it, and the contrast the note draws — half of what a CTE does,
/// already done, in a different position — still has to hold.
#[test]
fn the_subquery_a_cte_is_contrasted_with_is_still_accepted() {
    let playground = Playground::new();
    let out: Vec<Json> = serde_json::from_str(
        &playground
            .sql("SELECT id FROM books WHERE id IN (SELECT id FROM books WHERE year > 2000)"),
    )
    .expect("json");
    let got = out.into_iter().next().expect("one statement");
    assert!(
        got["error"].is_null(),
        "the subquery docs/ctes.md contrasts a CTE with was refused: {got}"
    );
}

/// `CREATE` is refused by name too, and says what a view could not be.
///
/// Its own test rather than a row in the loop above, because the content that
/// matters is different: `WITH`'s message is about three shapes, and this one
/// is about a security expectation a reader arrives with. Somebody writing
/// `CREATE VIEW` is usually trying to give a role less than a table, and the
/// answer here is that a view cannot do that — which is worth learning at the
/// statement rather than after a permission model rests on it.
#[test]
fn create_is_refused_by_name_and_says_a_view_is_not_a_grant() {
    let playground = Playground::new();
    let message = refusal(&playground, "CREATE VIEW v AS SELECT id FROM books");
    assert!(
        !message.contains("expected SELECT"),
        "CREATE fell through to the generic message: {message}"
    );
    // The grant is the point, not the syntax.
    assert!(message.contains("grant"), "{message}");
    assert!(message.contains("base table"), "{message}");
    // And where a reader goes for the rest of it.
    assert!(message.contains("docs/views.md"), "{message}");
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
