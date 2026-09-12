//! The wire conversion, in both directions.
//!
//! A conversion layer is the most boring code in a server and the easiest place
//! to lose something. The `ORDER BY` bug this project already found — a sort
//! column that was never decoded, so every row compared equal and the sort
//! silently did nothing — is exactly the shape a conversion bug takes: nothing
//! errors, the answer is merely different.
//!
//! So the test is not "does this field convert", which is the question whoever
//! wrote the field already asked. It is the round trip: for any kernel value,
//! converting out and back must give the same value. A field dropped on either
//! side fails it, including a field added later that nobody remembers to
//! convert, and including a *flag* on a field that does convert —
//! `the_round_trip_notices_a_dropped_flag` pins that the property is sharp
//! enough to see one.
//!
//! The generators are checked too. A generator that never produces a `Vector`
//! proves nothing about vectors while passing every case, which is how the
//! codec came to accept one on encode and refuse it on decode.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::docs;
use proptest::prelude::*;
use proptest::strategy::ValueTree as _;
use slate_kernel::query::{AccessHint, NullsOrder, Query, SortKey};
use slate_kernel::{CmpOp, Expr, Projection, ScanOrder};
use slate_schema::{IndexId, Ordinal};
use slate_server::convert::{
    expr_from_proto, expr_to_proto, query_from_proto, query_to_proto, row_from_proto, row_to_proto,
    value_from_proto, value_to_proto,
};
use slate_server::proto as pb;
use slate_tuple::{Direction, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Every [`Value`] variant, including the ones that are easy to forget.
///
/// `f64` is drawn from a set that includes the two values whose equality is a
/// choice rather than a fact: `Value`'s order canonicalises NaN and treats
/// `-0.0` and `0.0` as distinct byte patterns, so both belong in the round
/// trip.
fn any_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        prop::collection::vec(any::<u8>(), 0..24)
            .prop_map(|bytes| Value::Bytes(bytes::Bytes::from(bytes))),
        ".{0,24}".prop_map(Value::Str),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_map(Value::U64),
        prop_oneof![
            any::<f64>(),
            Just(f64::NAN),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            Just(-0.0_f64),
        ]
        .prop_map(Value::F64),
        any::<[u8; 16]>().prop_map(|bytes| Value::Uuid(Uuid::from_bytes(bytes))),
        prop::collection::vec(any::<f32>(), 0..8).prop_map(Value::Vector),
    ]
}

fn any_op() -> impl Strategy<Value = CmpOp> {
    prop_oneof![
        Just(CmpOp::Eq),
        Just(CmpOp::Ne),
        Just(CmpOp::Lt),
        Just(CmpOp::Le),
        Just(CmpOp::Gt),
        Just(CmpOp::Ge),
    ]
}

/// Column ordinals inside the `docs` fixture, so a generated predicate can also
/// be fed through `query_from_proto`, which checks them.
fn any_ordinal() -> impl Strategy<Value = Ordinal> {
    (0_usize..4).prop_map(Ordinal)
}

fn any_expr() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        Just(Expr::True),
        Just(Expr::False),
        (any_ordinal(), any_op(), any_value()).prop_map(|(column, op, value)| Expr::Compare {
            column,
            op,
            value
        }),
        (any_ordinal(), any_op(), any_ordinal())
            .prop_map(|(left, op, right)| Expr::CompareColumns { left, op, right }),
        (any_ordinal(), any::<bool>())
            .prop_map(|(column, negated)| Expr::IsNull { column, negated }),
        (any_ordinal(), ".{0,8}", any::<bool>(), any::<bool>()).prop_map(
            |(column, pattern, negated, insensitive)| Expr::Like {
                column,
                pattern,
                negated,
                insensitive
            }
        ),
        (any_ordinal(), ".{0,8}", any::<bool>(), any::<bool>()).prop_map(
            |(column, pattern, negated, insensitive)| Expr::Matches {
                column,
                pattern,
                negated,
                insensitive
            }
        ),
        (any_ordinal(), prop::collection::vec(any_value(), 0..4))
            .prop_map(|(column, values)| Expr::In { column, values }),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Expr::And),
            prop::collection::vec(inner.clone(), 0..3).prop_map(Expr::Or),
            inner.prop_map(|e| Expr::Not(Box::new(e))),
        ]
    })
}

fn any_query() -> impl Strategy<Value = Query> {
    (
        any_expr(),
        prop_oneof![Just(ScanOrder::Ascending), Just(ScanOrder::Descending)],
        prop_oneof![
            Just(Projection::All),
            prop::collection::vec(any_ordinal(), 0..4).prop_map(Projection::Columns),
        ],
        prop::collection::vec(
            (
                any_ordinal(),
                prop_oneof![Just(Direction::Asc), Just(Direction::Desc)],
                prop_oneof![Just(NullsOrder::First), Just(NullsOrder::Last)],
            )
                .prop_map(|(column, direction, nulls)| SortKey {
                    column,
                    direction,
                    nulls,
                }),
            0..3,
        ),
        prop::option::of(0_usize..100),
        0_usize..50,
        prop_oneof![
            Just(None),
            Just(Some(AccessHint::TableScan)),
            Just(Some(AccessHint::Index(IndexId(1)))),
            Just(Some(AccessHint::Index(IndexId(2)))),
        ],
    )
        .prop_map(
            |(filter, order, projection, sort, limit, offset, hint)| Query {
                filter,
                order,
                projection,
                sort,
                limit,
                offset,
                hint,
                compute: Vec::new(),
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    /// The property the whole module exists for.
    #[test]
    fn a_value_survives_the_round_trip(value in any_value()) {
        let back = value_from_proto(&value_to_proto(&value))
            .expect("a value this server produced must be one it can read");
        prop_assert_eq!(&back, &value, "converting {:?} out and back changed it", value);
    }

    #[test]
    fn a_row_survives_the_round_trip(values in prop::collection::vec(any_value(), 0..6)) {
        let row = slate_schema::Row::new(values);
        let back = row_from_proto(&row_to_proto(&row)).expect("readable");
        prop_assert_eq!(back, row);
    }

    #[test]
    fn a_predicate_survives_the_round_trip(expr in any_expr()) {
        let back = expr_from_proto(&expr_to_proto(&expr))
            .expect("a predicate this server produced must be one it can read");
        prop_assert_eq!(&back, &expr, "converting {:?} out and back changed it", expr);
    }

    /// The whole request, including the parts a client is most likely to think
    /// are cosmetic: sort direction, where nulls go, the offset, the hint.
    #[test]
    fn a_query_survives_the_round_trip(query in any_query()) {
        let table = docs();
        let wire = query_to_proto(&table, &query);
        let (back, warnings) = query_from_proto(&wire, &table)
            .expect("a query this server produced must be one it can read");
        prop_assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        prop_assert_eq!(back, query);
    }
}

/// A property suite is only as good as what its generators reach.
///
/// Written after the codec bug where a generator had never been extended to
/// produce vectors, so the vector path passed every case by never being tried.
#[test]
fn the_value_generator_reaches_every_variant() {
    let mut seen = BTreeSet::new();
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = any_value();
    for _ in 0..500 {
        let value = strategy.new_tree(&mut runner).expect("a value").current();
        seen.insert(variant_of(&value));
    }
    let expected: BTreeSet<&str> = [
        "null", "bool", "bytes", "str", "i64", "u64", "f64", "uuid", "vector",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        seen, expected,
        "the generator never produced some variants, so the round trip never tested them"
    );
}

fn variant_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Bytes(_) => "bytes",
        Value::Str(_) => "str",
        Value::I64(_) => "i64",
        Value::U64(_) => "u64",
        Value::F64(_) => "f64",
        Value::Uuid(_) => "uuid",
        Value::Vector(_) => "vector",
        other => panic!("a new Value variant is not covered here: {other:?}"),
    }
}

#[test]
fn the_expression_generator_reaches_every_variant() {
    let mut seen = BTreeSet::new();
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = any_expr();
    for _ in 0..500 {
        collect_variants(
            &strategy.new_tree(&mut runner).expect("an expr").current(),
            &mut seen,
        );
    }
    let expected: BTreeSet<&str> = [
        "true",
        "false",
        "compare",
        "compare_columns",
        "is_null",
        "like",
        "matches",
        "in",
        "and",
        "or",
        "not",
    ]
    .into_iter()
    .collect();
    assert_eq!(seen, expected, "some Expr variants were never generated");
}

fn collect_variants(expr: &Expr, into: &mut BTreeSet<&'static str>) {
    match expr {
        Expr::True => {
            into.insert("true");
        }
        Expr::False => {
            into.insert("false");
        }
        Expr::Compare { .. } => {
            into.insert("compare");
        }
        Expr::CompareColumns { .. } => {
            into.insert("compare_columns");
        }
        Expr::IsNull { .. } => {
            into.insert("is_null");
        }
        Expr::Like { .. } => {
            into.insert("like");
        }
        Expr::Matches { .. } => {
            into.insert("matches");
        }
        Expr::In { .. } => {
            into.insert("in");
        }
        Expr::And(parts) => {
            into.insert("and");
            for part in parts {
                collect_variants(part, into);
            }
        }
        Expr::Or(parts) => {
            into.insert("or");
            for part in parts {
                collect_variants(part, into);
            }
        }
        Expr::Not(inner) => {
            into.insert("not");
            collect_variants(inner, into);
        }
        other => panic!("a new Expr variant is not covered here: {other:?}"),
    }
}

/// Proof that the round trip is sharp enough to see a dropped flag.
///
/// A round-trip test that only compares the *shape* of a value would pass while
/// `ILIKE` silently became `LIKE`, which is a different query and, on a table
/// under a policy, potentially a different set of rows. Damaging the wire form
/// by hand and requiring the comparison to notice is what says the property has
/// teeth.
#[test]
fn the_round_trip_notices_a_dropped_flag() {
    let expr = Expr::ilike(Ordinal(1), "abc%");
    let mut wire = expr_to_proto(&expr);
    match &mut wire.node {
        Some(pb::expr::Node::Like(like)) => like.insensitive = false,
        other => panic!("ILIKE did not convert to a Like node: {other:?}"),
    }
    let damaged = expr_from_proto(&wire).expect("still a valid predicate");
    assert_ne!(
        damaged, expr,
        "the round trip cannot tell ILIKE from LIKE, so it would not catch losing the flag"
    );
}

// --- what a client cannot get away with -----------------------------------

#[test]
fn a_value_with_no_kind_is_refused_rather_than_read_as_null() {
    // proto3 cannot distinguish an unset field from a zero one, so an unset
    // `kind` most likely means a client built against a newer schema. Reading
    // it as null would quietly change the predicate it appears in.
    let error = value_from_proto(&pb::Value { kind: None }).expect_err("must be refused");
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
}

#[test]
fn a_uuid_of_the_wrong_length_is_refused() {
    let wire = pb::Value {
        kind: Some(pb::value::Kind::UuidValue(vec![0; 15])),
    };
    let error = value_from_proto(&wire).expect_err("15 bytes is not a uuid");
    assert!(
        error.message().contains("16 bytes"),
        "the message should say what was wrong: {}",
        error.message()
    );
}

#[test]
fn an_unspecified_comparison_operator_is_refused() {
    // Defaulting it would make a malformed comparison quietly mean equality.
    let wire = pb::Expr {
        node: Some(pb::expr::Node::Compare(pb::Compare {
            column: 0,
            op: pb::CmpOp::Unspecified as i32,
            value: Some(value_to_proto(&Value::U64(1))),
        })),
    };
    let error = expr_from_proto(&wire).expect_err("must be refused");
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
}

#[test]
fn an_expression_with_no_node_is_refused() {
    let error = expr_from_proto(&pb::Expr { node: None }).expect_err("must be refused");
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
}

#[test]
fn a_column_past_the_end_of_the_table_is_refused() {
    // The kernel treats a missing column as unknown, which is right for
    // evaluation and useless as a diagnosis: a predicate on ordinal 9 of a
    // four-column table silently matches nothing.
    let table = docs();
    let query = query_to_proto(
        &table,
        &Query::all().filter(Expr::eq(Ordinal(9), Value::U64(1))),
    );
    let error = query_from_proto(&query, &table).expect_err("must be refused");
    assert!(
        error.message().contains("has 4 columns"),
        "the message should name the width: {}",
        error.message()
    );
}

#[test]
fn a_hint_naming_an_index_that_does_not_exist_is_a_warning_not_an_error() {
    // The kernel's rule: a hint is advice, and a query that stops working
    // because an index was renamed is worse than one that gets slower. But a
    // hint that silently does nothing is undebuggable, so it is reported.
    let table = docs();
    let mut wire = query_to_proto(&table, &Query::all());
    wire.hint = Some(pb::AccessHint {
        path: Some(pb::access_hint::Path::Index("by_nothing".to_owned())),
    });

    let (query, warnings) = query_from_proto(&wire, &table).expect("still a valid query");
    assert!(query.hint.is_none(), "an unusable hint must be dropped");
    assert_eq!(warnings.len(), 1, "and reported: {warnings:?}");
    assert!(warnings[0].contains("by_nothing"), "{}", warnings[0]);
}

#[test]
fn an_absent_freshness_means_any_replica_will_do() {
    use slate_kernel::Freshness;
    use slate_server::convert::freshness_from_proto;
    assert_eq!(freshness_from_proto(None).unwrap(), Freshness::Any);

    // A client that zeroed the message sends `latest: false`. Reading that as
    // a request for the writer would send every such read to the scarcest
    // resource in the deployment.
    let zeroed = pb::Freshness {
        level: Some(pb::freshness::Level::Latest(false)),
    };
    assert_eq!(
        freshness_from_proto(Some(&zeroed)).unwrap(),
        Freshness::Any,
        "`latest: false` is not a request for the writer"
    );
}
