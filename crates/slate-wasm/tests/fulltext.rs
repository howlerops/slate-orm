//! `CONTAINS` in the SQL front end, over the workbench's own books.
//!
//! # Why a keyword at all, when the planner takes a scan
//!
//! It does take a scan: the fixture holds 4,848 books and a non-covering index
//! is worth taking at about one row in 24,000 (`docs/full-text.md` measures
//! it), and the workbench has no hint syntax to overrule that with. So every
//! `CONTAINS` here reads the table and the plan panel says so.
//!
//! The keyword is still not sugar. `LIKE '%game%'` finds *The Player of
//! Games*; `CONTAINS 'game'` does not, because a term is a whole word.
//! `CONTAINS 'games the'` finds it and `LIKE '%games%the%'` does not, because
//! terms are unordered. Neither predicate can be written as the other, and
//! `a_contains_is_not_a_like` is that claim as a test rather than a sentence.
//!
//! # Where the interesting mistake would be
//!
//! The parser stores the literal and the *kernel* tokenizes it, at lowering.
//! A front end that split the text itself would be a fifth tokenizer beside
//! Rust's, Python's, Go's and TypeScript's — and it would find fewer rows than
//! the table holds, with nothing reporting it. `the_editor_does_not_tokenize`
//! pins that by searching for something only the server's splitting rules
//! could match.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value as Json;
use slate_wasm::Playground;

fn run(playground: &Playground, text: &str) -> Json {
    let all: Vec<Json> = serde_json::from_str(&playground.sql(text)).unwrap();
    all.last().expect("a result").clone()
}

fn ok(playground: &Playground, text: &str) -> Json {
    let last = run(playground, text);
    assert!(last["error"].is_null(), "{text}: {}", last["error"]);
    last
}

/// The titles a statement returned, in the order they came back.
///
/// Found through the header rather than by index, because the answer carries
/// the table's columns and not only the projected one — so `row[0]` is the id
/// however the SELECT list is written, and a test that assumed otherwise
/// compares ids to titles and fails for the wrong reason. It did.
fn titles(playground: &Playground, text: &str) -> Vec<String> {
    let answer = ok(playground, text);
    let at = answer["columns"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| {
            c.as_str()
                .or_else(|| c["name"].as_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("title"))
        })
        .unwrap_or_else(|| panic!("no `title` column in {}", answer["columns"]));
    answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[at].as_str().unwrap().to_owned())
        .collect()
}

fn playground() -> Playground {
    Playground::new()
}

#[test]
fn one_term_finds_every_title_holding_it() {
    let p = playground();
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'earthsea' ORDER BY id"
        ),
        vec!["A Wizard of Earthsea"]
    );
}

#[test]
fn the_kernel_lowercases_the_search() {
    // Written `EARTHSEA`, stored `Earthsea`. The parser passes the literal
    // through untouched — see `the_editor_does_not_tokenize` — so the fold
    // happened below it.
    let p = playground();
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'EARTHSEA' ORDER BY id"
        ),
        vec!["A Wizard of Earthsea"]
    );
}

#[test]
fn two_terms_are_a_conjunction() {
    let p = playground();
    // Eight hand-written titles hold "the" and one holds "heaven". A
    // disjunction would return all eight, which is what makes this a test
    // rather than a restatement of the one-term case.
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'the heaven' ORDER BY id"
        ),
        vec!["The Lathe of Heaven"]
    );
}

#[test]
fn a_contains_is_not_a_like() {
    // The two directions in which neither predicate can express the other,
    // which is the whole argument for having both.
    let p = playground();

    // `LIKE` is case-sensitive and `CONTAINS` is not, so every pattern here is
    // written in the title's own case. Getting that wrong makes the negative
    // assertions pass for the wrong reason — `LIKE '%cosmic%'` finds nothing
    // whether or not a substring is a term — and the first draft of this test
    // did exactly that.

    // A substring of a term is not a term. `Cosmicomics` holds "Cosmic" as a
    // prefix of its only word, so the two predicates split on it exactly.
    assert!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'Cosmic' ORDER BY id"
        )
        .is_empty(),
        "`Cosmic` is a prefix of `Cosmicomics`, and CONTAINS matches whole words"
    );
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title LIKE '%Cosmic%' ORDER BY id"
        ),
        vec!["Cosmicomics"],
        "LIKE does match the substring, which is the difference"
    );

    // And terms are unordered, where a pattern is not. "The Lathe of Heaven"
    // holds both words; the pattern that asks for them in that order matches
    // it and the reversed pattern does not, while both searches do.
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'heaven the' ORDER BY id"
        ),
        vec!["The Lathe of Heaven"]
    );
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title LIKE '%the%Heaven%' ORDER BY id"
        ),
        vec!["The Lathe of Heaven"],
        "the ordered pattern does match, which is what makes the next line a test"
    );
    assert!(
        titles(
            &p,
            "SELECT title FROM books WHERE title LIKE '%Heaven%the%' ORDER BY id"
        )
        .is_empty(),
        "a pattern is ordered, so `Heaven` before `the` matches nothing"
    );
}

#[test]
fn the_editor_does_not_tokenize() {
    // `'master's voice'` cannot be written — the quote ends the literal — so
    // the case that separates "the editor split it" from "the kernel split
    // it" is punctuation the editor would have to know to drop. `His Master's
    // Voice` tokenizes to `his`, `master`, `s`, `voice`, and a search for
    // `master voice` finds it only if the *apostrophe* was treated as a
    // separator by whoever did the splitting.
    let p = playground();
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'master voice' ORDER BY id"
        ),
        vec!["His Master's Voice"]
    );

    // And the same search written with the punctuation still in it, which a
    // client-side splitter would have had to normalise and this one never
    // sees: it hands the literal down whole.
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'master, voice.' ORDER BY id"
        ),
        vec!["His Master's Voice"]
    );
}

#[test]
fn a_search_matching_nothing_is_an_answer_and_not_an_error() {
    let p = playground();
    assert!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'cephalopod' ORDER BY id"
        )
        .is_empty()
    );
}

#[test]
fn a_contains_combines_with_another_filter() {
    // The conjunction is built by the same `AND` every other predicate uses,
    // so this is really a check that `contains` did not become a special case
    // the filter list handles differently.
    let p = playground();
    assert_eq!(
        titles(
            &p,
            "SELECT title FROM books WHERE title CONTAINS 'the' AND year < 1970 ORDER BY id",
        ),
        vec!["The Left Hand of Darkness", "The Aleph", "The Cyberiad"]
    );
}
