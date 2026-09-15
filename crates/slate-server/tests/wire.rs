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

use common::{authors, books, docs};
use proptest::prelude::*;
use proptest::strategy::ValueTree as _;
use slate_kernel::query::{AccessHint, NullsOrder, Query, SortKey};
use slate_kernel::{
    Aggregate, CalendarPart, CmpOp, Expr, JoinSchema, Metric, Projection, Scalar, ScanOrder,
    TimeUnit,
};
use slate_schema::{IndexId, Ordinal};
use slate_server::convert::{
    Input, Space, aggregate_from_proto, aggregate_to_proto, column_ref, computed_ref,
    expr_from_proto, expr_to_proto, query_from_proto, query_to_proto, row_from_proto, row_to_proto,
    scalar_from_proto, scalar_to_proto, value_from_proto, value_to_proto,
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
        let table = docs();
        let space = Space::table(&table);
        let back = expr_from_proto(&space, &expr_to_proto(&space, &expr))
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
    let table = docs();
    let space = Space::table(&table);
    let expr = Expr::ilike(Ordinal(1), "abc%");
    let mut wire = expr_to_proto(&space, &expr);
    match &mut wire.node {
        Some(pb::expr::Node::Like(like)) => like.insensitive = false,
        other => panic!("ILIKE did not convert to a Like node: {other:?}"),
    }
    let damaged = expr_from_proto(&space, &wire).expect("still a valid predicate");
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
    let table = docs();
    let wire = pb::Expr {
        node: Some(pb::expr::Node::Compare(pb::Compare {
            column: Some(column_ref(0, 0)),
            op: pb::CmpOp::Unspecified as i32,
            value: Some(value_to_proto(&Value::U64(1))),
        })),
    };
    let error = expr_from_proto(&Space::table(&table), &wire).expect_err("must be refused");
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
}

#[test]
fn an_expression_with_no_node_is_refused() {
    let table = docs();
    let error = expr_from_proto(&Space::table(&table), &pb::Expr { node: None })
        .expect_err("must be refused");
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
}

/// `Freshness` used to have two ways to spell "any": the `any` arm, and the
/// `latest` arm set to false — which is what a client that zeroed the struct
/// sent. The server read the second as `ANY`, for the right reason (routing
/// every zeroed read to the writer would point the fleet at the scarcest
/// resource in the deployment) and with the wrong outcome: a wire field that
/// meant something other than what it said, in a file whose opening argument
/// is that an unset `oneof` must never be defaulted.
///
/// Both arms are `Unit` now, so the false spelling does not exist. What a
/// client with nothing to say sends is an absent message, which has always
/// meant `ANY` — the assertion above.
#[test]
fn a_freshness_level_cannot_be_spelled_as_a_selected_arm_meaning_no() {
    use slate_server::convert::freshness_from_proto;

    // The only value `Unit` has. Anything else is a client built against a
    // schema this server does not have, and is refused rather than read as
    // `UNIT` — which is what makes the false spelling unrepresentable rather
    // than merely discouraged.
    for arm in [
        pb::freshness::Level::Latest(1),
        pb::freshness::Level::Any(1),
        pb::freshness::Level::Latest(-1),
    ] {
        let message = pb::Freshness { level: Some(arm) };
        let status = freshness_from_proto(Some(&message))
            .expect_err("a Unit arm carrying anything but UNIT is refused");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(status.message().contains("UNIT"), "{}", status.message());
    }
}

// --- computed values ------------------------------------------------------

fn any_metric() -> impl Strategy<Value = Metric> {
    prop_oneof![
        Just(Metric::L2),
        Just(Metric::L2Squared),
        Just(Metric::Cosine),
        Just(Metric::NegativeInnerProduct),
    ]
}

fn any_unit() -> impl Strategy<Value = TimeUnit> {
    prop_oneof![
        Just(TimeUnit::Second),
        Just(TimeUnit::Minute),
        Just(TimeUnit::Hour),
        Just(TimeUnit::Day),
    ]
}

/// Every [`Scalar`] variant, including the two that carry an enum whose
/// default would be a different answer rather than an error.
fn any_scalar() -> impl Strategy<Value = Scalar> {
    let leaf = prop_oneof![
        any_ordinal().prop_map(Scalar::Column),
        any_value().prop_map(Scalar::Literal),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Scalar::Add(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Scalar::Sub(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Scalar::Mul(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Scalar::Div(Box::new(a), Box::new(b))),
            inner.clone().prop_map(|a| Scalar::Length(Box::new(a))),
            prop::collection::vec(inner.clone(), 0..3).prop_map(Scalar::Concat),
            inner.clone().prop_map(|a| Scalar::Lower(Box::new(a))),
            inner.clone().prop_map(|a| Scalar::Upper(Box::new(a))),
            (any_unit(), inner.clone()).prop_map(|(unit, value)| Scalar::Extract {
                unit,
                value: Box::new(value)
            }),
            (any_unit(), inner.clone()).prop_map(|(unit, value)| Scalar::DateTrunc {
                unit,
                value: Box::new(value)
            }),
            (
                prop::collection::vec((any_expr(), inner.clone()), 0..2),
                inner.clone()
            )
                .prop_map(|(branches, otherwise)| Scalar::Case {
                    branches,
                    otherwise: Box::new(otherwise)
                }),
            prop::collection::vec(inner.clone(), 0..3).prop_map(Scalar::Coalesce),
            (inner.clone(), inner.clone(), any_metric()).prop_map(|(left, right, metric)| {
                Scalar::Distance {
                    left: Box::new(left),
                    right: Box::new(right),
                    metric,
                }
            }),
            (inner.clone(), ".{0,6}", ".{0,6}").prop_map(|(value, pattern, replacement)| {
                Scalar::RegexpReplace {
                    value: Box::new(value),
                    pattern,
                    replacement,
                }
            }),
            (inner.clone(), any_calendar_part()).prop_map(|(value, part)| {
                Scalar::CalendarPart {
                    part,
                    value: Box::new(value),
                }
            }),
            inner.prop_map(|value| Scalar::Round(Box::new(value))),
        ]
    })
}

fn any_calendar_part() -> impl Strategy<Value = CalendarPart> {
    prop_oneof![
        Just(CalendarPart::Year),
        Just(CalendarPart::Month),
        Just(CalendarPart::DayOfMonth),
        Just(CalendarPart::DayOfWeek),
    ]
}

fn any_aggregate() -> impl Strategy<Value = Aggregate> {
    prop_oneof![
        Just(Aggregate::Count),
        any_ordinal().prop_map(Aggregate::CountColumn),
        any_ordinal().prop_map(Aggregate::Min),
        any_ordinal().prop_map(Aggregate::Max),
        any_ordinal().prop_map(Aggregate::Sum),
        any_ordinal().prop_map(Aggregate::Avg),
        any_ordinal().prop_map(Aggregate::CountDistinct),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn a_computed_value_survives_the_round_trip(scalar in any_scalar()) {
        let table = docs();
        let space = Space::table(&table);
        let back = scalar_from_proto(&space, &scalar_to_proto(&space, &scalar))
            .expect("a computed value this server produced must be one it can read");
        prop_assert_eq!(&back, &scalar, "converting {:?} out and back changed it", scalar);
    }

    #[test]
    fn an_aggregate_survives_the_round_trip(aggregate in any_aggregate()) {
        let table = docs();
        let space = Space::table(&table);
        let back = aggregate_from_proto(&space, &aggregate_to_proto(&space, aggregate))
            .expect("an aggregate this server produced must be one it can read");
        prop_assert_eq!(back, aggregate);
    }
}

/// A generator that never produces a `Distance` proves nothing about vectors
/// while passing every case — the codec bug this project already found.
#[test]
fn the_scalar_generator_reaches_every_variant() {
    let mut seen = BTreeSet::new();
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = any_scalar();
    for _ in 0..800 {
        collect_scalars(
            &strategy.new_tree(&mut runner).expect("a scalar").current(),
            &mut seen,
        );
    }
    let expected: BTreeSet<&str> = [
        "column",
        "literal",
        "add",
        "sub",
        "mul",
        "div",
        "length",
        "concat",
        "lower",
        "upper",
        "extract",
        "date_trunc",
        "case",
        "coalesce",
        "distance",
        "regexp_replace",
        "calendar_part",
        "round",
    ]
    .into_iter()
    .collect();
    assert_eq!(seen, expected, "some Scalar variants were never generated");
}

fn collect_scalars(scalar: &Scalar, into: &mut BTreeSet<&'static str>) {
    // Exhaustive on purpose: `Scalar` is not `#[non_exhaustive]`, so a new
    // kernel variant fails to compile here rather than going untested.
    match scalar {
        Scalar::Column(_) => {
            into.insert("column");
        }
        Scalar::Literal(_) => {
            into.insert("literal");
        }
        Scalar::Add(a, b) => {
            into.insert("add");
            collect_scalars(a, into);
            collect_scalars(b, into);
        }
        Scalar::Sub(a, b) => {
            into.insert("sub");
            collect_scalars(a, into);
            collect_scalars(b, into);
        }
        Scalar::Mul(a, b) => {
            into.insert("mul");
            collect_scalars(a, into);
            collect_scalars(b, into);
        }
        Scalar::Div(a, b) => {
            into.insert("div");
            collect_scalars(a, into);
            collect_scalars(b, into);
        }
        Scalar::Length(a) => {
            into.insert("length");
            collect_scalars(a, into);
        }
        Scalar::Concat(parts) => {
            into.insert("concat");
            for part in parts {
                collect_scalars(part, into);
            }
        }
        Scalar::Lower(a) => {
            into.insert("lower");
            collect_scalars(a, into);
        }
        Scalar::Upper(a) => {
            into.insert("upper");
            collect_scalars(a, into);
        }
        Scalar::Extract { value, .. } => {
            into.insert("extract");
            collect_scalars(value, into);
        }
        Scalar::DateTrunc { value, .. } => {
            into.insert("date_trunc");
            collect_scalars(value, into);
        }
        Scalar::Case {
            branches,
            otherwise,
        } => {
            into.insert("case");
            for (_, then) in branches {
                collect_scalars(then, into);
            }
            collect_scalars(otherwise, into);
        }
        Scalar::Coalesce(parts) => {
            into.insert("coalesce");
            for part in parts {
                collect_scalars(part, into);
            }
        }
        Scalar::Distance { left, right, .. } => {
            into.insert("distance");
            collect_scalars(left, into);
            collect_scalars(right, into);
        }
        Scalar::RegexpReplace { value, .. } => {
            into.insert("regexp_replace");
            collect_scalars(value, into);
        }
        Scalar::CalendarPart { value, .. } => {
            into.insert("calendar_part");
            collect_scalars(value, into);
        }
        Scalar::Round(value) => {
            into.insert("round");
            collect_scalars(value, into);
        }
    }
}

/// Proof that the computed-value round trip is sharp enough to see a changed
/// unit.
///
/// `TIME_UNIT_MINUTE` and `TIME_UNIT_HOUR` are one integer apart on the wire
/// and produce plausible numbers for different questions, which is exactly the
/// failure a round trip that only compared shapes would let through.
#[test]
fn the_round_trip_notices_a_changed_time_unit() {
    let table = docs();
    let space = Space::table(&table);
    let scalar = Scalar::column(Ordinal(2)).date_trunc(TimeUnit::Hour);
    let mut wire = scalar_to_proto(&space, &scalar);
    match &mut wire.node {
        Some(pb::scalar::Node::DateTrunc(part)) => {
            part.unit = pb::TimeUnit::Minute as i32;
        }
        other => panic!("date_trunc did not convert to a DateTrunc node: {other:?}"),
    }
    let damaged = scalar_from_proto(&space, &wire).expect("still a valid computed value");
    assert_ne!(
        damaged, scalar,
        "the round trip cannot tell an hour from a minute"
    );
}

// --- the ordinal model ----------------------------------------------------

/// The wire's arithmetic is the kernel's arithmetic.
///
/// `ColumnRef` exists so the client never computes an offset. The server still
/// has to, and this is the check that it computes the same one `JoinSchema`
/// does — a mismatch would put a predicate on the wrong table with no error
/// anywhere.
#[test]
fn a_column_reference_resolves_to_the_ordinal_the_kernel_would_use() {
    let (a, b) = (authors(), books());
    let space = Space::joined(vec![Input::new(&a, 0), Input::new(&b, 0)], 2);
    let kernel = JoinSchema::of(&a, &b);

    for ordinal in 0..a.columns().len() {
        assert_eq!(
            space
                .resolve(Some(&column_ref(0, ordinal)), "a test")
                .unwrap(),
            kernel.left(Ordinal(ordinal)),
        );
    }
    for ordinal in 0..b.columns().len() {
        assert_eq!(
            space
                .resolve(Some(&column_ref(1, ordinal)), "a test")
                .unwrap(),
            kernel.right(Ordinal(ordinal)),
            "input 1's column {ordinal} did not land where JoinSchema puts it"
        );
    }
    // And back again, which is what `join_to_proto` relies on.
    assert_eq!(
        space.unresolve(kernel.right(Ordinal(3))),
        column_ref(1, 3),
        "an ordinal in the joined space did not come back as the input that owns it"
    );
}

/// A computed value's slot is the one the kernel's `Query::computed` names.
#[test]
fn a_computed_reference_resolves_where_query_computed_puts_it() {
    let table = docs();
    let space = Space::input(&table, 2, 0);
    for at in 0..2 {
        assert_eq!(
            space.resolve(Some(&computed_ref(0, at)), "a test").unwrap(),
            Query::computed(&table, at),
        );
    }
    // One past the end is refused rather than read as a column of some other
    // table, which is the whole reason this is a kind and not an ordinal.
    assert!(
        space.resolve(Some(&computed_ref(0, 2)), "a test").is_err(),
        "a computed value the query does not compute must be refused"
    );
}

/// A chain row carries one entry per input, whatever length the kernel's row
/// happens to be.
///
/// A client reads a joined row positionally, so a row shorter than the chain
/// would shift every input past the gap rather than error — which is worse
/// than any wrong value, because nothing anywhere would say so.
#[test]
fn a_chain_row_is_padded_to_one_entry_per_input() {
    use slate_kernel::ChainRow;
    use slate_server::convert::chain_row_values;

    let row = ChainRow::start(slate_schema::Row::new(vec![Value::U64(1)]));
    let padded = chain_row_values(&row, 3);
    assert_eq!(padded.len(), 3, "a one-table row was not padded to three");
    assert!(padded[0].is_some());
    assert_eq!(padded[1], None);
    assert_eq!(padded[2], None);
}

/// A grouped predicate addresses keys and aggregates, and nothing else.
#[test]
fn a_grouped_space_refuses_a_raw_column() {
    let space = Space::groups(2, 3);
    assert_eq!(
        space
            .resolve(
                Some(&pb::ColumnRef {
                    input: 0,
                    of: Some(pb::column_ref::Of::Aggregate(2)),
                }),
                "a test"
            )
            .unwrap(),
        Ordinal(4),
        "the third aggregate of a two-key group is ordinal 4"
    );
    let error = space
        .resolve(Some(&column_ref(0, 0)), "the HAVING condition")
        .expect_err("a raw column is not addressable over groups");
    assert!(
        error.message().contains("not grouped"),
        "{}",
        error.message()
    );
}

/// A query that computes values survives the round trip, including a computed
/// value read by a later one, by the filter and by the sort.
#[test]
fn a_query_with_computed_values_survives_the_round_trip() {
    let table = docs();
    let first = Query::computed(&table, 0);
    let second = Query::computed(&table, 1);
    let query = Query::all()
        .computing([
            Scalar::column(Ordinal(2)) + 1i64,
            Scalar::column(first) * 2i64,
        ])
        .filter(Expr::compare(second, CmpOp::Gt, Value::I64(0)))
        .sort_by([SortKey::desc(first)]);

    let wire = query_to_proto(&table, &query);
    let (back, warnings) = query_from_proto(&wire, &table).expect("readable");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(back, query);
}
