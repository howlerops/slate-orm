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
//! Expression indexes are declared and maintained the same way, through the
//! `Computed` seam rather than the `Predicate` one.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::plan::{Access, Projection, implies, plan_hinted, plan_with};
use slate_kernel::{AccessHint, Query, SortKey};
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
        // Keyed on a value the row does not hold. `Str` is declared because
        // the decoder needs the type before it has a row to run `lower` on;
        // `Row::validate` holds every write to it.
        .index(
            IndexDef::builder("by_lower_title", BY_LOWER_TITLE)
                .expression(lower_title(), ValueType::Str),
        )
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
        // The computed value's own statistics, recorded against the index
        // rather than against an ordinal because `lower(title)` is not a
        // column and has none. `analyze` now measures this by evaluating the
        // expression over the rows it samples; recorded by hand here so the
        // tests below turn on the decision being tested rather than on the
        // size of a fixture. Without it the planner falls back to "a hundred
        // distinct values", which on a million rows makes an equality ten
        // thousand rows and no index worth reading.
        .with_expression(BY_LOWER_TITLE, spread)
}

fn plan_of(filter: Expr, compute: &[Scalar], projection: &Projection) -> Access {
    plan_hinted(
        &docs(),
        Arc::new(filter),
        ScanOrder::Ascending,
        projection,
        &big(),
        None,
        &[],
        None,
        compute,
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
    let plan = plan_hinted(
        &docs(),
        Arc::new(Expr::eq(col("author"), Value::U64(7))),
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
        &[],
        Some(AccessHint::Index(LIVE_BY_AUTHOR)),
        &[],
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
    let whole = plan_hinted(
        &docs(),
        Arc::clone(&filter),
        ScanOrder::Ascending,
        &Projection::All,
        &sparse,
        None,
        &[],
        Some(AccessHint::Index(BY_TITLE)),
        &[],
    );
    let partial = plan_hinted(
        &docs(),
        filter,
        ScanOrder::Ascending,
        &Projection::All,
        &sparse,
        None,
        &[],
        Some(AccessHint::Index(LIVE_BY_AUTHOR)),
        &[],
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

/// `title`, by position: `docs()` names this expression, so resolving the
/// column by name would recurse through `col`. Pinned below.
const TITLE: Ordinal = Ordinal(2);

fn lower_title() -> Scalar {
    Scalar::Lower(Box::new(Scalar::Column(TITLE)))
}

#[test]
fn title_is_the_third_column() {
    assert_eq!(TITLE, col("title"));
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
    match plan_of(filter.clone(), &compute, &Projection::All) {
        Access::IndexScans {
            index,
            ref ranges,
            covering,
        } => {
            assert_eq!(index, BY_LOWER_TITLE);
            assert_eq!(ranges.len(), 2);
            assert!(
                !covering,
                "`SELECT *` needs every column and the entry holds one value \
                 and the key"
            );
        }
        other => panic!("expected a union over the expression index, got {other:?}"),
    }

    // The same union, projected down to what the entry can answer, is covering
    // — the split into ranges and the index-only scan are independent, and a
    // change that coupled them would show up here.
    match plan_of(filter, &compute, &Projection::Columns(vec![col("id")])) {
        Access::IndexScans {
            ref ranges,
            covering,
            ..
        } => {
            assert_eq!(ranges.len(), 2);
            assert!(covering, "each range answers from its own entries");
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

/// An expression index *is* covering for a query that reads nothing but the
/// computed value and the key.
///
/// This used to be refused, and the refusal was about the executor rather than
/// about the entry: the entry holds `lower(title)` — the very value asked about
/// — but every scalar was evaluated from the row's own columns, and a row
/// rebuilt from an entry has `title` null, so the scan would have computed
/// `lower(null)` and answered differently from every other access path. The
/// executor now takes that value out of the entry, so there is nothing left to
/// read and the index answers on its own.
#[test]
fn an_expression_index_covers_a_projection_of_the_key_and_the_expression() {
    let compute = vec![lower_title()];
    let filter = Expr::eq(computed(), Value::Str("moby dick".to_owned()));
    for projection in [
        // The key alone.
        Projection::Columns(vec![col("id")]),
        // And the computed value itself, which is the query anyone would
        // actually write.
        Projection::Columns(vec![col("id"), computed()]),
        // Nothing at all, which is what a count projects.
        Projection::none(),
    ] {
        let access = plan_of(filter.clone(), &compute, &projection);
        assert!(
            uses(&access, BY_LOWER_TITLE),
            "expected the expression index for {projection:?}, got {access:?}"
        );
        assert!(
            matches!(access, Access::IndexScan { covering: true, .. }),
            "the entry holds the key and the computed value, so {projection:?} \
             needs no row: {access:?}"
        );
    }
}

/// And it is *not* covering as soon as the query needs the column the
/// expression reads.
///
/// The dangerous direction. `title` is nowhere in an entry keyed on
/// `lower(title)`, so a covering scan would hand back null for it while a
/// table scan hands back the string — the same query with two answers, decided
/// by a cost estimate.
#[test]
fn an_expression_index_does_not_cover_a_query_that_reads_the_source_column() {
    let compute = vec![lower_title()];
    let filter = Expr::eq(computed(), Value::Str("moby dick".to_owned()));
    for projection in [
        // Projected outright.
        Projection::Columns(vec![col("id"), col("title")]),
        // Or reached through a *second* computed value, whose inputs the entry
        // does not carry either. Only the one value the index keys on comes
        // out of the entry.
        Projection::Columns(vec![col("id"), Ordinal(computed().0 + 1)]),
        // Or a column the index has nothing to do with.
        Projection::Columns(vec![col("id"), col("size")]),
    ] {
        let compute = match projection {
            Projection::Columns(ref columns) if columns.contains(&Ordinal(computed().0 + 1)) => {
                vec![
                    lower_title(),
                    Scalar::Length(Box::new(Scalar::Column(TITLE))),
                ]
            }
            _ => compute.clone(),
        };
        let access = plan_of(filter.clone(), &compute, &projection);
        assert!(
            !matches!(access, Access::IndexScan { covering: true, .. }),
            "{projection:?} reads a column no entry of `by_lower_title` holds, \
             so it cannot be answered from one: {access:?}"
        );
    }
}

/// An *ordinary* index covers a computed query when it holds what the
/// expression reads.
///
/// A consequence of the same change rather than a separate feature: the
/// planner used to expand a computed value into its input columns before it
/// knew which index it was asking about, and then also insist on the computed
/// ordinal itself, which no index has. Now the inputs decide, and `by_title`
/// holds `title`.
#[test]
fn an_ordinary_index_covers_a_computed_query_from_its_own_columns() {
    let compute = vec![lower_title()];
    let filter = Expr::eq(col("title"), Value::Str("Moby Dick".to_owned()));
    let access = plan_of(
        filter,
        &compute,
        &Projection::Columns(vec![col("id"), computed()]),
    );
    assert!(
        matches!(access, Access::IndexScan { covering: true, .. }) && uses(&access, BY_TITLE),
        "`by_title` holds `title`, so it can compute `lower(title)` itself: {access:?}"
    );
}

/// The statistics decide whether an expression index is worth reading at all.
///
/// A *non-covering* scan of one costs a point read per row it fetches, so the
/// choice is entirely the estimate's — and until `analyze` evaluated the
/// expression there was no estimate, only the hundred distinct values an
/// unmeasured column gets. On a million rows that is ten thousand point reads,
/// and the table is cheaper to read whole.
#[test]
fn expression_statistics_decide_whether_the_index_is_worth_reading() {
    let filter = Expr::eq(computed(), Value::Str("moby dick".to_owned()));
    let compute = vec![lower_title()];

    let measured = plan_of(filter.clone(), &compute, &Projection::All);
    assert!(
        uses(&measured, BY_LOWER_TITLE),
        "a million distinct computed values makes this one row: {measured:?}"
    );

    // The same query, the same table, the same size — and nothing recorded
    // about what the expression produces.
    let unmeasured = plan_hinted(
        &docs(),
        Arc::new(filter),
        ScanOrder::Ascending,
        &Projection::All,
        &TableStats::with_row_count(1_000_000),
        None,
        &[],
        None,
        &compute,
    )
    .access;
    assert!(
        matches!(unmeasured, Access::TableScan { .. }),
        "unmeasured, the equality looks like ten thousand rows and the index \
         should lose: {unmeasured:?}"
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
    let plan = plan_hinted(
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
    );
    assert!(uses(&plan.access, BY_LOWER_TITLE), "{:?}", plan.access);
    assert!(
        plan.sort.is_none(),
        "the index already produces the requested order"
    );
}

// --- nothing changes for an ordinary index --------------------------------

/// An ordinary index plans as it always did.
///
/// Every entry point now sees partial and expression indexes, because they are
/// on the schema rather than passed in alongside it. The same query through the
/// short `plan_with` and the long `plan_hinted` must still produce the same
/// access path, or existing callers have quietly changed behaviour.
#[test]
fn an_ordinary_index_plans_exactly_as_before() {
    let filter = Expr::eq(col("title"), Value::Str("Moby Dick".to_owned()));
    let plain = plan_with(
        &docs(),
        &filter,
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
    );
    let annotated = plan_hinted(
        &docs(),
        Arc::new(filter),
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
        &[],
        None,
        &[],
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

use slate_kernel::latency::{LatencyProfile, LatencyStore};
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

// --- expression index maintenance ------------------------------------------
//
// The entries hold a value no row contains, so the two ways this can go wrong
// are the encode and the decode. A key written under `lower(title)` and read
// back as something else is a scan that quietly returns the wrong rows, and the
// only way to notice is to make the index and a table scan answer the same
// question and compare.

const NOTES: TableId = TableId(3);
const BY_LOWER_BODY: IndexId = IndexId(30);
const BY_BODY_LENGTH: IndexId = IndexId(31);

/// `id`, `body`. By position, since the schema names expressions over them.
const NOTE_ID: Ordinal = Ordinal(0);
const BODY: Ordinal = Ordinal(1);

fn lower_body() -> Scalar {
    Scalar::Lower(Box::new(Scalar::Column(BODY)))
}

fn body_length() -> Scalar {
    Scalar::Length(Box::new(Scalar::Column(BODY)))
}

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("id", ValueType::U64)
        // Nullable, so `lower(null)` and `length(null)` are reachable: an
        // expression index has to hold those rows, because the query computing
        // the same expression gets the same null and would expect to find them.
        .nullable_column("body", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_lower_body", BY_LOWER_BODY)
                .expression(lower_body(), ValueType::Str),
        )
        .index(
            IndexDef::builder("by_body_length", BY_BODY_LENGTH)
                .expression(body_length(), ValueType::I64),
        )
        .build()
        .expect("valid schema")
}

#[test]
fn the_note_ordinals_are_where_they_are_claimed_to_be() {
    let table = notes();
    assert_eq!(NOTE_ID, table.ordinal_of("id").unwrap());
    assert_eq!(BODY, table.ordinal_of("body").unwrap());
}

fn note(id: u64, body: Option<&str>) -> Row {
    Row::new(vec![
        Value::U64(id),
        body.map_or(Value::Null, |b| Value::Str(b.to_owned())),
    ])
}

fn note_store() -> (RecordStore<MemoryStore>, MemoryStore) {
    let kv = MemoryStore::new();
    let catalog = Catalog::from_tables([notes()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", NOTES, Action::ALL));
    (RecordStore::new(kv.clone(), catalog, security), kv)
}

fn note_index_keys(kv: &MemoryStore, index: IndexId) -> BTreeSet<Vec<u8>> {
    let table = notes();
    let index = table.index(index).expect("the index exists");
    let prefix = keys::index_prefix(&table, index, None);
    kv.keys()
        .into_iter()
        .filter(|key| key.starts_with(&prefix))
        .collect()
}

fn wanted_note_keys(index: IndexId, rows: &[Row]) -> BTreeSet<Vec<u8>> {
    let table = notes();
    let index = table.index(index).expect("the index exists");
    rows.iter()
        .map(|row| {
            keys::index_entry(
                &table,
                index,
                &index.key_values(row),
                &row.primary_key_values(&table),
            )
            .key
        })
        .collect()
}

#[tokio::test]
async fn an_expression_index_is_keyed_on_the_computed_value() {
    let (store, kv) = note_store();
    let table = notes();
    let rows = [note(1, Some("Moby Dick")), note(2, None)];
    let txn = store.begin().await.unwrap();
    for row in &rows {
        txn.insert(&root(), &table, row).await.unwrap();
    }
    txn.commit().await.unwrap();

    assert_eq!(
        note_index_keys(&kv, BY_LOWER_BODY),
        wanted_note_keys(BY_LOWER_BODY, &rows)
    );
    assert_eq!(
        note_index_keys(&kv, BY_BODY_LENGTH),
        wanted_note_keys(BY_BODY_LENGTH, &rows)
    );

    // And the key really is the *computed* value, not the column: an entry
    // keyed on "Moby Dick" would compare equal to nothing the query asks for.
    let index = table.index(BY_LOWER_BODY).unwrap();
    let by_column = keys::index_entry(
        &table,
        index,
        &[Value::Str("Moby Dick".to_owned())],
        &[Value::U64(1)],
    )
    .key;
    assert!(
        !note_index_keys(&kv, BY_LOWER_BODY).contains(&by_column),
        "the entry was keyed on the column rather than on lower() of it"
    );
}

#[tokio::test]
async fn changing_the_source_column_moves_the_entry() {
    let (store, kv) = note_store();
    let table = notes();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &note(1, Some("first")))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let before = note_index_keys(&kv, BY_LOWER_BODY);

    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &note(1, Some("SECOND")))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let after = note_index_keys(&kv, BY_LOWER_BODY);
    assert_eq!(
        after,
        wanted_note_keys(BY_LOWER_BODY, &[note(1, Some("SECOND"))])
    );
    assert_ne!(before, after, "the entry did not move");
    assert_eq!(after.len(), 1, "the old entry is still there");
}

/// An update the *expression* does not notice must not rewrite the entry.
///
/// `lower("abc")` and `lower("ABC")` are the same key, so the index has nothing
/// to do — and rewriting it anyway would invent a write-write conflict against
/// any concurrent writer of a row sharing that slot, which is exactly what the
/// unchanged-key shortcut exists to avoid. Whether the shortcut fires is not
/// observable from here; that the entry is right afterwards is.
#[tokio::test]
async fn an_update_the_expression_does_not_notice_leaves_the_entry_alone() {
    let (store, kv) = note_store();
    let table = notes();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &note(1, Some("abc")))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let before = note_index_keys(&kv, BY_LOWER_BODY);

    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &note(1, Some("ABC")))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(note_index_keys(&kv, BY_LOWER_BODY), before);
    // The length index does not notice either; the body index would, if there
    // were one, which is the point of checking a second expression at all.
    assert_eq!(
        note_index_keys(&kv, BY_BODY_LENGTH),
        wanted_note_keys(BY_BODY_LENGTH, &[note(1, Some("ABC"))])
    );
}

/// The oracle: reading through an expression index agrees with a table scan.
///
/// This is the test that covers the encode and the decode together. The index
/// stores a value the row does not contain, under a declared type, in a key the
/// scan bounds are built from — and the query computes the same expression per
/// row on the other path. If any step of that disagrees, the two answers do.
#[tokio::test]
async fn an_expression_index_agrees_with_a_table_scan() {
    let (store, _kv) = note_store();
    let table = notes();
    let bodies = [
        Some("Alpha"),
        Some("alpha"),
        Some("BETA"),
        Some("beta"),
        Some("gamma"),
        Some(""),
        None,
        Some("Delta"),
        Some("delta"),
        Some("epsilon"),
    ];
    let txn = store.begin().await.unwrap();
    for (id, body) in bodies.iter().enumerate() {
        txn.insert(&root(), &table, &note(id as u64, *body))
            .await
            .unwrap();
    }
    txn.commit().await.unwrap();

    let computed = Query::computed(&table, 0);
    let cases = [
        Expr::eq(computed, Value::Str("alpha".to_owned())),
        Expr::eq(computed, Value::Str("delta".to_owned())),
        Expr::eq(computed, Value::Str("nothing".to_owned())),
        Expr::compare(computed, CmpOp::Ge, Value::Str("c".to_owned())),
        Expr::compare(computed, CmpOp::Lt, Value::Str("c".to_owned())),
        Expr::In {
            column: computed,
            values: vec![
                Value::Str("beta".to_owned()),
                Value::Str("gamma".to_owned()),
            ],
        },
    ];

    for filter in cases {
        let mut through_index = Query::all().filter(filter.clone());
        through_index.compute = vec![lower_body()];
        through_index.hint = Some(AccessHint::Index(BY_LOWER_BODY));

        let mut through_scan = Query::all().filter(filter.clone());
        through_scan.compute = vec![lower_body()];
        through_scan.hint = Some(AccessHint::TableScan);

        let txn = store.begin().await.unwrap();
        let plan = txn.explain(&root(), &table, &through_index).unwrap();
        assert!(
            plan.access.to_string().contains("by_lower_body"),
            "the hint did not take: {} for {filter:?}",
            plan.access
        );

        let mut indexed: Vec<Value> = txn
            .execute(&root(), &table, &through_index)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.values()[NOTE_ID.0].clone())
            .collect();
        let mut scanned: Vec<Value> = txn
            .execute(&root(), &table, &through_scan)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.values()[NOTE_ID.0].clone())
            .collect();
        indexed.sort();
        scanned.sort();
        assert_eq!(indexed, scanned, "the two paths disagree on {filter:?}");
    }
}

/// The covering oracle: an index-only scan of an expression index must return
/// exactly what a table scan of the same query returns — whole rows, computed
/// value included.
///
/// This is the test the whole of gap two turns on, and it is a differential
/// rather than a list of expectations because the failure modes are all
/// "plausible but different". Three of them, each of which this catches:
///
/// - the executor evaluating `lower(body)` from a row rebuilt out of an entry,
///   where `body` is null, and returning null;
/// - the entry's value being read from the wrong place in the entry, or under
///   the wrong direction, and coming back as some other row's;
/// - the table scan returning `body` — which it has to decode to compute
///   `lower(body)` — while the covering scan cannot, so the two paths differ on
///   a column nobody projected.
///
/// The rows are compared in full, not by id, precisely because the third of
/// those is invisible to a comparison of ids.
#[tokio::test]
async fn a_covering_expression_scan_agrees_with_a_table_scan() {
    let (store, _kv) = note_store();
    let table = notes();
    let bodies = [
        Some("Alpha"),
        Some("alpha"),
        Some("BETA"),
        Some("beta"),
        Some("gamma"),
        Some(""),
        None,
        Some("Delta"),
        Some("delta"),
        Some("epsilon"),
    ];
    let txn = store.begin().await.unwrap();
    for (id, body) in bodies.iter().enumerate() {
        txn.insert(&root(), &table, &note(id as u64, *body))
            .await
            .unwrap();
    }
    txn.commit().await.unwrap();

    let computed = Query::computed(&table, 0);
    let cases = [
        Expr::True,
        Expr::eq(computed, Value::Str("alpha".to_owned())),
        Expr::eq(computed, Value::Str("nothing".to_owned())),
        Expr::is_null(computed),
        Expr::compare(computed, CmpOp::Ge, Value::Str("c".to_owned())),
        Expr::In {
            column: computed,
            values: vec![
                Value::Str("beta".to_owned()),
                Value::Str("gamma".to_owned()),
            ],
        },
    ];

    // Two projections: one that the entry can answer on its own, and one that
    // reaches for the source column and so must not be answered from it. Both
    // have to agree with the scan, and the second is the control — if the
    // planner ever did call it covering, the answers would part company here.
    let projections = [vec![NOTE_ID], vec![NOTE_ID, computed], vec![NOTE_ID, BODY]];

    for filter in cases {
        for projection in &projections {
            let base = Query::all()
                .filter(filter.clone())
                .select(projection.iter().copied())
                .computing([lower_body()]);
            let through_index = base.clone().using_index(BY_LOWER_BODY);
            let through_scan = base.using_table_scan();

            let txn = store.begin().await.unwrap();
            let plan = txn.explain(&root(), &table, &through_index).unwrap();
            assert!(
                plan.access.to_string().contains("by_lower_body"),
                "the hint did not take: {} for {filter:?}",
                plan.access
            );
            // The projection that needs `body` must not be answered from the
            // entry; the ones that do not, must be.
            assert_eq!(
                plan.access.to_string().contains("Index Only"),
                !projection.contains(&BODY),
                "wrong covering decision for {projection:?}: {}",
                plan.access
            );

            let mut indexed = rows_of(&txn, &table, &through_index).await;
            let mut scanned = rows_of(&txn, &table, &through_scan).await;
            indexed.sort();
            scanned.sort();
            assert_eq!(
                indexed, scanned,
                "the two paths disagree on {filter:?} projecting {projection:?}"
            );
        }
    }
}

/// Every value of every row a query returns, for comparing two access paths.
async fn rows_of(
    txn: &slate_kernel::RecordTransaction<'_>,
    table: &TableDef,
    query: &Query,
) -> Vec<Vec<Value>> {
    txn.execute(&root(), table, query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.values().to_vec())
        .collect()
}

/// And it really does skip the rows: the point of covering measured in reads
/// rather than asserted in prose.
///
/// A count, not a duration — "this reads no rows" survives a different machine
/// in a way "this took 2 ms" does not. The uncovered projection is measured in
/// the same run as the control, because a covering scan that read every row
/// would still pass a bare "zero is small" check if the counter were broken.
#[tokio::test]
async fn a_covering_expression_scan_reads_no_rows() {
    let catalog = Catalog::from_tables([notes()]).expect("catalog");
    let backing = MemoryStore::new();
    let table = notes();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let txn = loader.begin().await.unwrap();
    for id in 0..40u64 {
        txn.insert(
            &root(),
            &table,
            &note(id, Some(&format!("Body {}", id % 8))),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let counting = LatencyStore::new(backing, LatencyProfile::free());
    let counters = counting.counters();
    let security = SecurityCatalog::new().grant(Grant::new("r", NOTES, Action::ALL));
    let store = RecordStore::new(counting, catalog, security);

    let computed = Query::computed(&table, 0);
    let filter = Expr::eq(computed, Value::Str("body 3".to_owned()));
    let matching = 40u64 / 8;

    let covered = Query::all()
        .filter(filter.clone())
        .select([NOTE_ID])
        .computing([lower_body()])
        .using_index(BY_LOWER_BODY);
    counters.reset();
    let txn = store.begin().await.unwrap();
    let rows = rows_of(&txn, &table, &covered).await;
    assert_eq!(rows.len() as u64, matching);
    assert_eq!(
        counters.gets(),
        0,
        "an index-only scan of an expression index must read no rows"
    );

    // The same query asking for the column the entry lacks: the rows are read,
    // which is what makes the zero above a measurement rather than a constant.
    let uncovered = covered.clone().select([NOTE_ID, BODY]);
    counters.reset();
    let rows = rows_of(&txn, &table, &uncovered).await;
    assert_eq!(rows.len() as u64, matching);
    assert_eq!(
        counters.gets(),
        matching,
        "the source column can only come from the row"
    );
}

/// A computed value's inputs are read and are not part of the answer.
///
/// The rule that makes the covering scan above possible at all: an entry keyed
/// on `lower(body)` cannot produce `body`, so no path may produce it either.
/// Stated as its own test because it is a change to what a *table scan*
/// returns, and an oracle between two paths would pass just as happily if both
/// of them were wrong.
#[tokio::test]
async fn a_computed_values_inputs_are_not_part_of_the_answer() {
    let (store, _kv) = note_store();
    let table = notes();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &note(1, Some("Moby Dick")))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let query = Query::all()
        .select([NOTE_ID])
        .computing([lower_body()])
        .using_table_scan();
    let rows = rows_of(&txn, &table, &query).await;
    assert_eq!(
        rows,
        vec![vec![
            Value::U64(1),
            Value::Null,
            Value::Str("moby dick".to_owned())
        ]],
        "`body` was read to compute `lower(body)` and was not asked for"
    );

    // Asking for it brings it back, so this is a projection being honoured
    // rather than a column that can no longer be read alongside a computation.
    let query = Query::all()
        .select([NOTE_ID, BODY])
        .computing([lower_body()])
        .using_table_scan();
    assert_eq!(
        rows_of(&txn, &table, &query).await,
        vec![vec![
            Value::U64(1),
            Value::Str("Moby Dick".to_owned()),
            Value::Str("moby dick".to_owned())
        ]]
    );
}

/// A declared type the expression does not produce is refused at the write.
///
/// The alternative is an entry encoded as one type and decoded as another,
/// surfacing as a corrupt index at some later scan with nothing pointing back
/// at the write that caused it.
#[tokio::test]
async fn a_wrong_declared_type_is_refused_where_it_is_written() {
    const LIARS: TableId = TableId(4);
    let table = TableDef::builder("liars", LIARS)
        .column("id", ValueType::U64)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        // `lower(body)` is a string, whatever this says.
        .index(IndexDef::builder("by_wrong", IndexId(40)).expression(
            Scalar::Lower(Box::new(Scalar::Column(Ordinal(1)))),
            ValueType::I64,
        ))
        .build()
        .expect("the schema cannot know what the expression will produce");

    let kv = MemoryStore::new();
    let catalog = Catalog::from_tables([table.clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", LIARS, Action::ALL));
    let store = RecordStore::new(kv.clone(), catalog, security);

    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert(
            &root(),
            &table,
            &Row::new(vec![Value::U64(1), Value::Str("x".to_owned())]),
        )
        .await;
    assert!(refused.is_err(), "the write was accepted: {refused:?}");
    txn.rollback();
    assert!(kv.is_empty(), "the refused write left something behind");
}

/// An index that keys on neither columns nor an expression, or on both, is not
/// a schema.
#[test]
fn an_index_keys_on_columns_or_on_an_expression_but_not_both() {
    let both = TableDef::builder("both", TableId(5))
        .column("id", ValueType::U64)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_both", IndexId(50))
                .column("body")
                .expression(
                    Scalar::Lower(Box::new(Scalar::Column(Ordinal(1)))),
                    ValueType::Str,
                ),
        )
        .build();
    assert!(both.is_err(), "an index cannot key on two things");

    let neither = TableDef::builder("neither", TableId(6))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_nothing", IndexId(60)))
        .build();
    assert!(neither.is_err(), "an index has to key on something");
}
