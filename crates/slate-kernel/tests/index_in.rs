//! `IN` over a secondary index, which is several index ranges rather than one.
//!
//! `IN` over the *primary key* became point gets some time ago (see
//! `point_gets.rs`). Over a secondary index the same predicate had nothing:
//! `kind IN ('a', 'q')` matched the index's leading column, and the planner's
//! answer was a range covering everything from `a` to `q` — or, more often, the
//! whole index, since an `IN` pins no single value and so contributed no
//! equality prefix at all. Either way it read the index end to end.
//!
//! Two ranges read neither. That is the whole feature, and everything below is
//! about the two ways it could go wrong: reading the wrong rows, or reading
//! them in the wrong order.
//!
//! # What is measured here
//!
//! The counts are object-store *operations* against an in-memory store wrapped
//! in [`LatencyStore`], not wall-clock times: how many iterators were opened
//! and how many entries were pulled through them. That is the unit the cost
//! model is written in, so it is the unit worth asserting on — and it is the
//! only one an in-memory store can report honestly.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::plan::{Access, MAX_INDEX_RANGES, Projection, plan_full, plan_with};
use slate_kernel::{
    AccessHint, AccessSummary, Action, CmpOp, ColumnStats, Expr, Grant, Principal, Query,
    RecordStore, ScanOrder, SecurityCatalog, SecurityContext, SortKey, TableStats,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};
use std::sync::Arc;

const NOTES: TableId = TableId(1);
const BY_KIND: IndexId = IndexId(10);
const BY_KIND_SIZE: IndexId = IndexId(11);
const BY_SIZE_DESC: IndexId = IndexId(12);
const INDEXES: [IndexId; 3] = [BY_KIND, BY_KIND_SIZE, BY_SIZE_DESC];

const TENANTS: u64 = 2;
const PER_TENANT: u64 = 120;
const KINDS: u64 = 12;

/// Tenant-scoped on purpose: the tenant leads every index key, so an `IN` on
/// the index's own first column is *not* the first column of the key. Splitting
/// the wrong one produces ranges that each span the whole tenant — or, worse,
/// somebody else's.
fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_kind", BY_KIND).column("kind"))
        .index(
            IndexDef::builder("by_kind_size", BY_KIND_SIZE)
                .column("kind")
                .column("size"),
        )
        // Descending, so the ranges have to be ordered by their encoded bytes
        // rather than by the literals they came from.
        .index(IndexDef::builder("by_size_desc", BY_SIZE_DESC).column_with("size", Direction::Desc))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    notes().ordinal_of(name).expect("column exists")
}

fn row(tenant: u64, id: u64) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::Str(format!("k{:02}", id % KINDS)),
        Value::I64((id % 7) as i64 - 3),
        if id.is_multiple_of(4) {
            Value::Null
        } else {
            Value::Str(format!("note-{}", id % 5))
        },
    ])
}

fn kinds(values: impl IntoIterator<Item = u64>) -> Expr {
    Expr::In {
        column: col("kind"),
        values: values
            .into_iter()
            .map(|k| Value::Str(format!("k{k:02}")))
            .collect(),
    }
}

fn reader(tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(tenant))
            .with_role("r"),
    )
}

async fn store() -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
    let catalog = Catalog::from_tables([notes()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", NOTES, Action::EVERYTHING));
    let slow = LatencyStore::new(MemoryStore::new(), LatencyProfile::free());
    let counters = slow.counters();
    let store = RecordStore::new(slow, catalog, security);

    let root = SecurityContext::superuser();
    let rows: Vec<Row> = (0..TENANTS)
        .flat_map(|t| (0..PER_TENANT).map(move |id| row(t, id)))
        .collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root, &notes(), &rows).await.unwrap();
    txn.commit().await.unwrap();
    (store, counters)
}

/// Statistics for a table far larger than the one seeded, with `kind` spread
/// thinly enough that a handful of values is a handful of rows.
///
/// Invented rather than analysed, and deliberately: since the cost model was
/// recalibrated an index wins on the *absolute* number of rows it fetches, and
/// a 240-row corpus cannot express "few rows out of a great many". The seeded
/// corpus decides what the rows are; this decides what the planner believes
/// about their number, which is the thing being tested when a plan shape is
/// asserted.
fn big() -> TableStats {
    let spread = ColumnStats {
        distinct: 1_000_000,
        null_fraction: 0.0,
    };
    TableStats::with_row_count(1_000_000)
        // Two tenants, so the policy's own term keeps half the table rather
        // than the hundredth an un-analysed column is assumed to keep. Half a
        // million rows is what an index here has to beat, and a scan of them is
        // sixty-three requests — get this wrong and the fixture, not the cost
        // model, decides every assertion below.
        .with_column(
            col("tenant_id"),
            ColumnStats {
                distinct: 2,
                null_fraction: 0.0,
            },
        )
        .with_column(col("kind"), spread)
        .with_column(col("size"), spread)
        .with_column(col("id"), spread)
}

/// The planner's view of a query on the seeded tenant, with no execution.
fn plan_for(filter: &Expr, stats: &TableStats) -> Access {
    // The tenant term is what a policy would have conjoined; without it the
    // tenant column is unpinned and the `IN` is not on the leading key column.
    let secured = Expr::eq(col("tenant_id"), Value::U64(0)).and(filter.clone());
    plan_with(
        &notes(),
        &secured,
        ScanOrder::Ascending,
        &Projection::All,
        stats,
        None,
    )
    .access
}

fn ranges_of(access: &Access) -> usize {
    match access {
        Access::IndexScans { ranges, .. } => ranges.len(),
        other => panic!("expected several index ranges, got {other:?}"),
    }
}

async fn run(
    store: &RecordStore<LatencyStore<MemoryStore>>,
    tenant: u64,
    query: &Query,
) -> Vec<Row> {
    let txn = store.begin().await.unwrap();
    txn.execute(&reader(tenant), &notes(), query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
}

/// The `id` of each row, which identifies it within a tenant.
fn ids(rows: &[Row]) -> Vec<u64> {
    rows.iter()
        .map(|r| match r.values()[col("id").0] {
            Value::U64(id) => id,
            ref other => panic!("id was {other:?}"),
        })
        .collect()
}

// --- the plan -------------------------------------------------------------

/// The feature: three values, three ranges, one index.
#[test]
fn an_in_on_an_indexed_column_becomes_one_range_per_value() {
    let access = plan_for(&kinds([1, 5, 9]), &big());
    assert_eq!(ranges_of(&access), 3, "{access:?}");
    assert!(
        matches!(access, Access::IndexScans { index, .. } if index == BY_KIND),
        "{access:?}"
    );
}

/// Two ranges of the same index are not one range spanning both.
///
/// The failure this guards against does not look like a failure: a hull range
/// returns the right rows, because the residual removes everything between the
/// endpoints. It just reads the whole span to do it. So the assertion is on the
/// bytes — the ranges must not touch, or they would have been better as one.
#[test]
fn the_ranges_are_disjoint_and_hold_nothing_between_the_values() {
    let Access::IndexScans { ranges, .. } = plan_for(&kinds([0, 11]), &big()) else {
        panic!("expected several ranges");
    };
    assert_eq!(ranges.len(), 2);
    // A key inside the gap — `k05` — belongs to neither range. One hull range
    // would contain it, and would read every entry that shares its prefix.
    let middle = plan_for(&kinds([5]), &big());
    let Access::IndexScan { range: gap, .. } = middle else {
        panic!("one value should be one range, got {middle:?}");
    };
    let probe = match &gap.start {
        core::ops::Bound::Included(key) | core::ops::Bound::Excluded(key) => key.clone(),
        core::ops::Bound::Unbounded => panic!("a pinned value has a lower bound"),
    };
    for range in &ranges {
        assert!(
            !range.contains(&probe),
            "a value that was not asked for falls inside {range:?}"
        );
    }
}

/// One value is one range, and one range is an ordinary index scan.
///
/// Not a special case for its own sake: `IN ('a')` arriving as a one-element
/// `IndexScans` would be a second spelling of a plan that already exists, and
/// every later stage would have to handle both.
#[test]
fn a_single_value_is_a_plain_index_scan() {
    let access = plan_for(&kinds([3]), &big());
    assert!(
        matches!(access, Access::IndexScan { index, .. } if index == BY_KIND),
        "{access:?}"
    );
}

/// Repeats collapse. Two identical ranges would return every row in them twice.
#[test]
fn repeated_values_are_one_range() {
    assert_eq!(ranges_of(&plan_for(&kinds([4, 4, 4, 7]), &big())), 2);
}

/// A value contradicted by another bound contributes no range at all.
///
/// `kind IN ('k04', 'k09') AND kind > 'k05'` leaves one value standing. Pinning
/// the column to a value hides the `>` from the bound derivation — an equality
/// wins over a range there — so without this check the plan carries a range for
/// `k04` that is scanned in full and rejected in full.
#[test]
fn a_contradicted_value_drops_out_rather_than_being_scanned_for_nothing() {
    let filter = kinds([4, 9]).and(Expr::compare(
        col("kind"),
        CmpOp::Gt,
        Value::Str("k05".to_owned()),
    ));
    let access = plan_for(&filter, &big());
    assert!(
        matches!(access, Access::IndexScan { index, .. } if index == BY_KIND),
        "one surviving value should be one range, got {access:?}"
    );
}

/// Every value contradicted leaves nothing to split on.
///
/// The union collapses and the planner falls back to the key range — not to
/// `Nothing`, because nothing here proves the table is empty; the residual
/// still has the last word. What must not survive is a range per value.
#[test]
fn every_value_contradicted_falls_back_rather_than_splitting() {
    let filter = kinds([1, 2]).and(Expr::compare(
        col("kind"),
        CmpOp::Gt,
        Value::Str("k09".to_owned()),
    ));
    let secured = Expr::eq(col("tenant_id"), Value::U64(0)).and(filter);
    let plan = plan_with(
        &notes(),
        &secured,
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
    );
    // The index candidate collapses, so the planner falls back to the key
    // range — which is a scan of one tenant, not `Nothing`, because nothing
    // proves the *table* is empty. What must not happen is a range per value.
    assert!(
        !matches!(plan.access, Access::IndexScans { .. }),
        "{:?}",
        plan.access
    );
}

/// Nulls in the set buy no range, and lose no rows.
///
/// `x IN (NULL, 'k01')` is true only where `x = 'k01'`: a null candidate makes
/// the test *unknown* everywhere else, and unknown admits nothing. A range for
/// the null would read entries the residual then throws away.
#[test]
fn a_null_in_the_set_is_not_a_range() {
    let filter = Expr::In {
        column: col("kind"),
        values: vec![
            Value::Null,
            Value::Str("k01".to_owned()),
            Value::Str("k02".to_owned()),
        ],
    };
    assert_eq!(ranges_of(&plan_for(&filter, &big())), 2);
}

/// The split happens on the first key column the equalities do not already pin,
/// and not on a later one.
///
/// With `(kind, size)` and only `size IN (…)`, a range per size would have to
/// span every `kind` — which is the whole index, several times over.
#[test]
fn an_in_on_a_later_index_column_is_not_split() {
    let filter = Expr::In {
        column: col("size"),
        values: vec![Value::I64(-1), Value::I64(2)],
    };
    let secured = Expr::eq(col("tenant_id"), Value::U64(0)).and(filter);
    let plan = plan_with(
        &notes(),
        &secured,
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
    );
    // `by_size_desc` leads on `size`, so *that* index may be split — the point
    // is that `by_kind_size`, where `size` is the second column, is not.
    if let Access::IndexScans { index, .. } = plan.access {
        assert_ne!(
            index, BY_KIND_SIZE,
            "split on a column that is not the leading one"
        );
    }
}

/// An `IN` on a column that is *also* the whole primary key still becomes point
/// gets, which are cheaper than ranges over an index that points back at them.
#[test]
fn the_primary_key_case_is_unchanged() {
    let filter = Expr::In {
        column: col("id"),
        values: (1..=4).map(Value::U64).collect(),
    };
    let access = plan_for(&filter, &big());
    assert!(
        matches!(access, Access::PointGets { ref keys } if keys.len() == 4),
        "{access:?}"
    );
}

// --- the cost -------------------------------------------------------------

/// Ranges are costed in requests, and a request per range is what they cost.
///
/// A hundred values is a hundred iterators to open before a single row is read.
/// Against a table scan of a million rows — twenty-six requests, since a scan
/// returns about eight thousand rows per request — the union has to lose, and
/// the cost model has to be the thing that says so rather than a cap.
#[test]
fn a_wide_in_loses_to_a_table_scan() {
    let wide = kinds(0..100);
    let access = plan_for(&wide, &big());
    assert!(
        matches!(access, Access::TableScan { .. }),
        "a hundred ranges should lose to a scan, got {access:?}"
    );

    // And a narrow one wins, or the comparison above proves nothing.
    let narrow = plan_for(&kinds([1, 2, 3]), &big());
    assert_eq!(ranges_of(&narrow), 3);
}

/// The cap is a bound on planning work, not on the decision.
///
/// The cost model rejects a large set on its own; this stops the planner from
/// encoding a hundred thousand key pairs to find that out.
#[test]
fn an_enormous_set_is_not_expanded() {
    let values: Vec<Value> = (0..=MAX_INDEX_RANGES as u64)
        .map(|n| Value::Str(format!("k{n:06}")))
        .collect();
    let filter = Expr::In {
        column: col("kind"),
        values,
    };
    let access = plan_for(&filter, &big());
    assert!(
        !matches!(access, Access::IndexScans { .. }),
        "{access:?} expanded past the cap"
    );
}

/// A covering union pays no row read at all, and so wins where a fetching one
/// would not.
///
/// This is the shape that makes the feature worth having on a wide table: the
/// entries answer the query, so the cost is the opens plus the entries scanned.
#[test]
fn a_covering_union_is_chosen_where_a_fetching_one_is_not() {
    let secured = Expr::eq(col("tenant_id"), Value::U64(0)).and(kinds([1, 3, 5, 7]));
    let plan = plan_with(
        &notes(),
        &secured,
        ScanOrder::Ascending,
        &Projection::Columns(vec![col("kind"), col("size")]),
        &big(),
        None,
    );
    match plan.access {
        Access::IndexScans {
            index,
            ref ranges,
            covering,
        } => {
            assert_eq!(index, BY_KIND_SIZE);
            assert_eq!(ranges.len(), 4);
            assert!(covering, "the index holds every column the query reads");
        }
        ref other => panic!("expected a covering union, got {other:?}"),
    }
    // Four opens and nothing else: the estimate has to say so too, or `EXPLAIN`
    // is describing a plan nobody is running.
    assert!(
        (plan.estimated_cost - 4.0).abs() < 0.5,
        "cost was {}",
        plan.estimated_cost
    );
}

// --- the rows -------------------------------------------------------------

/// The rows are the rows, and the reads are only the ones asked for.
#[tokio::test]
async fn an_in_over_an_index_reads_only_the_matching_entries() {
    let (store, counters) = store().await;
    let mut query = Query::all().filter(kinds([1, 5]));
    query.hint = Some(AccessHint::Index(BY_KIND));

    let txn = store.begin().await.unwrap();
    let explained = txn.explain(&reader(0), &notes(), &query).unwrap();
    assert_eq!(
        explained.access,
        AccessSummary::IndexScans {
            index: "by_kind".to_owned(),
            ranges: 2,
        },
        "got {explained}"
    );
    drop(txn);

    counters.reset();
    let rows = run(&store, 0, &query).await;
    // Nothing asked for an order, so this is the index's: every `k01` and then
    // every `k05`, each by primary key. A table scan would have interleaved
    // them, which is why the oracle below sorts before it compares and why a
    // caller who cares says so.
    let mut expected: Vec<u64> = (0..PER_TENANT)
        .filter(|id| [1, 5].contains(&(id % KINDS)))
        .collect();
    expected.sort_by_key(|id| (id % KINDS, *id));
    assert_eq!(ids(&rows), expected);

    // Two ranges, two iterators, and the entries pulled are the twenty matching
    // ones and nothing else.
    assert_eq!(counters.scans(), 2, "one iterator per range");
    assert_eq!(
        counters.scan_rows(),
        expected.len() as u64,
        "only the matching index entries were walked"
    );
    let split = counters.scan_rows();

    // What one range spanning both values costs, measured rather than argued:
    // the same index, hinted the same way, over `k01..=k05`. Fifty entries
    // against twenty, because the span holds five of the twelve kinds and the
    // union holds two. The ratio is the gap between the values, so on a column
    // with a thousand of them a hull range is a thousand times worse. Both
    // sides are one tenant of a 240-row corpus: this says what shape the work
    // has, not what a real table would cost.
    let hull = Query::all().filter(
        Expr::compare(col("kind"), CmpOp::Ge, Value::Str("k01".to_owned())).and(Expr::compare(
            col("kind"),
            CmpOp::Le,
            Value::Str("k05".to_owned()),
        )),
    );
    let mut hull = hull;
    hull.hint = Some(AccessHint::Index(BY_KIND));
    counters.reset();
    run(&store, 0, &hull).await;
    assert_eq!(counters.scans(), 1, "premise: the hull really is one range");
    assert_eq!(counters.scan_rows(), 5 * PER_TENANT / KINDS, "five kinds");
    assert_eq!(split, 2 * PER_TENANT / KINDS, "two kinds");
}

/// A union produces the index's own order, so an `ORDER BY` it satisfies does
/// not sort.
///
/// The ranges are concatenated, not merged: each pins a different value of the
/// leading column, so reading them in key order is reading the index in key
/// order with the other values left out. If they were walked in the order the
/// caller wrote them, this returns rows out of order while every other test
/// still passes.
#[tokio::test]
async fn the_order_is_the_index_order_without_a_sort() {
    let (store, _) = store().await;
    let sort = vec![SortKey::asc(col("kind")), SortKey::asc(col("id"))];
    let filter = Expr::eq(col("tenant_id"), Value::U64(0)).and(kinds([9, 2, 5]));
    let plan = plan_full(
        &notes(),
        Arc::new(filter),
        ScanOrder::Ascending,
        &Projection::All,
        &big(),
        None,
        &sort,
    );
    assert_eq!(ranges_of(&plan.access), 3);
    assert!(
        plan.sort.is_none(),
        "the union already produces the requested order"
    );

    let mut query = Query::all().filter(kinds([9, 2, 5]));
    query.hint = Some(AccessHint::Index(BY_KIND));
    query.sort = sort;
    let rows = run(&store, 0, &query).await;

    let mut expected = ids(&rows);
    expected.sort_by_key(|id| (id % KINDS, *id));
    assert_eq!(ids(&rows), expected, "the rows came back out of order");
    assert_eq!(rows.len(), 3 * (PER_TENANT / KINDS) as usize);
}

/// Walked the other way, the ranges come in the other order.
#[tokio::test]
async fn a_descending_walk_reverses_the_ranges() {
    let (store, _) = store().await;
    let mut query = Query::all()
        .filter(kinds([2, 5, 9]))
        .order(ScanOrder::Descending);
    query.hint = Some(AccessHint::Index(BY_KIND));

    let rows = run(&store, 0, &query).await;
    let mut expected = ids(&rows);
    expected.sort_by_key(|id| (core::cmp::Reverse(id % KINDS), core::cmp::Reverse(*id)));
    assert_eq!(ids(&rows), expected, "a descending walk was not descending");
}

/// A descending *column* orders its ranges by the encoded bytes, which is the
/// reverse of the literals they came from.
///
/// Sorting the values instead of the keys puts the ranges in the wrong order on
/// this index only, which is exactly the sort of thing that survives a test
/// suite built on ascending columns.
#[tokio::test]
async fn a_descending_index_column_still_yields_index_order() {
    let (store, _) = store().await;
    let filter = Expr::In {
        column: col("size"),
        values: vec![Value::I64(-3), Value::I64(0), Value::I64(3)],
    };
    let mut query = Query::all().filter(filter);
    query.hint = Some(AccessHint::Index(BY_SIZE_DESC));
    query.sort = vec![SortKey::desc(col("size")), SortKey::asc(col("id"))];

    let rows = run(&store, 0, &query).await;
    let sizes: Vec<i64> = rows
        .iter()
        .map(|r| match r.values()[col("size").0] {
            Value::I64(size) => size,
            ref other => panic!("size was {other:?}"),
        })
        .collect();
    let mut sorted = sizes.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(sizes, sorted, "a descending index came back out of order");
    assert!(!sizes.is_empty(), "premise: the filter matches something");
}

/// A limit stops the walk, and a range past it is never opened.
#[tokio::test]
async fn a_limit_stops_before_the_later_ranges() {
    let (store, counters) = store().await;
    let mut query = Query::all().filter(kinds([1, 5, 9])).limit(3);
    query.hint = Some(AccessHint::Index(BY_KIND));

    counters.reset();
    let rows = run(&store, 0, &query).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        counters.scans(),
        1,
        "the first range answered the limit, so no other was opened"
    );
}

/// The tenant prefix binds every range.
///
/// A range is built from the policy's tenant term plus one value of the `IN`.
/// Getting the prefix wrong in one of several ranges is the failure a
/// single-range test cannot see.
#[tokio::test]
async fn no_range_reaches_another_tenant() {
    let (store, _) = store().await;
    for tenant in 0..TENANTS {
        let mut query = Query::all().filter(kinds([0, 3, 7]));
        query.hint = Some(AccessHint::Index(BY_KIND));
        for row in run(&store, tenant, &query).await {
            assert_eq!(
                row.values()[col("tenant_id").0],
                Value::U64(tenant),
                "a range escaped into another tenant"
            );
        }
    }
}

// --- the oracle -----------------------------------------------------------
//
// The property every access path owes: the rows do not depend on the plan. A
// new path has to join it or it is only tested by the cases somebody thought
// of, and the cases somebody thought of are never the ones with the bug.

fn cmp_op() -> impl Strategy<Value = CmpOp> {
    prop_oneof![
        Just(CmpOp::Eq),
        Just(CmpOp::Ne),
        Just(CmpOp::Lt),
        Just(CmpOp::Le),
        Just(CmpOp::Gt),
        Just(CmpOp::Ge),
    ]
}

/// Leaves weighted towards `IN`, since that is what this file is about, with
/// enough of everything else that an `IN` has to compose with other bounds.
fn leaf() -> impl Strategy<Value = Expr> {
    prop_oneof![
        proptest::collection::vec(0..KINDS, 1..5).prop_map(kinds),
        proptest::collection::vec(-4i64..4, 1..4).prop_map(|sizes| Expr::In {
            column: col("size"),
            values: sizes.into_iter().map(Value::I64).collect(),
        }),
        proptest::collection::vec(0..PER_TENANT, 1..4).prop_map(|ids| Expr::In {
            column: col("id"),
            values: ids.into_iter().map(Value::U64).collect(),
        }),
        (0..KINDS, cmp_op()).prop_map(|(k, op)| Expr::compare(
            col("kind"),
            op,
            Value::Str(format!("k{k:02}"))
        )),
        (-4i64..4, cmp_op()).prop_map(|(s, op)| Expr::compare(col("size"), op, Value::I64(s))),
        (0..PER_TENANT, cmp_op()).prop_map(|(n, op)| Expr::compare(col("id"), op, Value::U64(n))),
        Just(Expr::is_null(col("note"))),
        // A null among the candidates: unknown for every row it does not
        // match, which is a different thing from false.
        Just(Expr::In {
            column: col("kind"),
            values: vec![Value::Null, Value::Str("k03".to_owned())],
        }),
        Just(Expr::True),
    ]
}

/// Conjunctions three times as often as anything else.
///
/// Not for realism: a term inside an `OR` or a `NOT` contributes no bound at
/// all, so a generator that reaches for them evenly spends most of its cases on
/// predicates the planner turns into a plain scan — and then the oracle is
/// testing the scan again rather than the union. `OR` and `NOT` still appear,
/// because a bound wrongly derived from one branch is a real bug and this is
/// where it would show.
fn any_filter() -> impl Strategy<Value = Expr> {
    leaf().prop_recursive(3, 10, 2, |inner| {
        prop_oneof![
            3 => (inner.clone(), inner.clone()).prop_map(|(a, b)| a.and(b)),
            1 => (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::Or(vec![a, b])),
            1 => inner.prop_map(|a| Expr::Not(Box::new(a))),
        ]
    })
}

fn any_query() -> impl Strategy<Value = Query> {
    (
        any_filter(),
        proptest::collection::vec(
            prop_oneof![
                Just(SortKey::asc(col("kind"))),
                // Descending as well as ascending, because a union walked
                // backwards has to take its ranges backwards too — and a sort
                // no path satisfies is sorted by the executor, which hides
                // exactly that.
                Just(SortKey::desc(col("kind"))),
                Just(SortKey::desc(col("size"))),
                Just(SortKey::asc(col("size"))),
                Just(SortKey::desc(col("note"))),
            ],
            0..2,
        ),
        // The trailing key that makes the order total, in either direction: a
        // descending index scan produces descending keys, so an ascending tie
        // break is never satisfied by one.
        prop_oneof![
            Just(SortKey::asc(col("id"))),
            Just(SortKey::desc(col("id")))
        ],
        prop_oneof![
            Just(Projection::All),
            Just(Projection::Columns(vec![col("id")])),
            Just(Projection::Columns(vec![col("kind"), col("size")])),
            Just(Projection::none()),
        ],
        prop_oneof![Just(None), (0..8usize).prop_map(Some)],
        0..3usize,
        prop_oneof![Just(ScanOrder::Ascending), Just(ScanOrder::Descending)],
    )
        .prop_map(
            |(filter, mut sort, tie_break, projection, limit, offset, order)| {
                // The primary key last, so the order is total: without it two paths
                // may each return a different correct answer under a limit.
                sort.push(tie_break);
                let mut query = Query::all().filter(filter).order(order).offset(offset);
                query.sort = sort;
                query.projection = projection;
                query.limit = limit;
                query
            },
        )
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// Every access path returns the same rows, with `IN` in the generator and a
/// union among the paths.
///
/// More cases than proptest's default, which was measured rather than guessed:
/// with the executor's descending reversal deliberately removed, 256 cases
/// passed and 1,500 failed. The shape that catches it needs four independent
/// draws to line up — a descending walk, a union, and a sort the union
/// satisfies in both its keys — and at the default rate that is a coin flip.
#[test]
fn every_access_path_agrees_with_an_in_over_an_index() {
    let rt = runtime();
    let (store, _) = rt.block_on(store());

    proptest!(ProptestConfig::with_cases(1_024), |(query in any_query())| {
        let mut baseline = query.clone();
        baseline.hint = Some(AccessHint::TableScan);
        let expected = rt.block_on(run(&store, 1, &baseline));

        let chosen = rt.block_on(run(&store, 1, &query));
        prop_assert_eq!(&chosen, &expected,
            "the planner's choice disagreed with a table scan for {:?}", query.filter);

        for index in INDEXES {
            let mut forced = query.clone();
            forced.hint = Some(AccessHint::Index(index));
            let got = rt.block_on(run(&store, 1, &forced));
            prop_assert_eq!(&got, &expected,
                "index {:?} disagreed with a table scan for {:?}", index, query.filter);
        }
    });
}

/// A union returns exactly the rows the predicate admits — the ones between the
/// values as well as the ones on them.
#[test]
fn a_union_never_loses_a_row() {
    let rt = runtime();
    let (store, _) = rt.block_on(store());
    let all: Vec<Row> = (0..PER_TENANT).map(|id| row(1, id)).collect();

    proptest!(|(filter in any_filter())| {
        let expected: Vec<u64> = all
            .iter()
            .filter(|r| filter.admits(r))
            .map(|r| match r.values()[col("id").0] {
                Value::U64(id) => id,
                ref other => panic!("id was {other:?}"),
            })
            .collect();

        for hint in core::iter::once(None)
            .chain(core::iter::once(Some(AccessHint::TableScan)))
            .chain(INDEXES.into_iter().map(|i| Some(AccessHint::Index(i))))
        {
            let mut query = Query::all().filter(filter.clone());
            query.hint = hint;
            let mut got = ids(&rt.block_on(run(&store, 1, &query)));
            got.sort_unstable();
            prop_assert_eq!(&got, &expected,
                "{:?} with hint {:?} returned the wrong rows", filter, hint);
        }
    });
}

/// Cases for the generator-quality check below: enough that the observed rate
/// is stable to a couple of percent.
const SAMPLE: u32 = 600;

/// The generator has to actually produce unions, or the oracle above is a
/// second copy of `oracle.rs`.
///
/// Counted on the plan rather than on the query text: what matters is that the
/// new access path was exercised, not that the word `IN` appeared.
#[test]
fn the_generated_queries_produce_unions() {
    let unions = std::cell::RefCell::new(0usize);
    let total = std::cell::RefCell::new(0usize);
    let stats = big();

    proptest!(ProptestConfig::with_cases(SAMPLE), |(filter in any_filter())| {
        *total.borrow_mut() += 1;
        for index in INDEXES {
            let secured = Expr::eq(col("tenant_id"), Value::U64(1)).and(filter.clone());
            let plan = slate_kernel::plan::plan_hinted(
                &notes(),
                Arc::new(secured),
                ScanOrder::Ascending,
                &Projection::All,
                &stats,
                None,
                &[],
                Some(AccessHint::Index(index)),
                &[],
            );
            if matches!(plan.access, Access::IndexScans { .. }) {
                *unions.borrow_mut() += 1;
                break;
            }
        }
    });

    let (unions, total) = (unions.into_inner(), total.into_inner());
    // Measured at 13% and 15% on two runs of 600 cases. The bar is 7%, about
    // five standard errors below that: a generator-quality check that fires one
    // run in twenty is worse than no check, because it teaches people to rerun
    // the suite.
    assert!(
        unions * 14 > total,
        "only {unions} of {total} generated predicates planned as a union"
    );
}
