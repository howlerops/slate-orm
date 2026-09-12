//! Patterns, which are caller input.
//!
//! A `LIKE` pattern and a regular expression both arrive from whoever wrote the
//! query, which in an application means they can arrive from whoever filled in
//! a search box. Three things must hold no matter what they send:
//!
//! 1. **It terminates.** A matcher that backtracks catastrophically turns a
//!    search box into a denial of service — one request pinning a core for
//!    minutes. This is why `LIKE` matches iteratively rather than recursively
//!    and why `regex` was chosen over an engine with backreferences.
//! 2. **It does not overflow the stack.** Recursion depth driven by input
//!    length is an abort, not an error, and no `catch_unwind` saves it.
//! 3. **A pattern turned into scan bounds still finds every matching row.**
//!    This is the quiet one. An anchored pattern becomes a key range, and a
//!    range derived one byte too narrow silently drops rows — the query
//!    succeeds and the answer is wrong.
//!
//! The timing bounds here are deliberately loose. They are not performance
//! assertions; they are the difference between linear and exponential, which is
//! the difference between microseconds and heat death.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::expr::{Expr, like_matches, like_prefix};
use slate_tuple::{Direction, Value, encode, encode_prefix_into, prefix_range};
use std::ops::Bound;
use std::time::{Duration, Instant};

/// Generous enough that an ordinary match never trips it, tight enough that
/// exponential backtracking cannot hide under it.
const BUDGET: Duration = Duration::from_secs(5);

fn within_budget(what: &str, f: impl FnOnce()) {
    let started = Instant::now();
    f();
    let elapsed = started.elapsed();
    assert!(
        elapsed < BUDGET,
        "{what} took {elapsed:?}, which is not a constant factor"
    );
}

/// The classic catastrophic-backtracking shapes, against text that cannot match.
///
/// A recursive `LIKE` matcher on `%a%a%a%…` explores every way of splitting the
/// text between the wildcards. At twenty wildcards that is astronomically many,
/// so a matcher that does it does not finish, ever.
#[test]
fn pathological_like_patterns_terminate() {
    let cases: Vec<(String, String)> = vec![
        ("%a".repeat(24), "a".repeat(64)),
        ("%a".repeat(24), format!("{}b", "a".repeat(64))),
        ("a%".repeat(24), "a".repeat(64)),
        ("%".repeat(200), "x".repeat(200)),
        (format!("{}b", "%_".repeat(40)), "a".repeat(200)),
        ("%_".repeat(50), "a".repeat(49)),
        (format!("{}%", "_".repeat(100)), "a".repeat(99)),
    ];

    for (pattern, text) in cases {
        within_budget(
            &format!("LIKE {:?}", &pattern[..pattern.len().min(24)]),
            || {
                let _ = like_matches(&text, &pattern);
            },
        );
    }
}

/// Deep nesting must not recurse. A stack overflow aborts the process, so this
/// one is not catchable and not recoverable.
#[test]
fn a_very_long_pattern_does_not_overflow_the_stack() {
    let pattern = "%".repeat(50_000);
    let text = "x".repeat(50_000);
    within_budget("a fifty-thousand-wildcard pattern", || {
        assert!(like_matches(&text, &pattern));
    });
}

/// A pattern is not required to be valid UTF-8-shaped anything; it is a string,
/// and every string has to be answerable.
#[test]
fn unusual_patterns_are_answered_rather_than_refused() {
    let cases = [
        ("", ""),
        ("", "abc"),
        ("abc", ""),
        ("%", ""),
        ("_", ""),
        ("\\", "\\"),
        ("\\\\", "\\"),
        ("\\%", "%"),
        ("\\%", "abc"),
        ("100\\%", "100%"),
        ("\u{1F600}%", "\u{1F600}abc"),
        ("%\u{1F600}", "abc\u{1F600}"),
        ("\0", "\0"),
        ("%\0%", "a\0b"),
    ];
    for (pattern, text) in cases {
        // Only that it answers.
        let _ = like_matches(text, pattern);
    }
}

/// A regular expression that would make a backtracking engine hang.
///
/// `regex` guarantees linear time, so this passes by construction — which is
/// exactly the property being pinned. Someone swapping the engine for one with
/// backreferences would fail here rather than in production.
#[test]
fn pathological_regexes_terminate() {
    let cases = [
        ("(a+)+$", "a".repeat(40)),
        ("(a|a)*$", "a".repeat(40)),
        ("(a*)*b", "a".repeat(60)),
        ("(x+x+)+y", "x".repeat(50)),
    ];

    for (pattern, text) in cases {
        within_budget(&format!("regex {pattern:?}"), || {
            let expr = Expr::matches(slate_schema::Ordinal(0), pattern);
            let row = slate_schema::Row::new(vec![Value::Str(text.clone())]);
            let _ = expr.admits(&row);
        });
    }
}

/// An invalid or oversized regex is an error, not a panic.
///
/// The pattern reaches the engine from the caller, so "this does not compile"
/// has to be a result the query returns rather than a crash it causes.
///
/// `(a{1000}){1000}` is the case that matters and the reason a length limit on
/// the pattern string would not be a defence: fifteen characters asking for a
/// million-state machine. What stops it is the crate's 10 MiB compiled-size
/// limit, which is a property of the engine rather than of this code — so it
/// is pinned here, where swapping the engine for one without such a limit
/// would fail.
#[test]
fn an_unusable_regex_is_reported_rather_than_fatal() {
    for pattern in [
        "(",
        "[",
        "a{2,1}",
        "(?P<n>a)(?P<n>b)",
        "\\",
        "(a{1000}){1000}",
        "((a{100}){100}){100}",
    ] {
        let expr = Expr::matches(slate_schema::Ordinal(0), pattern);
        // The error is reported up front, where a caller can see it...
        let reported = expr.regex_error();
        // ...and evaluating it anyway does not panic.
        let row = slate_schema::Row::new(vec![Value::Str("anything".to_owned())]);
        let admitted = expr.admits(&row);
        assert!(
            reported.is_some(),
            "{pattern:?} compiled, so this case no longer tests what it claims"
        );
        assert!(
            !admitted,
            "{pattern:?} failed to compile but admitted a row anyway"
        );
    }
}

/// A pattern that is large but legal still compiles promptly.
///
/// The size limit rejects the extreme; everything under it is work the caller
/// gets to ask for. This bounds that work, so "legal but enormous" cannot be a
/// slow denial of service in place of a fast one.
#[test]
fn a_large_but_legal_regex_compiles_promptly() {
    for pattern in [
        "a{100000}".to_owned(),
        "a{1000}".repeat(100),
        format!("({})", vec!["a"; 50_000].join("|")),
    ] {
        within_budget("a large regex", || {
            let expr = Expr::matches(slate_schema::Ordinal(0), pattern.clone());
            let row = slate_schema::Row::new(vec![Value::Str("a".repeat(1000))]);
            let _ = expr.admits(&row);
        });
    }
}

/// The property that makes anchored patterns safe to turn into scan bounds:
/// **if a value matches the pattern, its encoding is inside the derived range.**
///
/// A bound that is too wide costs a few rows of scanning. A bound that is too
/// narrow loses rows, returns a short answer, and reports success. This is
/// checked against the matcher rather than against a list of examples, because
/// the cases that break it are escapes and multi-byte characters at the
/// boundary — the ones nobody writes down.
#[test]
fn a_derived_scan_bound_never_loses_a_matching_row() {
    proptest!(ProptestConfig::with_cases(3000), |(
        pattern in pattern_strategy(),
        text in text_strategy(),
    )| {
        if !like_matches(&text, &pattern) {
            return Ok(());
        }
        let Some(prefix) = like_prefix(&pattern) else {
            // No anchored prefix means no bound is derived, so nothing to lose.
            return Ok(());
        };

        // The bound the planner would build from that prefix.
        let mut bound = Vec::new();
        encode_prefix_into(&mut bound, &Value::Str(prefix.clone()), Direction::Asc);
        let (start, end) = prefix_range(&bound);

        let encoded = encode(&[Value::Str(text.clone())]);
        let above = match &start {
            Bound::Unbounded => true,
            Bound::Included(s) => encoded >= *s,
            Bound::Excluded(s) => encoded > *s,
        };
        let below = match &end {
            Bound::Unbounded => true,
            Bound::Included(e) => encoded <= *e,
            Bound::Excluded(e) => encoded < *e,
        };
        prop_assert!(
            above && below,
            "{text:?} matches {pattern:?} (prefix {prefix:?}) but its encoding \
             falls outside the derived bound"
        );
    });
}

/// Characters chosen to land on the codec's escaping rules and on multi-byte
/// boundaries, which is where a prefix bound goes wrong.
fn text_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just('a'),
            Just('b'),
            Just('%'),
            Just('_'),
            Just('\\'),
            Just('\0'),
            Just('\u{7F}'),
            Just('\u{80}'),
            Just('é'),
            Just('\u{1F600}'),
            Just('\u{10FFFF}'),
        ],
        0..6,
    )
    .prop_map(|cs| cs.into_iter().collect())
}

fn pattern_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("a".to_owned()),
            Just("b".to_owned()),
            Just("%".to_owned()),
            Just("_".to_owned()),
            Just("\\%".to_owned()),
            Just("\\_".to_owned()),
            Just("\\\\".to_owned()),
            Just("\0".to_owned()),
            Just("é".to_owned()),
            Just("\u{1F600}".to_owned()),
        ],
        0..6,
    )
    .prop_map(|parts| parts.concat())
}

/// Matching itself must not panic, for any pattern against any text.
#[test]
fn matching_never_panics() {
    proptest!(ProptestConfig::with_cases(4000), |(
        pattern in pattern_strategy(),
        text in text_strategy(),
    )| {
        let _ = like_matches(&text, &pattern);
        let _ = like_prefix(&pattern);
    });
}

/// A trailing escape is the one malformed pattern SQL does not define, and the
/// one most likely to index past the end.
#[test]
fn a_pattern_ending_in_an_escape_is_handled() {
    for pattern in ["\\", "abc\\", "%\\", "\\\\\\"] {
        let _ = like_matches("abc\\", pattern);
        let _ = like_prefix(pattern);
    }
}
