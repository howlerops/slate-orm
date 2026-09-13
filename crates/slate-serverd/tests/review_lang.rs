//! Adversarial probes on the configuration language: does an expression *mean*
//! what it reads as?
//!
//! The parser's own unit tests live beside it and assert the tree it produces.
//! That is the right shape for a parser and it has one blind spot: a test that
//! says `parse("a OR b AND c")` equals `Or([a, And([b, c])])` is written by
//! whoever decided where the `And` goes. This file asks the question from the
//! other end — a predicate goes into a `[[security.policies]]` entry, rows go
//! into the store, and the ids that come back are compared against a set
//! written out by hand from reading the text as SQL.
//!
//! Nothing here shares a line with the parser, so a precedence table that is
//! wrong the same way in both places cannot pass.
//!
//! # Why one server and many tables
//!
//! Every predicate needs a policy, policies on one table are combined with
//! `OR`, and starting a process per predicate would make this the slowest file
//! in the crate. So each predicate gets a table of its own, all of them carry
//! the identical seven rows, and one server answers the lot.
//!
//! # The cases
//!
//! Precedence (`AND` binds tighter than `OR`, `NOT` tighter than both),
//! associativity, unary minus, the doubled-quote escape, and the places SQL's
//! three-valued logic decides the answer rather than a comparison does —
//! `NOT LIKE`, `NOT IN`, `IS NULL`, a `NOT` in front of a pattern, and a
//! placeholder that lowers to null. Each is chosen so that the *wrong* reading
//! returns a different set, which is stated in the comment beside it: a case
//! both readings agree on tests nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_update,
    unreachable_pub
)]

mod harness;

use harness::{Identity, Serving, connect, proto, query, rows};

/// The reader. Principal `u64:3` so `id = :principal` has one row to find.
const APP: Identity = Identity::app("u64:3", "u64:1");

/// `(id, kind, size, note)`. Sizes straddle zero so a negative literal is not
/// decoration; two notes are null and one holds a quote.
const ROWS: [(u64, &str, i64, Option<&str>); 7] = [
    (0, "a", 1, None),
    (1, "a", 7, Some("note-1")),
    (2, "b", 2, None),
    (3, "b", 9, Some("x-3")),
    (4, "c", -2, Some("note-4")),
    (5, "c", 3, None),
    (6, "d", 0, Some("it's")),
];

/// One predicate, the ids it must admit, and what the *other* plausible
/// reading of the same text would have returned.
struct Case {
    /// The `using` text, exactly as an operator would write it.
    using: &'static str,
    /// The ids a reader of the text expects, written out here.
    admits: &'static [u64],
    /// Why this case is not vacuous: the reading it rules out.
    rules_out: &'static str,
}

const CASES: &[Case] = &[
    Case {
        // `AND` binds tighter than `OR`.
        using: "kind = 'a' OR kind = 'b' AND size > 5",
        admits: &[0, 1, 3],
        rules_out: "(kind='a' OR kind='b') AND size>5, which is {1, 3}",
    },
    Case {
        // `NOT` binds tighter than `AND`.
        using: "NOT kind = 'a' AND size > 5",
        admits: &[3],
        rules_out: "NOT (kind='a' AND size>5), which is everything but row 1",
    },
    Case {
        using: "NOT (kind = 'a' OR size > 5)",
        admits: &[2, 4, 5, 6],
        rules_out: "NOT kind='a' OR size>5, which would add rows 1 and 3",
    },
    Case {
        // Unary minus on the right of a comparison.
        using: "size >= -1",
        admits: &[0, 1, 2, 3, 5, 6],
        rules_out: "`-1` read as `1`, which would drop row 6",
    },
    Case {
        // A range with a negative bound, so the minus survives a conjunction.
        using: "size > -1 AND size <= 3",
        admits: &[0, 2, 5, 6],
        rules_out: "a bound that lost its sign, which would drop row 6",
    },
    Case {
        using: "size <> 3",
        admits: &[0, 1, 2, 3, 4, 6],
        rules_out: "`<>` read as anything but not-equal",
    },
    Case {
        using: "size != 3",
        admits: &[0, 1, 2, 3, 4, 6],
        rules_out: "`!=` and `<>` meaning different things",
    },
    Case {
        // A null note is a group, not an omission.
        using: "note IS NULL OR note LIKE 'n%'",
        admits: &[0, 1, 2, 4, 5],
        rules_out: "`IS NULL` reading as `IS NOT NULL`",
    },
    Case {
        // Three-valued: a null note is *unknown*, not "does not match".
        using: "note NOT LIKE 'n%'",
        admits: &[3, 6],
        rules_out: "a `NOT LIKE` that lets nulls through, which would add 0, 2 and 5",
    },
    Case {
        // The same statement with `NOT` in front rather than infixed. The two
        // lower to different `Expr` shapes — `Like { negated }` against
        // `Not(Like)` — and must mean the same thing.
        using: "NOT (note LIKE 'n%')",
        admits: &[3, 6],
        rules_out: "the prefix and infix spellings disagreeing about nulls",
    },
    Case {
        using: "kind NOT IN ('a', 'b')",
        admits: &[4, 5, 6],
        rules_out: "`NOT IN` reading as `IN`",
    },
    Case {
        // `NOT IN` and a conjunction, so the `NOT` cannot have escaped its test.
        using: "kind NOT IN ('a') AND size > 2",
        admits: &[3, 5],
        rules_out: "NOT (kind IN ('a') AND size > 2), which is everything but 1",
    },
    Case {
        // A negative literal inside an `IN` list.
        using: "size IN (1, -2, 3)",
        admits: &[0, 4, 5],
        rules_out: "an `IN` list that dropped the sign, which would swap 4 for nothing",
    },
    Case {
        // Case-insensitive regex.
        using: "kind ~* 'A'",
        admits: &[0, 1],
        rules_out: "`~*` read as case-sensitive, which is empty",
    },
    Case {
        using: "size > 1 AND size < 4 OR kind = 'z'",
        admits: &[2, 5],
        rules_out: "size > 1 AND (size < 4 OR kind='z'), which would add rows 1 and 3",
    },
    Case {
        // A doubled quote is one quote.
        using: "note = 'it''s'",
        admits: &[6],
        rules_out: "`''` read as an empty string or as the end of the literal",
    },
    Case {
        // The placeholder, against a `u64` column and a `u64:` principal.
        using: "id = :principal",
        admits: &[3],
        rules_out: "a principal id that did not take the column's type, which is empty",
    },
    Case {
        using: "NOT NOT kind = 'a'",
        admits: &[0, 1],
        rules_out: "a double negation that cancelled once",
    },
];

/// Predicates read by a caller who has **no tenant**, where `:tenant` lowers
/// to null.
///
/// `Pred::lower` claims this "fails closed in every position a placeholder can
/// occupy", and gives the reasoning — a comparison against null is unknown,
/// and `NOT unknown` is unknown. Every position is worth checking rather than
/// the one that was in mind, because `NOT IN` lowers to `Not(In)` where `NOT
/// LIKE` lowers to a flag, and the two could disagree about a null candidate.
const TENANTLESS: &[Case] = &[
    Case {
        using: "kind = :tenant",
        admits: &[],
        rules_out: "a null placeholder compared as if it were a value",
    },
    Case {
        using: "kind <> :tenant",
        admits: &[],
        rules_out: "`<>` against null reading as true, which admits everything",
    },
    Case {
        using: "NOT (kind = :tenant)",
        admits: &[],
        rules_out: "`NOT unknown` reading as true, which admits everything",
    },
    Case {
        using: "kind NOT IN ('a', :tenant)",
        admits: &[],
        rules_out: "an `IN` list that ignores a null candidate, which would admit 2..6",
    },
    // The control. A null in the list must not suppress a match that another
    // element makes — that is SQL, and without it the four above could pass
    // because `:tenant` disabled the whole predicate.
    Case {
        using: "kind IN ('a', :tenant)",
        admits: &[0, 1],
        rules_out: "a null in the list swallowing a real match",
    },
];

/// The tenantless reader: same principal, no `slate-tenant` header.
fn tenantless() -> Identity {
    Identity {
        principal: "u64:3",
        tenant: None,
        roles: "app",
        bearer: None,
    }
}

fn all_cases() -> Vec<&'static Case> {
    CASES.iter().chain(TENANTLESS).collect()
}

fn table_name(at: usize) -> String {
    format!("t{at}")
}

fn config() -> String {
    let mut out = String::from(
        "[listen]\naddress = \"127.0.0.1:0\"\n\n[auth]\nmode = \"trusted-header\"\n\n\
         [storage]\nbackend = \"memory\"\n\n",
    );
    for at in 0..all_cases().len() {
        out.push_str(&format!(
            "[[tables]]\nname = \"{}\"\nid = {}\ncolumns = [\n  \
             {{ name = \"id\", type = \"u64\" }},\n  \
             {{ name = \"kind\", type = \"str\" }},\n  \
             {{ name = \"size\", type = \"i64\" }},\n  \
             {{ name = \"note\", type = \"str\", nullable = true }},\n]\n\
             primary_key = [\"id\"]\n\n",
            table_name(at),
            at + 1
        ));
    }
    let names: Vec<String> = (0..all_cases().len())
        .map(|at| format!("\"{}\"", table_name(at)))
        .collect();
    out.push_str(&format!(
        "[[security.grants]]\nrole = \"app\"\ntables = [{}]\nactions = [\"all\"]\n\n",
        names.join(", ")
    ));
    for (at, case) in all_cases().into_iter().enumerate() {
        out.push_str(&format!(
            "[[security.policies]]\nname = \"p{at}\"\ntable = \"{}\"\nactions = [\"all\"]\n\
             using = \"{}\"\n\n",
            table_name(at),
            // The predicate is inside a TOML basic string, so a backslash or a
            // double quote would need escaping. None of the cases use either,
            // and this asserts it rather than silently mangling one.
            {
                assert!(
                    !case.using.contains('"') && !case.using.contains('\\'),
                    "case `{}` needs TOML escaping this writer does not do",
                    case.using
                );
                case.using
            }
        ));
    }
    out
}

fn seed() -> String {
    let mut out = String::new();
    for at in 0..all_cases().len() {
        out.push_str(&format!(
            "[[seed]]\ntable = \"{}\"\nrows = [\n",
            table_name(at)
        ));
        for (id, kind, size, note) in ROWS {
            let note = note.map_or(String::new(), |n| {
                format!(
                    ", note = \"{}\"",
                    n.replace('\\', "\\\\").replace('"', "\\\"")
                )
            });
            out.push_str(&format!(
                "  {{ id = {id}, kind = \"{kind}\", size = {size}{note} }},\n"
            ));
        }
        out.push_str("]\n\n");
    }
    out
}

fn returned_ids(returned: &[proto::Row]) -> Vec<u64> {
    let mut ids: Vec<u64> = returned
        .iter()
        .map(
            |row| match row.values.first().and_then(|v| v.kind.as_ref()) {
                Some(proto::value::Kind::Uint64Value(n)) => *n,
                other => panic!("the first column should be the u64 id, found {other:?}"),
            },
        )
        .collect();
    ids.sort_unstable();
    ids
}

/// Every predicate admits the rows a reader of its text expects.
///
/// Failures are collected rather than asserted one at a time: when a
/// precedence table is wrong it is usually wrong for several of these, and a
/// run that reports the first hides how far it spread.
#[tokio::test]
async fn every_predicate_admits_the_rows_it_reads_as_admitting() {
    let files = harness::Files::new();
    let config = files.write("review-lang.toml", &config());
    let seed = files.write("review-lang-seed.toml", &seed());
    let serving = Serving::start(&[
        "--config",
        config.to_str().unwrap(),
        "--seed",
        seed.to_str().unwrap(),
    ]);
    let mut client = connect(&serving).await;

    let mut failures = Vec::new();
    let tenantless = tenantless();
    for (at, case) in all_cases().into_iter().enumerate() {
        // The last block is read by a caller with no tenant, which is what
        // makes `:tenant` null there.
        let identity = if at < CASES.len() { &APP } else { &tenantless };
        let returned = rows(&mut client, identity, query(&table_name(at)))
            .await
            .unwrap_or_else(|status| panic!("`{}` was refused: {status}", case.using));
        let got = returned_ids(&returned);
        if got != case.admits {
            failures.push(format!(
                "`{}` admitted {:?}, and reads as admitting {:?} (this case rules out {})",
                case.using, got, case.admits, case.rules_out
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The corpus cannot be all-or-nothing.
///
/// A predicate that admitted every row, or none, would pass the property above
/// for reasons that have nothing to do with the parser. Checked against the
/// declared expectations rather than against a run, so it is a property of the
/// file and not of the server.
#[test]
fn no_case_admits_everything_or_nothing() {
    for case in CASES {
        assert!(
            !case.admits.is_empty() && case.admits.len() < ROWS.len(),
            "`{}` expects {:?} of {} rows, which tests the fixture rather than the parser",
            case.using,
            case.admits,
            ROWS.len()
        );
    }
    // The tenantless block is the one place an empty answer is the point, so
    // it is guarded the other way: at least one of its cases must admit
    // something, or "fails closed" would be a statement about a broken server.
    assert!(
        TENANTLESS.iter().any(|case| !case.admits.is_empty()),
        "every tenantless case expects nothing, so none of them proves the null \
         placeholder is what closed the door"
    );

    // And no two cases expect the same set through the same text, which would
    // mean one of them is a duplicate wearing different words.
    let mut seen: Vec<(&str, &[u64])> = Vec::new();
    for case in all_cases() {
        assert!(
            !seen.iter().any(|(text, _)| *text == case.using),
            "`{}` appears twice",
            case.using
        );
        seen.push((case.using, case.admits));
    }
}
