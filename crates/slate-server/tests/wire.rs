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
    Aggregate, CalendarPart, CalendarUnit, CmpOp, Expr, JoinSchema, Metric, Projection, Scalar,
    ScanOrder, TimeUnit, Window, WindowFunction,
};
use slate_schema::{IndexId, Ordinal};
use slate_server::convert::{
    Input, MAX_EXPRESSION_DEPTH, Space, aggregate_from_proto, aggregate_to_proto, column_ref,
    computed_ref, expr_from_proto, expr_to_proto, query_from_proto, query_to_proto, row_from_proto,
    row_to_proto, scalar_from_proto, scalar_to_proto, value_from_proto, value_to_proto,
};
use slate_server::proto as pb;
use slate_tuple::{Direction, Value, ValueType};
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
        // `Decimal` was missing here from the day it was added, and so was
        // this file's expected list — so the round trip has never once
        // converted one. See `the_value_generator_reaches_every_variant`,
        // which is the guard that was supposed to catch exactly this and
        // could not, because it compared the generator against a list
        // maintained by the same hand.
        any::<i64>().prop_map(Value::Decimal),
        // Elements are drawn from the same set minus arrays, because the
        // server refuses a nested one — see `value_from_proto`.
        prop::collection::vec(any_element(), 0..5).prop_map(Value::Array),
    ]
}

/// What may appear inside an array on the wire: anything but another array.
fn any_element() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        prop::collection::vec(any::<u8>(), 0..8)
            .prop_map(|bytes| Value::Bytes(bytes::Bytes::from(bytes))),
        ".{0,8}".prop_map(Value::Str),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_map(Value::U64),
        any::<f64>().prop_map(Value::F64),
        any::<i64>().prop_map(Value::Decimal),
        any::<[u8; 16]>().prop_map(|bytes| Value::Uuid(Uuid::from_bytes(bytes))),
        prop::collection::vec(any::<f32>(), 0..4).prop_map(Value::Vector),
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

/// One sort key over the `docs` fixture's columns.
fn any_sort_key() -> impl Strategy<Value = SortKey> {
    (
        any_ordinal(),
        prop_oneof![Just(Direction::Asc), Just(Direction::Desc)],
        prop_oneof![Just(NullsOrder::First), Just(NullsOrder::Last)],
    )
        .prop_map(|(column, direction, nulls)| SortKey {
            column,
            direction,
            nulls,
        })
}

/// A window the kernel would accept.
///
/// Two arms rather than one, because the `ORDER BY` is not free-floating: a
/// ranking function refuses without one and a running `COUNT(DISTINCT)`
/// refuses *with* one, so generating the order independently of the function
/// would spend most cases on inputs `Window::new` rejects — and `expect` below
/// would then be testing the generator rather than the round trip.
fn any_window() -> impl Strategy<Value = Window> {
    let ordered = (
        prop_oneof![
            Just(WindowFunction::RowNumber),
            Just(WindowFunction::Rank),
            Just(WindowFunction::DenseRank),
            (any_ordinal(), 1_usize..4)
                .prop_map(|(column, offset)| WindowFunction::Lag { column, offset }),
            (any_ordinal(), 1_usize..4)
                .prop_map(|(column, offset)| WindowFunction::Lead { column, offset }),
            Just(WindowFunction::Over(Aggregate::Count)),
            any_ordinal().prop_map(|c| WindowFunction::Over(Aggregate::Sum(c))),
            any_ordinal().prop_map(|c| WindowFunction::Over(Aggregate::Max(c))),
        ],
        prop::collection::vec(any_ordinal(), 0..3),
        prop::collection::vec(any_sort_key(), 1..3),
    );
    // The unordered half, which is the *only* place a whole-partition frame
    // and a windowed `COUNT(DISTINCT)` are reachable.
    let unordered = (
        prop_oneof![
            Just(WindowFunction::Over(Aggregate::Count)),
            any_ordinal().prop_map(|c| WindowFunction::Over(Aggregate::Avg(c))),
            any_ordinal().prop_map(|c| WindowFunction::Over(Aggregate::CountDistinct(c))),
        ],
        prop::collection::vec(any_ordinal(), 0..3),
        Just(Vec::new()),
    );
    prop_oneof![ordered, unordered].prop_map(|(function, partition, order)| {
        Window::new(function, partition, order).expect("the generator only builds valid windows")
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
        // A cursor, now that the wire carries one. `Some(vec![])` is left out
        // rather than forgotten: an empty cursor is how the proto spells
        // *absent*, because proto3 cannot tell an unset repeated field from an
        // empty one, so it is the one value that cannot survive the round trip
        // and the one the server reads as `None`.
        prop_oneof![
            Just(None),
            prop::collection::vec(any_value(), 1..3).prop_map(Some),
        ],
    )
        // A second tuple because `prop::strategy` tuples stop at twelve arms
        // and the first is full.
        .prop_flat_map(|first| {
            (
                Just(first),
                any::<bool>(),
                // Two at most, and that is deliberate rather than a cost
                // saving: one window cannot catch a converter that drops the
                // *second*, and the two also exercise the path where a pair
                // shares a specification.
                prop::collection::vec(any_window(), 0..3),
            )
        })
        .prop_map(
            |(
                (filter, order, projection, sort, limit, offset, hint, after),
                include_deleted,
                window,
            )| {
                Query {
                    filter,
                    order,
                    projection,
                    sort,
                    limit,
                    offset,
                    hint,
                    compute: Vec::new(),
                    // Generated now that there is a message for it to survive.
                    // The comment this replaces said it would become generated
                    // when there was one — which is what happened, and is the
                    // second time this file has recorded that transition.
                    window,
                    // `paging` is implied by `after` on the way in and is set by
                    // `Query::after`, so a generated `after` must carry it or the
                    // round trip compares a value the builder cannot produce.
                    paging: after.is_some(),
                    after,
                    // It crosses now, so it is generated rather than pinned false.
                    // The comment this replaces said "when it does cross, this
                    // becomes generated" — which is what happened.
                    include_deleted,
                }
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
/// produce vectors, so the vector path passed every case by never being tried
/// — and then this test repeated the mistake it was written against. It
/// compared `any_value` to a **hand-written list of nine names**, and both
/// were missing `decimal`, so from the day decimals were added until now the
/// wire round trip never converted one and this guard said everything was
/// covered.
///
/// The expected set is now `ValueType::ALL` plus `"null"`, which is not a list
/// anybody maintains: `ALL` is held to the enum by a compiler-checked
/// exhaustive match inside `slate-tuple`, so a new variant fails *there*, and
/// then fails here until the generator produces one. `Value::type_name` gives
/// the same names from the same source, so the two sides cannot drift apart
/// either.
#[test]
fn the_value_generator_reaches_every_variant() {
    let mut seen = BTreeSet::new();
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = any_value();
    for _ in 0..500 {
        let value = strategy.new_tree(&mut runner).expect("a value").current();
        seen.insert(value.type_name());
    }
    let mut expected: BTreeSet<&str> = ValueType::ALL.iter().map(|kind| kind.name()).collect();
    // Null has no `ValueType` — nullability is a column property — so it is
    // the one name that has to be added by hand, and it is a constant rather
    // than a list that can go one short.
    expected.insert("null");
    assert_eq!(
        seen, expected,
        "the generator never produced some variants, so the round trip never tested them"
    );
}

/// The array generator reaches everything the outer one does, bar arrays.
///
/// Same argument as above, one level down: `a_value_survives_the_round_trip`
/// converts an array by converting its elements, so an element kind the
/// generator never produces is an element kind the round trip never sees.
#[test]
fn the_array_element_generator_reaches_every_variant_but_array() {
    let mut seen = BTreeSet::new();
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = any_element();
    for _ in 0..500 {
        let value = strategy.new_tree(&mut runner).expect("a value").current();
        seen.insert(value.type_name());
    }
    let mut expected: BTreeSet<&str> = ValueType::ALL
        .iter()
        .filter(|kind| **kind != ValueType::Array)
        .map(|kind| kind.name())
        .collect();
    expected.insert("null");
    assert_eq!(seen, expected);
}

/// A nested array is refused, with a `Status` rather than a panic or a stack.
///
/// The depth here is chosen by whoever sends the message, which is why this is
/// the server's refusal and not only the kernel's: refusing at depth one means
/// there is no depth to bound.
#[test]
fn a_nested_array_is_refused() {
    use pb::value::Kind;
    let inner = pb::Value {
        kind: Some(Kind::ArrayValue(pb::ArrayValue { elements: vec![] })),
    };
    let outer = pb::Value {
        kind: Some(Kind::ArrayValue(pb::ArrayValue {
            elements: vec![inner],
        })),
    };
    let error = value_from_proto(&outer).expect_err("an array of arrays must be refused");
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
    assert!(
        error.message().contains("element type"),
        "the refusal should say why: {}",
        error.message()
    );
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
            (inner.clone(), any_calendar_unit()).prop_map(|(value, unit)| {
                Scalar::CalendarTrunc {
                    unit,
                    value: Box::new(value),
                }
            }),
            // Drawn from the table rather than from an arbitrary string,
            // because the conversion *refuses* an unknown zone and a refusal
            // is not a round trip. The refusal has its own test below; this
            // one is about a zone shift surviving the wire.
            (inner.clone(), any_zone()).prop_map(|(value, zone)| Scalar::ZoneShift {
                zone,
                value: Box::new(value),
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

fn any_calendar_unit() -> impl Strategy<Value = CalendarUnit> {
    prop_oneof![Just(CalendarUnit::Month), Just(CalendarUnit::Year)]
}

/// One of the zones the kernel's table has, by index.
///
/// Every one of them, rather than a handful: the conversion carries the name
/// through untouched, so a zone it mangles would be one this could not name in
/// advance. Sampling the list means the strategy grows with the table.
fn any_zone() -> impl Strategy<Value = String> {
    let names: Vec<String> = slate_kernel::zones::names().map(str::to_owned).collect();
    assert!(!names.is_empty(), "the zone table is empty");
    (0..names.len()).prop_map(move |at| names[at].clone())
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
        "calendar_trunc",
        "zone_shift",
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
        Scalar::CalendarTrunc { value, .. } => {
            into.insert("calendar_trunc");
            collect_scalars(value, into);
        }
        Scalar::ZoneShift { value, .. } => {
            into.insert("zone_shift");
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

#[test]
fn a_zone_the_server_does_not_have_is_refused_by_name() {
    // The kernel answers null for an unknown zone, because a `Scalar` has
    // nowhere to put an error. Null is the wrong answer to hand a caller who
    // typed `america/new_york`: it is indistinguishable from an empty column,
    // and the mistake is a capital letter. So the boundary refuses, and the
    // refusal has to name what *is* available — a bare "unknown zone" leaves
    // the caller guessing which spelling the server wants.
    let table = docs();
    let space = Space::table(&table);
    for zone in ["america/new_york", "Mars/Olympus_Mons", "EST5EDT", ""] {
        let wire = pb::Scalar {
            node: Some(pb::scalar::Node::ZoneShift(Box::new(pb::ZoneShift {
                zone: zone.to_owned(),
                value: Some(Box::new(scalar_to_proto(
                    &space,
                    &Scalar::Literal(Value::I64(0)),
                ))),
            }))),
        };
        let error = match scalar_from_proto(&space, &wire) {
            Err(error) => error,
            Ok(accepted) => panic!("{zone:?} must be refused, and gave {accepted:?}"),
        };
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert!(
            error.message().contains("America/New_York"),
            "the refusal for {zone:?} should list the zones that exist: {}",
            error.message()
        );
    }

    // And the spelling the table has goes through, so the refusal is about the
    // name and not about zone shifts in general.
    let wire = pb::Scalar {
        node: Some(pb::scalar::Node::ZoneShift(Box::new(pb::ZoneShift {
            zone: "America/New_York".to_owned(),
            value: Some(Box::new(scalar_to_proto(
                &space,
                &Scalar::Literal(Value::I64(0)),
            ))),
        }))),
    };
    scalar_from_proto(&space, &wire).expect("a zone the table has must be accepted");
}

// --- how deep a caller may nest -------------------------------------------

/// How an expression can wrap another, which is every way it can get deeper.
#[derive(Debug, Clone, Copy)]
enum Wrap {
    Not,
    And,
    Or,
}

/// A chain of `wrap`s exactly `n` deep around a literal.
///
/// All three, rather than the `Not` this was written with: a mutation making a
/// *conjunction's* children not count as deeper survived a `Not`-only test,
/// because `And` and `Or` recurse through a different arm and nothing reached
/// it. An `And` chain is also the shape a query builder actually produces, so
/// it is the one a pathological request would use.
fn nested(wrap: Wrap, n: usize) -> pb::Expr {
    let mut expr = pb::Expr {
        node: Some(pb::expr::Node::Literal(true)),
    };
    for _ in 0..n {
        let node = match wrap {
            Wrap::Not => pb::expr::Node::Negation(Box::new(expr)),
            Wrap::And => pb::expr::Node::Conjunction(pb::ExprList { exprs: vec![expr] }),
            Wrap::Or => pb::expr::Node::Disjunction(pb::ExprList { exprs: vec![expr] }),
        };
        expr = pb::Expr { node: Some(node) };
    }
    expr
}

/// The `Not` chain, kept as its own name because two tests read better for it.
fn nested_not(n: usize) -> pb::Expr {
    nested(Wrap::Not, n)
}

/// A chain of `Length`s exactly `n` deep around a column.
fn nested_length(n: usize) -> pb::Scalar {
    let mut scalar = pb::Scalar {
        node: Some(pb::scalar::Node::Column(column_ref(0, 0))),
    };
    for _ in 0..n {
        scalar = pb::Scalar {
            node: Some(pb::scalar::Node::Length(Box::new(scalar))),
        };
    }
    scalar
}

/// The limit is this server's, stated, and it fires.
///
/// It used to be prost's: a message nested past 100 is refused while decoding,
/// so the conversion below never saw a pathological one and its recursion was
/// safe by inheritance. Inheritance is a bad place to leave a limit — it is a
/// dependency's default, so an upgrade that raises it changes this server's
/// contract without anybody reading this file, and a caller who hits it gets a
/// decode error rather than a message naming what was too deep.
///
/// The pair of assertions is the case: at the limit it converts, one past it
/// it does not. Only the second would pass against a limit of zero, and only
/// the first against no limit at all.
#[test]
fn an_expression_may_nest_to_the_limit_and_no_further() {
    let table = docs();
    let space = Space::table(&table);

    // Every way an expression can wrap another. Written with `Not` alone
    // first, which left the `And`/`Or` arm untested — a mutation dropping the
    // increment there survived, and an `And` chain is the shape a query
    // builder produces, so it was the likelier attack of the two.
    for wrap in [Wrap::Not, Wrap::And, Wrap::Or] {
        expr_from_proto(&space, &nested(wrap, MAX_EXPRESSION_DEPTH))
            .unwrap_or_else(|e| panic!("{wrap:?} at the limit should convert: {e:?}"));

        let error = expr_from_proto(&space, &nested(wrap, MAX_EXPRESSION_DEPTH + 1)).unwrap_err();
        assert_eq!(
            error.code(),
            tonic::Code::InvalidArgument,
            "{wrap:?}: {error:?}"
        );
        assert!(
            error.message().contains("nests more than"),
            "{wrap:?}: the refusal should say what was wrong: {}",
            error.message()
        );
    }
}

/// The same budget on a computed value, because a limit on one of the two is a
/// limit on neither: `Scalar` nests through `Length`, `Add`, `Case` and the
/// rest exactly as `Expr` nests through `And` and `Not`.
#[test]
fn a_computed_value_may_nest_to_the_limit_and_no_further() {
    let table = docs();
    let space = Space::table(&table);

    scalar_from_proto(&space, &nested_length(MAX_EXPRESSION_DEPTH))
        .expect("a computed value at the limit converts");

    let error = scalar_from_proto(&space, &nested_length(MAX_EXPRESSION_DEPTH + 1))
        .expect_err("one deeper must be refused");
    assert_eq!(error.code(), tonic::Code::InvalidArgument, "{error:?}");
    assert!(error.message().contains("nests more than"), "{error:?}");
}

/// A `CASE` condition carries the same budget rather than starting a new one.
///
/// The one place the two recursions meet: a `Scalar::Case` holds an `Expr` in
/// each branch's `when`. They cannot compose into unbounded depth, because an
/// `Expr` has no node that holds a `Scalar` and so the crossing goes only one
/// way — but a `when` that restarted at zero would let a chain of `CASE`s each
/// carry a full-depth condition, and a limit whose real ceiling is the product
/// of two limits is not the limit it says it is.
#[test]
fn a_case_condition_shares_the_expressions_budget() {
    let table = docs();
    let space = Space::table(&table);

    // A `CASE` one level in, whose condition is a chain that would be legal on
    // its own and is one too many from inside.
    let case = pb::Scalar {
        node: Some(pb::scalar::Node::Case(Box::new(pb::Case {
            branches: vec![pb::CaseBranch {
                when: Some(nested_not(MAX_EXPRESSION_DEPTH)),
                then: Some(pb::Scalar {
                    node: Some(pb::scalar::Node::Column(column_ref(0, 0))),
                }),
            }],
            otherwise: Some(Box::new(pb::Scalar {
                node: Some(pb::scalar::Node::Column(column_ref(0, 0))),
            })),
        }))),
    };
    let error = scalar_from_proto(&space, &case)
        .expect_err("the condition starts one deep, so a full-depth chain is one too many");
    assert!(error.message().contains("nests more than"), "{error:?}");

    // And one shallower converts, which is what says the budget is shared
    // rather than simply absent inside a `CASE`.
    let ok = pb::Scalar {
        node: Some(pb::scalar::Node::Case(Box::new(pb::Case {
            branches: vec![pb::CaseBranch {
                when: Some(nested_not(MAX_EXPRESSION_DEPTH - 1)),
                then: Some(pb::Scalar {
                    node: Some(pb::scalar::Node::Column(column_ref(0, 0))),
                }),
            }],
            otherwise: Some(Box::new(pb::Scalar {
                node: Some(pb::scalar::Node::Column(column_ref(0, 0))),
            })),
        }))),
    };
    scalar_from_proto(&space, &ok).expect("one shallower fits");
}
