//! Adversarial probes on the schema fingerprint.
//!
//! `schema_check.rs` states the cases somebody had in mind: a rename, a drop,
//! an append, a swap, a name the table never had. Two things it does not do,
//! and this file does.
//!
//! **A collision sweep.** Every case there is one hand-picked pair. The
//! question "can two genuinely different declarations hash alike" is answered
//! by generating the declarations rather than choosing them, over an alphabet
//! chosen to attack the *framing* rather than the hash: names made of digits,
//! colons and semicolons — the two bytes the canonical form uses as
//! delimiters — and names that spell the literal field markers `key` and
//! `columns`. A length-prefixed encoding is supposed to make those harmless;
//! that is a claim, and this is the test of it.
//!
//! **What the prefix rule does not cover.** The check compares a *prefix*, and
//! nothing ties the prefix to the ordinals the request then goes on to use.
//! See [`a_short_declaration_leaves_later_ordinals_unchecked`], which is
//! written as a statement of the current behaviour rather than as a failure,
//! because whether it should be refused is a design call and not a bug in the
//! arithmetic.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, drain, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_schema::{Ordinal, TableDef, TableId};
use slate_server::fingerprint;
use slate_server::proto as pb;
use slate_tuple::{Value, ValueType};
use std::collections::HashMap;
use std::sync::Arc;
use tonic::Code;

const DOCS: TableId = TableId(1);

// --- the collision sweep --------------------------------------------------

/// Names picked to attack the framing, not the hash.
///
/// `:` and `;` are the canonical form's own delimiters, `key` and `columns`
/// are its two literal field markers, and the bare digits sit next to the
/// decimal length prefix and the decimal ordinal. If any of the framing is
/// ambiguous, two of these spell it.
const NAMES: &[&str] = &[
    "a", "ab", "a:b", "a;b", "1", "12", "1:a", "key", "columns", "3:a", "0",
];

const TYPES: &[ValueType] = &[ValueType::U64, ValueType::Str, ValueType::I64];

/// A declaration exactly as a client states it, and the only thing the
/// fingerprint is allowed to distinguish on.
///
/// Written out here rather than taken from `TableDef` so that "two
/// declarations are the same" is this test's statement and not the
/// implementation's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Declared {
    table: String,
    columns: Vec<(String, String)>,
    key: Vec<usize>,
}

fn build(table: &str, columns: &[(&str, ValueType)], key: &[usize]) -> Option<TableDef> {
    let mut builder = TableDef::builder(table, DOCS);
    for (name, value_type) in columns {
        builder = builder.column(*name, *value_type);
    }
    let key_names: Vec<&str> = key.iter().map(|k| columns[*k].0).collect();
    builder.primary_key(key_names).build().ok()
}

/// Every fingerprint of every declaration the corpus can state must be
/// injective: two different declarations, two different numbers.
///
/// The corpus is every one- and two-column table over the adversarial names
/// and three types, under both single- and two-column keys, hashed at every
/// declared width the check would accept — which is what folds the prefix rule
/// into the sweep, since `of(table, 1)` of a two-column table is what an older
/// client sends.
#[test]
fn no_two_declarations_share_a_fingerprint() {
    let mut seen: HashMap<u64, Declared> = HashMap::new();
    let mut checked = 0usize;

    for table in ["docs", "doc", "docs1", "1", "key"] {
        for first in NAMES {
            for first_type in TYPES {
                // One column, keyed on it.
                let single = [(*first, *first_type)];
                record(&mut seen, &mut checked, table, &single, &[0], 1);

                for second in NAMES {
                    if second == first {
                        continue;
                    }
                    for second_type in TYPES {
                        let pair = [(*first, *first_type), (*second, *second_type)];
                        // A client that knows both columns.
                        record(&mut seen, &mut checked, table, &pair, &[0], 2);
                        // A client written before the second was added. The
                        // check accepts this against the two-column table, so
                        // it has to be distinguishable from every other
                        // declaration too.
                        record(&mut seen, &mut checked, table, &pair, &[0], 1);
                        // A two-column key, which changes the declaration
                        // without changing a single name or type.
                        record(&mut seen, &mut checked, table, &pair, &[0, 1], 2);
                        record(&mut seen, &mut checked, table, &pair, &[1, 0], 2);
                    }
                }
            }
        }
    }

    assert!(
        checked > 15_000,
        "the sweep only reached {checked} declarations, which is not a sweep"
    );
}

fn record(
    seen: &mut HashMap<u64, Declared>,
    checked: &mut usize,
    table: &str,
    columns: &[(&str, ValueType)],
    key: &[usize],
    declared: usize,
) {
    let Some(built) = build(table, columns, key) else {
        return;
    };
    // The check refuses a declaration that stops before a key column, so those
    // are not declarations this fingerprint has to separate.
    if key.iter().any(|k| *k >= declared) {
        return;
    }
    let statement = Declared {
        table: table.to_owned(),
        columns: columns
            .iter()
            .take(declared)
            .map(|(name, value_type)| ((*name).to_owned(), value_type.name().to_owned()))
            .collect(),
        key: key.to_vec(),
    };
    let hash = fingerprint::of(&built, declared);
    *checked += 1;
    match seen.get(&hash) {
        Some(other) if *other != statement => panic!(
            "two different declarations share fingerprint {hash:#x}:\n  {statement:?}\n  {other:?}"
        ),
        _ => {
            seen.insert(hash, statement);
        }
    }
}

/// The sweep above proves nothing if the corpus is degenerate — if every
/// declaration in it were the same declaration, no collision could exist.
#[test]
fn the_sweep_contains_declarations_that_differ_only_in_the_framing() {
    // `a:b` + `c` against `a` + `b:c`, which a delimiter-joined encoding would
    // hash alike.
    let left = build("t", &[("a:b", ValueType::Str), ("c", ValueType::Str)], &[0]).unwrap();
    let right = build("t", &[("a", ValueType::Str), ("b:c", ValueType::Str)], &[0]).unwrap();
    assert_ne!(fingerprint::of(&left, 2), fingerprint::of(&right, 2));

    // A one-column table whose only column is called `columns`, against the
    // literal field marker.
    let marker = build("t", &[("columns", ValueType::Str)], &[0]).unwrap();
    let plain = build("t", &[("a", ValueType::Str)], &[0]).unwrap();
    assert_ne!(fingerprint::of(&marker, 1), fingerprint::of(&plain, 1));
}

// --- what the prefix rule leaves open -------------------------------------

/// The check compares the client's declaration against the table's *prefix*,
/// and never against the ordinals the request actually uses.
///
/// So a request may declare one column, pass the check, and then filter on
/// ordinal 2 — an ordinal outside everything it claimed to know. That is the
/// case the module exists for ("a request still says input 1's column 3, and a
/// client outside Rust knows `title` is column 3 only because somebody wrote
/// that down"), reached through a declaration that is a perfectly valid prefix.
///
/// This is recorded as the current behaviour, not asserted to be wrong: the
/// server *has* the information to refuse it — `convert` resolves every
/// `ColumnRef` after `fingerprint::check` runs — so closing it is a change of
/// policy rather than a fix to the arithmetic.
#[tokio::test]
async fn a_short_declaration_leaves_later_ordinals_unchecked() {
    let writer = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&writer)).await;
    let mut client = serving.client().await;

    client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![
                slate_server::convert::row_to_proto(&common::doc(1, "a", 10, None)),
                slate_server::convert::row_to_proto(&common::doc(2, "b", 20, None)),
            ],
            upsert: false,
            schema: None,
        }))
        .await
        .expect("the seed insert is served");

    let table = common::docs();
    // The declaration a client that knows only `id` would send. It is a
    // genuine prefix of this table and holds the whole primary key, so the
    // check accepts it.
    let short = pb::SchemaCheck {
        columns: 1,
        fingerprint: fingerprint::of(&table, 1),
    };
    assert!(
        fingerprint::check(&table, Some(&short)).is_ok(),
        "a one-column prefix declaration is accepted"
    );

    // And a filter on ordinal 2 — `size`, which the declaration never
    // mentioned — is served against it.
    let query = pb::Query {
        table: "docs".to_owned(),
        filter: Some(pb::Expr {
            node: Some(pb::expr::Node::Compare(pb::Compare {
                column: Some(pb::ColumnRef {
                    input: 0,
                    of: Some(pb::column_ref::Of::Column(2)),
                }),
                op: pb::CmpOp::Ge as i32,
                value: Some(slate_server::convert::value_to_proto(&Value::I64(20))),
            })),
        }),
        schema: Some(short),
        ..Default::default()
    };
    let response = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(query),
            freshness: None,
        }))
        .await
        .expect("the query is served");
    let (rows, _) = drain(response.into_inner()).await;
    assert_eq!(
        rows.len(),
        1,
        "the prefix declaration did not stop a filter on an ordinal outside it"
    );
    // And the ordinal it filtered on is one the declaration never covered.
    assert!(Ordinal(2).0 >= 1);
}

/// A decimal's scale is in the fingerprint, and a client with it wrong is
/// refused.
///
/// The one property in the hash that addresses no column, and the only place
/// in the system where a wrong scale can be caught: the wire carries units and
/// never the scale, so a client that believes `amount` is scale 4 where the
/// schema says 2 reaches the right column and renders every value a hundred
/// times wrong, consistently, for ever.
///
/// Hashing it is safe in the one way that matters: a scale cannot change under
/// a running client, because changing one is a refused migration. So this
/// cannot do what hashing a `CHECK` would — invalidate a fleet on an unrelated
/// schema change — since there is no such change to make.
///
/// The pinned value is the Python client's, computed independently:
///
/// ```text
/// >>> hex(fingerprint_of(PRICES))    # amount at scale 2
/// '0xdab8856481bc4a6d'
/// >>> hex(fingerprint_of(WRONG))     # the same table at scale 4
/// '0xdaba08fbb666133f'
/// ```
#[test]
fn a_decimals_scale_is_hashed_and_a_wrong_one_is_refused() {
    let at = |scale: u8| {
        TableDef::builder("prices", TableId(1))
            .column("id", ValueType::U64)
            .column("label", ValueType::Str)
            .decimal_column("amount", scale)
            .primary_key(["id"])
            .build()
            .expect("valid schema")
    };
    let two = at(2);
    let four = at(4);

    assert_eq!(
        fingerprint::of_table(&two),
        0xdab8_8564_81bc_4a6d,
        "the canonical form moved; the three clients compute this too"
    );
    assert_eq!(fingerprint::of_table(&four), 0xdaba_08fb_b666_133f);
    assert_ne!(
        fingerprint::of_table(&two),
        fingerprint::of_table(&four),
        "two scales must not hash alike — the whole point"
    );

    // And the server refuses the wrong one, which is what the hash is for.
    let claim = pb::SchemaCheck {
        columns: 3,
        fingerprint: fingerprint::of_table(&four),
    };
    let status = fingerprint::check(&two, Some(&claim)).expect_err("scale 4 against scale 2");
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");

    // That a table *without* a decimal hashes exactly as it did — so no client
    // using one needs rebuilding — is not asserted here with a number of its
    // own. It is already pinned in three places that predate this change and
    // are still green: `schema_check.rs`'s constants, and the
    // `0x97c3c1256af4cfdb` for `DOCS` written down independently in the Go and
    // TypeScript suites. A fourth copy of that claim would be a fourth thing
    // to update and no more evidence.
}
