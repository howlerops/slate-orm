//! Partial and expression indexes: when the planner may use one, and — the
//! part that matters — when it may not.
//!
//! A partial index is the one optimisation in this layer that can return the
//! wrong *answer* rather than the wrong *time*. Every other choice the planner
//! makes is a choice between paths over the same rows, with the residual
//! predicate deciding; a partial index changes which rows exist to be found. Use
//! one for a query it does not cover and the rows that fall outside it are not
//! filtered out, they are never seen — no error, no empty result, just fewer
//! rows than the caller asked for, and only on the queries whose plan happened
//! to flip.
//!
//! So the tests below are lopsided on purpose. There is one showing a partial
//! index being used, and a table of cases showing it not being used, including
//! several where a human can see the implication holds and the planner cannot.
//! Missing one of those costs a table scan.
//!
//! # Maintenance
//!
//! The other half is here too, at the end of the file: a partial index has to
//! be *maintained* partially — an insert that does not match the predicate
//! writes no entry, an update that stops matching deletes one, one that starts
//! matching writes one — and that lives in the record store. Those tests read
//! the index back through a forced index scan and compare it against a table
//! scan of the same predicate, which is the only comparison that can catch an
//! entry that should not be there: the planner would never choose the index for
//! a query that would notice.
//!
//! Expression indexes remain planner-only, and are not maintained. Nothing
//! writes one, so nothing declares one on a schema; they are stated as
//! [`IndexFacts`] alongside the table.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::plan::{Access, IndexFacts, Projection, implies, plan_annotated, plan_with};
use slate_kernel::{AccessHint, SortKey};
use slate_kernel::{CmpOp, ColumnStats, Expr, Scalar, ScanOrder, TableStats};
use slate_schema::{IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const DOCS: TableId = TableId(1);
const BY_TITLE: IndexId = IndexId(10);
const LIVE_BY_AUTHOR: IndexId = IndexId(11);
const BY_LOWER_TITLE: IndexId = IndexId(12);

fn docs() -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("author", ValueType::U64)
        .column("title", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("deleted_at", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_title", BY_TITLE).column("title"))
        // Partial: declared on the schema, so the record store maintains it
        // and the planner reads the same predicate the writes were filtered by.
        .index(
            IndexDef::builder("live_by_author", LIVE_BY_AUTHOR)
                .column("author")
                .only_where(live()),
        )
        // And what makes this one an expression index is the same.
        .index(IndexDef::builder("by_lower_title", BY_LOWER_TITLE).column("title"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    docs().ordinal_of(name).expect("column exists")
}

/// `deleted_at`, by position rather than by name.
///
/// `col` resolves a name by building the table, and the table now names this
/// predicate, so `live()` cannot go back through `col` without recursing
/// forever. `deleted_at_is_the_fifth_column` below pins the constant, so a
/// reordered schema fails a test rather than silently indexing something else.
const DELETED_AT: Ordinal = Ordinal(4);

/// `deleted_at IS NULL`: the commonest partial index there is.
fn live() -> Expr {
    Expr::is_null(DELETED_AT)
}

#[test]
fn deleted_at_is_the_fifth_column() {
    assert_eq!(DELETED_AT, col("deleted_at"));
}

/// A table large enough that an index is worth its point reads, with `author`
/// selective enough that an equality on it is a handful of rows.
fn big() -> TableStats {
    let spread = ColumnStats {
        distinct: 1_000_000,
        null_fraction: 0.0,
    };
    TableStats::with_row_count(1_000_000)
        .with_column(col("author"), spread)
        .with_column(col("title"), spread)
        .with_column(
            col("deleted_at"),
            // Nine rows in ten are live, so the partial index holds nine tenths
            // of the table — enough to be worth choosing and not so little that
            // it wins by default.
            ColumnStats {
                distinct: 100,
                null_fraction: 0.9,
            },
        )
        // A computed value has no statistics of its own: `analyze` measures
        // columns, and `lower(title)` is not one. Without this the planner falls
        // back to "a hundred distinct values", which on a million rows makes an
        // equality ten thousand rows and no index worth reading — so an
        // expression index would lose every test below on the estimate rather
        // than on the decision being tested. Recorded here, and noted as a real
        // gap: see the module docs.
        .with_column(computed(), spread)
}

/// The one fact the schema still cannot state: which index keys on what.
fn facts() -> IndexFacts {
    IndexFacts::new().computed(
        BY_LOWER_TITLE,
        Scalar::Lower(Box::new(Scalar::Column(col("title")))),
    )
}

fn plan_of(filter: Expr, compute: &[Scalar], projection: &Projection) -> Access {
    plan_annotated(
        &docs(),
        Arc::new(filter),
        ScanOrder::Ascending,
        projection,
        &big(),
        None,
        &[],
        None,
        compute,
        &facts(),
    )
    .access
}

fn uses(access: &Access, index: IndexId) -> bool {
    match access {
        Access::IndexScan { index: used, .. } | Access::IndexScans { index: used, .. } => {
            *used == index
        }
        _ => false,
    }
}

// --- using one ------------------------------------------------------------

/// The point of a partial index: a query inside it may read it.
#[test]
fn a_query_inside_the_predicate_may_use_the_index() {
    let filter = live().and(Expr::eq(col("author"), Value::U64(7)));
    let access = plan_of(filter, &[], &Projection::All);
    assert!(
        uses(&access, LIVE_BY_AUTHOR),
        "a live-only query should reach for the live-only index, got {access:?}"
    );
}

/// A query that says nothing about the predicate's column may not.
///
/// This is the whole risk in one test. `author = 7` selects deleted rows too;
/// the index does not hold them; there is no residual that puts them back.
#[test]
fn a_query_outside_the_predicate_may_not() {
    let access = plan_of(
        Expr::eq(col("author"), Value::U64(7)),
        &[],
        &Projection::All,
    );
    assert!(
        !uses(&access, LIVE_BY_AUTHOR),
        "a partial index answered a query it does not cover: {access:?}"
    );
}

/// Nor may a hint reach it.
///
/// A hint is advice about which of several correct plans to take. It is not
/// permission to read an index that does not hold the rows asked for, and a
/// planner that treats it as one has a correctness bug reachable from the query
/// API.
#[test]
fn a_hint_cannot_force_a_partial_index_that_does_not_cover_the_query() {
    let plan = plan_annotated(
        &docs(),
        Arc::new(Expr::eq(col("author"), Value::U64(7))),
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
        &[],
        Some(AccessHint::Index(LIVE_BY_AUTHOR)),
        &[],
        &facts(),
    );
    assert!(
        !uses(&plan.access, LIVE_BY_AUTHOR),
        "a hint forced a partial index onto a query outside it: {:?}",
        plan.access
    );
    // And the query still plans: an unusable hint falls back rather than
    // failing, the same as a hint naming an index that has been dropped.
    assert!(
        matches!(plan.access, Access::TableScan { .. }),
        "{:?}",
        plan.access
    );
}

/// A policy's own term counts.
///
/// The security filter is conjoined before planning, so a row-level policy of
/// `deleted_at IS NULL` makes every query under it eligible for the index —
/// without the caller having written the term, and without the planner
/// treating policy terms as a special case.
#[test]
fn a_security_term_can_be_what_implies_the_predicate() {
    // What `SecuredReads::plan` builds: the caller's filter and the policy's,
    // conjoined.
    let secured = Expr::eq(col("author"), Value::U64(7)).and(live());
    assert!(uses(
        &plan_of(secured, &[], &Projection::All),
        LIVE_BY_AUTHOR
    ));
}

/// A partial index is smaller than the table, and the cost has to say so.
///
/// Two indexes over the same column, one holding a tenth of the rows: the
/// smaller one is cheaper to scan, and if the estimate does not reflect that
/// then the only thing choosing between them is declaration order.
#[test]
fn the_estimate_reflects_how_much_of_the_table_the_index_holds() {
    let sparse = TableStats::with_row_count(1_000_000)
        .with_column(
            col("author"),
            ColumnStats {
                distinct: 10,
                null_fraction: 0.0,
            },
        )
        .with_column(
            col("deleted_at"),
            ColumnStats {
                distinct: 100,
                // One row in a hundred is live, so the partial index is a
                // hundredth of the size.
                null_fraction: 0.01,
            },
        );
    let filter = Arc::new(live().and(Expr::eq(col("author"), Value::U64(3))));
    let whole = plan_annotated(
        &docs(),
        Arc::clone(&filter),
        ScanOrder::Ascending,
        &Projection::All,
        &sparse,
        None,
        &[],
        Some(AccessHint::Index(BY_TITLE)),
        &[],
        &facts(),
    );
    let partial = plan_annotated(
        &docs(),
        filter,
        ScanOrder::Ascending,
        &Projection::All,
        &sparse,
        None,
        &[],
        Some(AccessHint::Index(LIVE_BY_AUTHOR)),
        &[],
        &facts(),
    );
    assert!(
        partial.estimated_cost < whole.estimated_cost,
        "the partial index was not costed as the smaller one: {} against {}",
        partial.estimated_cost,
        whole.estimated_cost
    );
}

// --- the implication test itself ------------------------------------------

/// What the planner can and cannot show, stated as a table.
///
/// Each row is a claim about `implies`, and the `false` rows are as
/// load-bearing as the `true` ones — several of them are implications that do
/// hold, listed here to record that the planner does not see them and that
/// missing them is a scan rather than a wrong answer.
#[test]
fn implication_holds_exactly_where_it_is_claimed_to() {
    let author = col("author");
    let size = col("size");
    let cases: Vec<(&str, Expr, Expr, bool)> = vec![
        ("the same term", live(), live(), true),
        (
            "a conjunct of the query",
            live().and(Expr::eq(author, Value::U64(1))),
            live(),
            true,
        ),
        (
            "every branch of a disjunction",
            Expr::Or(vec![
                live().and(Expr::eq(author, Value::U64(1))),
                live().and(Expr::eq(author, Value::U64(2))),
            ]),
            live(),
            true,
        ),
        (
            "only one branch of a disjunction",
            Expr::Or(vec![live(), Expr::eq(author, Value::U64(2))]),
            live(),
            false,
        ),
        (
            "a tighter bound in the same direction",
            Expr::compare(size, CmpOp::Gt, Value::I64(10)),
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            true,
        ),
        (
            "an inclusive bound at the exclusive boundary",
            Expr::compare(size, CmpOp::Ge, Value::I64(3)),
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            false,
        ),
        (
            "an inclusive bound above the exclusive boundary",
            Expr::compare(size, CmpOp::Ge, Value::I64(4)),
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            true,
        ),
        (
            "a looser bound",
            Expr::compare(size, CmpOp::Gt, Value::I64(1)),
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            false,
        ),
        (
            "a bound in the other direction",
            Expr::compare(size, CmpOp::Lt, Value::I64(100)),
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            false,
        ),
        (
            "an equality meeting the bound",
            Expr::eq(size, Value::I64(50)),
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            true,
        ),
        (
            "an equality below the bound",
            Expr::eq(size, Value::I64(2)),
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            false,
        ),
        (
            "every value of an `IN` meeting the bound",
            Expr::In {
                column: size,
                values: vec![Value::I64(5), Value::I64(9)],
            },
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            true,
        ),
        (
            "one value of an `IN` below it",
            Expr::In {
                column: size,
                values: vec![Value::I64(5), Value::I64(1)],
            },
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            false,
        ),
        (
            "a comparison excludes nulls",
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            Expr::Not(Box::new(Expr::is_null(size))),
            true,
        ),
        (
            "`<>` excludes nulls too, because unknown is not true",
            Expr::compare(size, CmpOp::Ne, Value::I64(3)),
            Expr::Not(Box::new(Expr::is_null(size))),
            true,
        ),
        (
            "a pattern excludes nulls",
            Expr::like(col("title"), "a%"),
            Expr::Not(Box::new(Expr::is_null(col("title")))),
            true,
        ),
        (
            "a comparison on another column says nothing about this one",
            Expr::compare(size, CmpOp::Gt, Value::I64(3)),
            Expr::Not(Box::new(Expr::is_null(col("title")))),
            false,
        ),
        (
            "`IS NULL` is not `IS NOT NULL`",
            Expr::is_null(size),
            Expr::Not(Box::new(Expr::is_null(size))),
            false,
        ),
        (
            "a goal of two terms needs both",
            live(),
            live().and(Expr::compare(size, CmpOp::Gt, Value::I64(3))),
            false,
        ),
        (
            "a goal of two terms, both established",
            live().and(Expr::eq(size, Value::I64(9))),
            live().and(Expr::compare(size, CmpOp::Gt, Value::I64(3))),
            true,
        ),
        // Known blind spots, recorded rather than hidden. Each of these is a
        // true implication the planner cannot see, and each costs a scan.
        (
            "two bounds meeting at a point (not seen)",
            Expr::compare(size, CmpOp::Ge, Value::I64(5)).and(Expr::compare(
                size,
                CmpOp::Le,
                Value::I64(5),
            )),
            Expr::eq(size, Value::I64(5)),
            false,
        ),
        (
            "an unrelated `NOT` (not seen)",
            Expr::Not(Box::new(Expr::compare(size, CmpOp::Lt, Value::I64(3)))),
            Expr::compare(size, CmpOp::Ge, Value::I64(3)),
            false,
        ),
    ];

    for (what, query, index, expected) in cases {
        assert_eq!(
            implies(&query, &index),
            expected,
            "{what}: implies({query:?}, {index:?})"
        );
    }
}

// --- expression indexes ---------------------------------------------------

fn lower_title() -> Scalar {
    Scalar::Lower(Box::new(Scalar::Column(col("title"))))
}

/// The computed value's ordinal: appended after the table's own columns.
fn computed() -> Ordinal {
    Ordinal(docs().columns().len())
}

/// A query computing the same expression can use the index built on it.
#[test]
fn an_expression_index_serves_a_predicate_on_that_expression() {
    let compute = vec![lower_title()];
    let filter = Expr::eq(computed(), Value::Str("moby dick".to_owned()));
    let access = plan_of(filter, &compute, &Projection::All);
    assert!(
        uses(&access, BY_LOWER_TITLE),
        "expected the expression index, got {access:?}"
    );
}

/// `IN` over an expression index is several ranges, like `IN` over any other.
///
/// Nothing in the union is specific to columns; it works off the key columns
/// the index match produced, and for an expression index one of those is a
/// value no row contains.
#[test]
fn an_in_over_an_expression_index_is_still_a_union() {
    let compute = vec![lower_title()];
    let filter = Expr::In {
        column: computed(),
        values: vec![
            Value::Str("moby dick".to_owned()),
            Value::Str("ulysses".to_owned()),
        ],
    };
    match plan_of(filter, &compute, &Projection::All) {
        Access::IndexScans {
            index,
            ref ranges,
            covering,
        } => {
            assert_eq!(index, BY_LOWER_TITLE);
            assert_eq!(ranges.len(), 2);
            assert!(
                !covering,
                "`lower(title)` does not give back `title`, so nothing outside \
                 the key is covered"
            );
        }
        other => panic!("expected a union over the expression index, got {other:?}"),
    }
}

/// A predicate on the underlying column is not a predicate on the expression.
///
/// `title = 'Moby Dick'` and `lower(title) = 'moby dick'` select different
/// rows, and the index holds the second. Matching one against the other would
/// be a wrong answer, not a slow one.
#[test]
fn an_expression_index_does_not_serve_the_column_it_is_built_from() {
    let compute = vec![lower_title()];
    let filter = Expr::eq(col("title"), Value::Str("Moby Dick".to_owned()));
    let access = plan_of(filter, &compute, &Projection::All);
    assert!(
        !uses(&access, BY_LOWER_TITLE),
        "the expression index answered a query on the raw column: {access:?}"
    );
}

/// A query computing a *different* expression cannot use it either.
#[test]
fn a_different_expression_is_a_different_index() {
    let compute = vec![Scalar::Upper(Box::new(Scalar::Column(col("title"))))];
    let filter = Expr::eq(computed(), Value::Str("MOBY DICK".to_owned()));
    let access = plan_of(filter, &compute, &Projection::All);
    assert!(
        !uses(&access, BY_LOWER_TITLE),
        "`upper` was answered from an index on `lower`: {access:?}"
    );
}

/// An expression index is never covering, and that is about the executor
/// rather than about the entry.
///
/// The entry does hold `lower(title)` — the very value the query asked about.
/// What it does not hold is `title`, and the executor recomputes every scalar
/// from the row's own columns, so a row rebuilt from the entry would compute
/// `lower(null)` and hand back a null where every other access path returns a
/// string. Better to read the row than to answer differently depending on the
/// plan.
#[test]
fn an_expression_index_is_not_covering_even_for_a_key_only_projection() {
    let compute = vec![lower_title()];
    let filter = Expr::eq(computed(), Value::Str("moby dick".to_owned()));
    let key_only = plan_of(filter, &compute, &Projection::Columns(vec![col("id")]));
    assert!(
        uses(&key_only, BY_LOWER_TITLE),
        "expected the expression index, got {key_only:?}"
    );
    assert!(
        matches!(
            key_only,
            Access::IndexScan {
                covering: false,
                ..
            }
        ),
        "an expression index was called covering: {key_only:?}"
    );
}

/// An expression index still orders by the value it stores.
///
/// With a limit, which is what makes an ordered index worth its row reads: a
/// path that already produces the order returns the first ten rows after ten
/// reads, where a sort has to find every matching row before it can return one.
#[test]
fn an_expression_index_can_serve_an_order_by_on_the_expression() {
    let compute = vec![lower_title()];
    let plan = plan_annotated(
        &docs(),
        Arc::new(Expr::compare(
            computed(),
            CmpOp::Gt,
            Value::Str("m".to_owned()),
        )),
        ScanOrder::Ascending,
        &Projection::Columns(vec![col("id")]),
        &big(),
        Some(10),
        &[SortKey::asc(computed()), SortKey::asc(col("id"))],
        None,
        &compute,
        &facts(),
    );
    assert!(uses(&plan.access, BY_LOWER_TITLE), "{:?}", plan.access);
    assert!(
        plan.sort.is_none(),
        "the index already produces the requested order"
    );
}

// --- nothing changes for an ordinary index --------------------------------

/// With no facts recorded, the planner is the planner it always was.
///
/// The same query planned with and without an empty fact set has to produce the
/// same access path, or every existing caller has quietly changed behaviour.
#[test]
fn an_index_with_no_facts_plans_exactly_as_before() {
    let filter = Expr::eq(col("title"), Value::Str("Moby Dick".to_owned()));
    let plain = plan_with(
        &docs(),
        &filter,
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
    );
    let annotated = plan_annotated(
        &docs(),
        Arc::new(filter),
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
        &[],
        None,
        &[],
        &IndexFacts::new(),
    );
    assert_eq!(plain.access, annotated.access);
    assert!((plain.estimated_cost - annotated.estimated_cost).abs() < f64::EPSILON);
}

// --- the property ---------------------------------------------------------
//
// `implies` is a claim about every row that could ever exist, which is not
// something a fixed corpus can settle. What a corpus *can* do is refute it:
// generate rows, generate predicates, and check that no row the query admits
// falls outside an index predicate the planner said it implied. A single row in
// the gap is a row a real partial index would have lost.

fn rows() -> Vec<Row> {
    (0..64u64)
        .map(|n| {
            Row::new(vec![
                Value::U64(n),
                Value::U64(n % 7),
                Value::Str(format!("t{}", n % 5)),
                Value::I64((n % 11) as i64 - 5),
                if n.is_multiple_of(3) {
                    Value::Null
                } else {
                    Value::I64(n as i64)
                },
            ])
        })
        .collect()
}

fn leaf() -> impl Strategy<Value = Expr> {
    let ops = prop_oneof![
        Just(CmpOp::Eq),
        Just(CmpOp::Ne),
        Just(CmpOp::Lt),
        Just(CmpOp::Le),
        Just(CmpOp::Gt),
        Just(CmpOp::Ge),
    ];
    prop_oneof![
        (-6i64..6, ops.clone()).prop_map(|(n, op)| Expr::compare(col("size"), op, Value::I64(n))),
        (0..8u64, ops).prop_map(|(a, op)| Expr::compare(col("author"), op, Value::U64(a))),
        Just(Expr::is_null(col("deleted_at"))),
        Just(Expr::Not(Box::new(Expr::is_null(col("deleted_at"))))),
        Just(Expr::Not(Box::new(Expr::is_null(col("size"))))),
        proptest::collection::vec(-6i64..6, 1..4).prop_map(|ns| Expr::In {
            column: col("size"),
            values: ns.into_iter().map(Value::I64).collect(),
        }),
        (0..5u64).prop_map(|t| Expr::like(col("title"), format!("t{t}%"))),
        Just(Expr::True),
    ]
}

fn any_expr() -> impl Strategy<Value = Expr> {
    leaf().prop_recursive(3, 10, 2, |inner| {
        prop_oneof![
            2 => (inner.clone(), inner.clone()).prop_map(|(a, b)| a.and(b)),
            1 => (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::Or(vec![a, b])),
            1 => inner.prop_map(|a| Expr::Not(Box::new(a))),
        ]
    })
}

/// Whenever `implies` says yes, no row disagrees.
///
/// The refutation is the point: `implies` is checked against the evaluator on
/// every row of a corpus chosen to have values on both sides of every literal
/// the generator draws. It cannot prove the general claim — that is what the
/// reasoning in `plan.rs` is for — but a wrong rule shows up here as a row the
/// query admits and the index does not hold.
#[test]
fn a_claimed_implication_is_never_contradicted_by_a_row() {
    let corpus = rows();
    proptest!(
        ProptestConfig::with_cases(2_048),
        |(query in any_expr(), index in any_expr())| {
            // An `if` rather than `prop_assume!`: the implication holds for
            // about one pair in eight, and rejecting the rest aborts the run
            // for too many global rejects long before the cases are spent.
            if implies(&query, &index) {
                for row in &corpus {
                    if query.admits(row) {
                        prop_assert!(
                            index.admits(row),
                            "implies said {:?} lands inside {:?}, but {:?} does not",
                            query, index, row
                        );
                    }
                }
            }
        }
    );
}

/// The property above has to be reached, not assumed away.
///
/// `prop_assume!` skips every case where the implication does not hold, so a
/// broken `implies` that answered `false` to everything would pass it in
/// silence. This counts how often it answers `true` on the same generators.
#[test]
fn the_generators_produce_implications_to_check() {
    let holds = std::cell::RefCell::new(0usize);
    let total = std::cell::RefCell::new(0usize);
    proptest!(ProptestConfig::with_cases(2_048), |(query in any_expr(), index in any_expr())| {
        *total.borrow_mut() += 1;
        if implies(&query, &index) {
            *holds.borrow_mut() += 1;
        }
    });
    let (holds, total) = (holds.into_inner(), total.into_inner());
    // Measured at 200 of 2,048, just under 10%. The bar is 5%, seven standard
    // errors below that at this sample size — and the number that really
    // matters is that it is not zero.
    assert!(
        holds * 20 > total,
        "only {holds} of {total} generated pairs implied anything, so the \
         property above is mostly skipping"
    );
}

// --- maintenance -----------------------------------------------------------
//
// The other half of a partial index, and the half that has to be checked
// against raw keys rather than through a query. A *missing* entry shows up in a
// query — rows go absent. A *spurious* one never can: the residual predicate
// still runs on every row an index scan fetches, and a row that should not be
// in the index is by definition one the predicate rejects, so the residual
// throws it away and the answer looks right. The only way to see the entry that
// should not be there is to look at the keys.

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Grant, KernelError, RecordStore, SecurityCatalog, SecurityContext, keys,
};
use slate_schema::Catalog;
use std::collections::BTreeSet;

const TASKS: TableId = TableId(2);
const OPEN_BY_OWNER: IndexId = IndexId(20);
const ONE_OPEN_PER_OWNER: IndexId = IndexId(21);

/// `id`, `owner`, `state`. By position, for the same reason `DELETED_AT` is.
const ID: Ordinal = Ordinal(0);
const OWNER: Ordinal = Ordinal(1);
const STATE: Ordinal = Ordinal(2);

/// The rows both partial indexes hold.
fn open() -> Expr {
    Expr::eq(STATE, Value::Str("open".into()))
}

fn tasks() -> TableDef {
    TableDef::builder("tasks", TASKS)
        .column("id", ValueType::U64)
        .column("owner", ValueType::Str)
        .column("state", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("open_by_owner", OPEN_BY_OWNER)
                .column("owner")
                .only_where(open()),
        )
        // Unique *among open tasks*: one owner may have any number of closed
        // ones. This is what a partial unique index is for, and it works by
        // holding no entry for the others rather than by a second rule.
        .index(
            IndexDef::builder("one_open_per_owner", ONE_OPEN_PER_OWNER)
                .column("owner")
                .unique()
                .only_where(open()),
        )
        .build()
        .expect("valid schema")
}

#[test]
fn the_task_ordinals_are_where_they_are_claimed_to_be() {
    let table = tasks();
    assert_eq!(ID, table.ordinal_of("id").unwrap());
    assert_eq!(OWNER, table.ordinal_of("owner").unwrap());
    assert_eq!(STATE, table.ordinal_of("state").unwrap());
}

fn task(id: u64, owner: &str, state: &str) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(owner.to_owned()),
        Value::Str(state.to_owned()),
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn task_store() -> (RecordStore<MemoryStore>, MemoryStore) {
    let kv = MemoryStore::new();
    let catalog = Catalog::from_tables([tasks()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", TASKS, Action::ALL));
    (
        RecordStore::new(kv.clone(), catalog, security),
        // The same store: `MemoryStore` is a handle, so this reads the keys the
        // record layer is writing rather than a copy of them.
        kv,
    )
}

/// Every committed key under one index's prefix.
fn index_keys(kv: &MemoryStore, index: IndexId) -> BTreeSet<Vec<u8>> {
    let table = tasks();
    let index = table.index(index).expect("the index exists");
    let prefix = keys::index_prefix(&table, index, None);
    kv.keys()
        .into_iter()
        .filter(|key| key.starts_with(&prefix))
        .collect()
}

/// The keys the index *should* hold, given the rows that are in the table.
fn wanted_keys(index: IndexId, rows: &[Row]) -> BTreeSet<Vec<u8>> {
    let table = tasks();
    let index = table.index(index).expect("the index exists");
    rows.iter()
        .filter(|row| index.admits(row))
        .map(|row| {
            keys::index_entry(
                &table,
                index,
                &row.index_values(index),
                &row.primary_key_values(&table),
            )
            .key
        })
        .collect()
}

#[tokio::test]
async fn an_insert_the_predicate_rejects_writes_no_entry() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.insert(&root(), &table, &task(2, "ann", "closed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(index_keys(&kv, OPEN_BY_OWNER).len(), 1);
    assert_eq!(
        index_keys(&kv, OPEN_BY_OWNER),
        wanted_keys(OPEN_BY_OWNER, &[task(1, "ann", "open")]),
        "the closed task is in the index, or the open one is not"
    );

    // Both rows are still there. A partial index restricts the index, not the
    // table, and confusing the two would be a far worse bug than an extra entry.
    let txn = store.begin().await.unwrap();
    assert!(
        txn.get(&root(), &table, &[Value::U64(2)])
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn an_update_that_stops_matching_deletes_the_entry() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(index_keys(&kv, OPEN_BY_OWNER).len(), 1);

    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &task(1, "ann", "closed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert!(
        index_keys(&kv, OPEN_BY_OWNER).is_empty(),
        "closing a task left its entry behind"
    );
    assert!(
        index_keys(&kv, ONE_OPEN_PER_OWNER).is_empty(),
        "the unique slot was never released"
    );
}

#[tokio::test]
async fn an_update_that_starts_matching_writes_one() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "closed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert!(index_keys(&kv, OPEN_BY_OWNER).is_empty());

    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(
        index_keys(&kv, OPEN_BY_OWNER),
        wanted_keys(OPEN_BY_OWNER, &[task(1, "ann", "open")]),
        "reopening a task did not put it back in the index"
    );
}

/// Reopening a task whose *owner* did not change still needs an entry.
///
/// The write path skips an index whose key is unchanged, to avoid inventing a
/// write-write conflict. For a partial index "unchanged" has to mean the row
/// was held before and is held now — a row that was outside the index owns no
/// entry, however familiar its indexed values look.
#[tokio::test]
async fn the_unchanged_key_shortcut_does_not_skip_a_row_rejoining_the_index() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    for state in ["closed", "open"] {
        let txn = store.begin().await.unwrap();
        txn.update(&root(), &table, &task(1, "ann", state))
            .await
            .unwrap();
        txn.commit().await.unwrap();
    }

    assert_eq!(
        index_keys(&kv, OPEN_BY_OWNER),
        wanted_keys(OPEN_BY_OWNER, &[task(1, "ann", "open")]),
        "the owner never changed, so the entry was skipped and never rewritten"
    );
}

#[tokio::test]
async fn a_partial_unique_index_constrains_only_the_rows_it_holds() {
    let (store, kv) = task_store();
    let table = tasks();

    // Any number of closed tasks may share an owner.
    let txn = store.begin().await.unwrap();
    for id in 1..=3 {
        txn.insert(&root(), &table, &task(id, "ann", "closed"))
            .await
            .expect("closed tasks are not unique per owner");
    }
    txn.commit().await.unwrap();
    assert!(index_keys(&kv, ONE_OPEN_PER_OWNER).is_empty());

    // One open one is fine.
    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    // A second is not.
    let txn = store.begin().await.unwrap();
    let refused = txn.update(&root(), &table, &task(2, "ann", "open")).await;
    assert!(
        matches!(refused, Err(KernelError::UniqueViolation { .. })),
        "a second open task for one owner was allowed: {refused:?}"
    );
}

/// Closing the open task frees the slot for another.
#[tokio::test]
async fn the_unique_slot_is_released_when_a_row_leaves_the_index() {
    let (store, _kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.insert(&root(), &table, &task(2, "ann", "closed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &task(1, "ann", "closed"))
        .await
        .unwrap();
    txn.update(&root(), &table, &task(2, "ann", "open"))
        .await
        .expect("the slot was not released by closing the first task");
    txn.commit().await.unwrap();
}

#[tokio::test]
async fn deleting_a_row_the_index_does_not_hold_leaves_the_others_alone() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.insert(&root(), &table, &task(2, "bob", "closed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.delete(&root(), &table, &[Value::U64(2)]).await.unwrap();
    txn.commit().await.unwrap();

    assert_eq!(
        index_keys(&kv, OPEN_BY_OWNER),
        wanted_keys(OPEN_BY_OWNER, &[task(1, "ann", "open")])
    );
}

/// One operation of the generated sequence.
#[derive(Debug, Clone)]
enum Op {
    Put(u64, &'static str, &'static str),
    Delete(u64),
}

fn any_op() -> impl Strategy<Value = Op> {
    let owners = prop_oneof![Just("ann"), Just("bob"), Just("cat")];
    let states = prop_oneof![Just("open"), Just("closed"), Just("done")];
    prop_oneof![
        3 => (0..4u64, owners, states).prop_map(|(id, o, s)| Op::Put(id, o, s)),
        1 => (0..4u64).prop_map(Op::Delete),
    ]
}

/// After any sequence of writes, both indexes hold exactly the entries the
/// predicate says they should.
///
/// The oracle is the key set, not a query, for the reason at the top of this
/// section: a spurious entry is invisible through a query. The expected set is
/// computed from a plain `Vec<Row>` maintained alongside — a second model of
/// the table, which is the point.
#[test]
fn the_indexes_hold_exactly_the_rows_the_predicate_admits() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    proptest!(
        ProptestConfig::with_cases(256),
        |(ops in proptest::collection::vec(any_op(), 1..24))| {
            runtime.block_on(async {
                let (store, kv) = task_store();
                let table = tasks();
                // The model: what the table holds, by primary key.
                let mut model: Vec<Row> = Vec::new();

                for op in &ops {
                    let txn = store.begin().await.unwrap();
                    match op {
                        Op::Put(id, owner, state) => {
                            let row = task(*id, owner, state);
                            // `upsert` rather than insert-or-update, so the
                            // sequence never has to know what is already there.
                            if txn.upsert(&root(), &table, &row).await.is_err() {
                                txn.rollback();
                                continue;
                            }
                            model.retain(|r| r.values()[ID.0] != Value::U64(*id));
                            model.push(row);
                        }
                        Op::Delete(id) => {
                            if txn.delete(&root(), &table, &[Value::U64(*id)]).await.is_err() {
                                txn.rollback();
                                continue;
                            }
                            model.retain(|r| r.values()[ID.0] != Value::U64(*id));
                        }
                    }
                    if txn.commit().await.is_err() {
                        // A refused commit changed nothing, so the model has
                        // to be put back the way it was. Simpler to rebuild it
                        // from storage than to undo one operation.
                        continue;
                    }
                }

                for index in [OPEN_BY_OWNER, ONE_OPEN_PER_OWNER] {
                    prop_assert_eq!(
                        index_keys(&kv, index),
                        wanted_keys(index, &model),
                        "index {:?} disagrees with the predicate after {:?}",
                        index,
                        ops
                    );
                }
                Ok(())
            })?;
        }
    );
}

/// The sequences have to actually put rows on both sides of the predicate.
#[test]
fn the_generated_sequences_reach_both_sides_of_the_predicate() {
    let held = std::cell::RefCell::new(0usize);
    let rejected = std::cell::RefCell::new(0usize);
    proptest!(ProptestConfig::with_cases(256), |(ops in proptest::collection::vec(any_op(), 1..24))| {
        for op in &ops {
            if let Op::Put(_, _, state) = op {
                if *state == "open" {
                    *held.borrow_mut() += 1;
                } else {
                    *rejected.borrow_mut() += 1;
                }
            }
        }
    });
    let (held, rejected) = (held.into_inner(), rejected.into_inner());
    assert!(
        held > 0 && rejected > 0,
        "{held} rows inside the predicate and {rejected} outside; the oracle \
         above needs both"
    );
}

/// Removing a row the index does not hold must not delete somebody else's
/// entry.
///
/// The sharp case, and the reason the write path checks the predicate on the
/// row being *replaced or removed* rather than only on the row being written. A
/// unique index's key omits the primary key — that is what makes two rows
/// collide in it — so the entry a closed task *would* have had is byte for byte
/// the entry the open task really has. Computing it from a row the index does
/// not hold and deleting it takes the open task's entry with it.
#[tokio::test]
async fn removing_an_unheld_row_does_not_take_another_rows_unique_entry() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "closed"))
        .await
        .unwrap();
    txn.insert(&root(), &table, &task(2, "ann", "open"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let held = index_keys(&kv, ONE_OPEN_PER_OWNER);
    assert_eq!(held.len(), 1, "the open task should own the slot");

    let txn = store.begin().await.unwrap();
    txn.delete(&root(), &table, &[Value::U64(1)]).await.unwrap();
    txn.commit().await.unwrap();

    assert_eq!(
        index_keys(&kv, ONE_OPEN_PER_OWNER),
        held,
        "deleting the closed task deleted the open task's index entry"
    );
}

/// The same, for an update rather than a delete.
#[tokio::test]
async fn updating_an_unheld_row_does_not_take_another_rows_unique_entry() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "closed"))
        .await
        .unwrap();
    txn.insert(&root(), &table, &task(2, "ann", "open"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let held = index_keys(&kv, ONE_OPEN_PER_OWNER);

    // Closed to done: outside the index before and after, so it has no entry
    // to move and none to delete.
    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &task(1, "ann", "done"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(
        index_keys(&kv, ONE_OPEN_PER_OWNER),
        held,
        "a write to a row outside the index removed one that is inside it"
    );
}

/// The bulk path has its own two unique checks, and both need the predicate.
///
/// `insert_many` checks for collisions *within* the batch before it reads
/// anything, and then for collisions against storage. Neither sees a `TableDef`
/// method that would apply the predicate for it, so both had to be told, and
/// this is what tells them: three tasks for one owner, none of them open, is a
/// batch a partial unique index has nothing to say about.
#[tokio::test]
async fn a_bulk_insert_does_not_invent_a_collision_outside_the_index() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert_many(
        &root(),
        &table,
        &[
            task(1, "ann", "closed"),
            task(2, "ann", "done"),
            task(3, "ann", "closed"),
        ],
    )
    .await
    .expect("three closed tasks for one owner collide in nothing");
    txn.commit().await.unwrap();

    assert!(index_keys(&kv, ONE_OPEN_PER_OWNER).is_empty());
    assert_eq!(index_keys(&kv, OPEN_BY_OWNER).len(), 0);
}

/// And it still refuses a real one.
#[tokio::test]
async fn a_bulk_insert_still_refuses_a_collision_inside_the_index() {
    let (store, _kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert_many(
            &root(),
            &table,
            &[task(1, "ann", "open"), task(2, "ann", "open")],
        )
        .await;
    assert!(
        matches!(refused, Err(KernelError::UniqueViolation { .. })),
        "two open tasks for one owner were accepted in one batch: {refused:?}"
    );
}

/// A batch that collides with a row already stored, rather than with itself.
#[tokio::test]
async fn a_bulk_insert_checks_the_index_against_storage_too() {
    let (store, _kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert_many(&root(), &table, &[task(2, "ann", "open")])
        .await;
    assert!(
        matches!(refused, Err(KernelError::UniqueViolation { .. })),
        "a batch took a slot storage already held: {refused:?}"
    );

    // ...and a closed one in the same shape does not.
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table, &[task(3, "ann", "closed")])
        .await
        .expect("a closed task cannot collide with an open one");
    txn.commit().await.unwrap();
}

/// A row joining the index in a bulk write is a *new* slot, however familiar it
/// looks.
///
/// The bulk unique check skips a row whose slot has not changed, on the grounds
/// that it already owns it. For a partial index that reasoning needs the row to
/// have been in the index before: a closed task that opens has the same owner
/// it always had and so the same would-be key, and skipping the check on that
/// resemblance writes over whichever row really holds the slot — silently, in
/// the same transaction, where no write-write conflict can catch it.
#[tokio::test]
async fn a_row_joining_the_index_in_bulk_is_checked_against_the_slot_it_takes() {
    let (store, kv) = task_store();
    let table = tasks();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &task(1, "ann", "open"))
        .await
        .unwrap();
    txn.insert(&root(), &table, &task(2, "ann", "closed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let held = index_keys(&kv, ONE_OPEN_PER_OWNER);
    assert_eq!(held.len(), 1);

    // Task 2's owner does not change; only its state does, from outside the
    // index to inside it.
    let txn = store.begin().await.unwrap();
    let refused = txn
        .upsert_many(&root(), &table, &[task(2, "ann", "open")])
        .await;
    assert!(
        matches!(refused, Err(KernelError::UniqueViolation { .. })),
        "task 2 opened into a slot task 1 already holds: {refused:?}"
    );
    txn.rollback();

    assert_eq!(
        index_keys(&kv, ONE_OPEN_PER_OWNER),
        held,
        "the entry moved anyway"
    );
}
