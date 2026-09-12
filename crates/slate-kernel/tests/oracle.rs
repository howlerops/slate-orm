//! Whatever plan the planner picks, the answer is the same.
//!
//! A query optimiser has one obligation: to be invisible. Every access path for
//! a query must return the same rows, or the optimiser is not choosing between
//! plans, it is choosing between answers. That failure is quiet — the rows come
//! back, sorted and plausible, just wrong — and it is exactly what happened
//! here once already, when `ORDER BY` on an unprojected column compared nulls
//! and silently returned the scan's order instead. ClickBench found it by
//! accident. This finds that class on purpose.
//!
//! Three properties, in decreasing order of how much they prove and increasing
//! order of how much they assume:
//!
//! 1. [`every_access_path_returns_the_same_rows`] — the same query forced down
//!    a table scan and down each index must agree. Nothing is assumed about
//!    what the right answer is, only that there is one. This is the property
//!    that catches a bad scan bound, a covering scan reading a column it does
//!    not hold, or a sort key that one path decodes and another does not.
//! 2. [`scan_bounds_never_lose_a_row`] — the rows a plan returns must be the
//!    rows the predicate admits. Bound derivation and row filtering are
//!    separate pieces of code aiming at the same set, so disagreement is real
//!    even though both are ours.
//! 3. [`sorting_and_paging_agree_with_the_obvious_implementation`] — with a
//!    total order, `ORDER BY`/`LIMIT`/`OFFSET` must match sorting the rows and
//!    slicing them.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::{
    AccessHint, Action, CmpOp, Expr, Grant, Projection, Query, RecordStore, ScanOrder,
    SecurityCatalog, SecurityContext, SortKey, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};

const ITEMS: TableId = TableId(1);
const ROWS: u64 = 80;

/// Indexes are declared here so the oracle can force each one in turn; a new
/// index added to the table without being added here is simply not exercised.
const INDEXES: [IndexId; 4] = [IndexId(10), IndexId(11), IndexId(12), IndexId(13)];

fn items() -> TableDef {
    TableDef::builder("items", ITEMS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .column("score", ValueType::F64)
        .nullable_column("label", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .index(IndexDef::builder("by_size", IndexId(11)).column("size"))
        // Descending, so direction handling is exercised rather than assumed.
        .index(IndexDef::builder("by_score", IndexId(12)).column_with("score", Direction::Desc))
        .index(
            IndexDef::builder("by_kind_size", IndexId(13))
                .column("kind")
                .column("size"),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    items().ordinal_of(name).expect("column exists")
}

/// Values overlap heavily, so a predicate selects a varied slice rather than
/// everything or nothing, and duplicates exist on every indexed column.
fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 5)),
        Value::I64((id % 13) as i64 - 6),
        Value::F64((id % 7) as f64 / 2.0),
        if id.is_multiple_of(4) {
            Value::Null
        } else {
            Value::Str(format!("label-{}", id % 9))
        },
    ])
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([items()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", ITEMS, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let table = items();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table, &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

// --- generators -----------------------------------------------------------

/// A leaf predicate. The literals are drawn from the same small domains the
/// rows use, so most of these select a middling number of rows rather than
/// none.
fn leaf() -> impl Strategy<Value = Expr> {
    prop_oneof![
        (0..5u64).prop_map(|k| Expr::eq(col("kind"), Value::Str(format!("kind-{k}")))),
        (0..8u64, cmp_op()).prop_map(|(k, op)| Expr::compare(
            col("kind"),
            op,
            Value::Str(format!("kind-{k}"))
        )),
        (-8i64..8, cmp_op()).prop_map(|(n, op)| Expr::compare(col("size"), op, Value::I64(n))),
        (0..ROWS, cmp_op()).prop_map(|(n, op)| Expr::compare(col("id"), op, Value::U64(n))),
        (0..7u64, cmp_op()).prop_map(|(n, op)| Expr::compare(
            col("score"),
            op,
            Value::F64(n as f64 / 2.0)
        )),
        Just(Expr::is_null(col("label"))),
        Just(Expr::Not(Box::new(Expr::is_null(col("label"))))),
        // A prefix pattern becomes scan bounds rather than a filter, which is
        // the single most likely place for a bound to be derived too narrowly.
        (0..5u64).prop_map(|k| Expr::like(col("kind"), format!("kind-{k}%"))),
        Just(Expr::like(col("kind"), "%1")),
        (0..5u64).prop_map(|k| Expr::ilike(col("kind"), format!("KIND-{k}%"))),
        proptest::collection::vec(0..ROWS, 1..6).prop_map(|ids| Expr::In {
            column: col("id"),
            values: ids.into_iter().map(Value::U64).collect(),
        }),
        Just(Expr::True),
    ]
}

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

/// Predicates up to three deep. Conjunctions matter most — that is where two
/// columns' bounds have to be combined — but `OR` and `NOT` are where a bound
/// derived from one branch would be wrong for the whole.
fn any_filter() -> impl Strategy<Value = Expr> {
    leaf().prop_recursive(3, 12, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| a.and(b)),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::Or(vec![a, b])),
            inner.prop_map(|a| Expr::Not(Box::new(a))),
        ]
    })
}

fn any_sort() -> impl Strategy<Value = Vec<SortKey>> {
    let key = prop_oneof![
        Just(SortKey::asc(col("size"))),
        Just(SortKey::desc(col("size"))),
        Just(SortKey::asc(col("kind"))),
        Just(SortKey::desc(col("score"))),
        Just(SortKey::asc(col("label"))),
        Just(SortKey::desc(col("label"))),
    ];
    proptest::collection::vec(key, 0..2)
}

fn any_projection() -> impl Strategy<Value = Projection> {
    prop_oneof![
        Just(Projection::All),
        Just(Projection::Columns(vec![col("id")])),
        Just(Projection::Columns(vec![col("kind"), col("size")])),
        Just(Projection::Columns(vec![col("score")])),
        Just(Projection::none()),
    ]
}

/// A whole query. The primary key is appended to every sort so the order is
/// total: without it, `ORDER BY size LIMIT 5` has many correct answers and two
/// access paths may each return a different one, legitimately.
fn any_query() -> impl Strategy<Value = Query> {
    (
        any_filter(),
        any_sort(),
        any_projection(),
        prop_oneof![Just(None), (0..12usize).prop_map(Some)],
        0..4usize,
        prop_oneof![Just(ScanOrder::Ascending), Just(ScanOrder::Descending)],
    )
        .prop_map(|(filter, mut sort, projection, limit, offset, order)| {
            sort.push(SortKey::asc(col("id")));
            let mut query = Query::all().filter(filter).order(order).offset(offset);
            query.sort = sort;
            query.projection = projection;
            query.limit = limit;
            query
        })
}

// --- helpers --------------------------------------------------------------

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

async fn run(store: &RecordStore<MemoryStore>, query: &Query) -> Vec<Row> {
    let table = items();
    let txn = store.begin().await.unwrap();
    txn.execute(&root(), &table, query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
}

/// Rows a query returns, ignoring order and paging: the *set* it selected.
fn ids_of(rows: &[Row]) -> Vec<u64> {
    let mut ids: Vec<u64> = rows
        .iter()
        .map(|r| match r.get(col("id")) {
            Some(Value::U64(id)) => *id,
            other => panic!("id was {other:?}"),
        })
        .collect();
    ids.sort_unstable();
    ids
}

// --- the properties -------------------------------------------------------

/// Every access path returns the same rows for the same query.
///
/// This assumes nothing about what the answer should be. It only requires that
/// forcing a table scan, or any index, does not change it.
#[test]
fn every_access_path_returns_the_same_rows() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(query in any_query())| {
        let mut baseline = query.clone();
        baseline.hint = Some(AccessHint::TableScan);
        let expected = rt.block_on(run(&store, &baseline));

        // The planner's own choice, with no hint at all.
        let chosen = rt.block_on(run(&store, &query));
        prop_assert_eq!(
            &chosen, &expected,
            "the planner's choice disagreed with a table scan for {:?}",
            query.filter
        );

        for index in INDEXES {
            let mut forced = query.clone();
            forced.hint = Some(AccessHint::Index(index));
            let got = rt.block_on(run(&store, &forced));
            prop_assert_eq!(
                &got, &expected,
                "index {:?} disagreed with a table scan for {:?}",
                index, query.filter
            );
        }
    });
}

/// A plan returns exactly the rows the predicate admits — no more, and
/// crucially no fewer.
///
/// Scan bounds are derived by the planner from the predicate; `Expr::admits`
/// evaluates the predicate against a row. They are separate code reaching for
/// the same set, and a bound that is one row too narrow is invisible to any
/// test that only checks the rows it did return.
#[test]
fn scan_bounds_never_lose_a_row() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let all: Vec<Row> = (0..ROWS).map(row).collect();

    proptest!(|(filter in any_filter())| {
        let expected: Vec<u64> = all
            .iter()
            .filter(|r| filter.admits(r))
            .map(|r| match r.values()[0] {
                Value::U64(id) => id,
                ref other => panic!("id was {other:?}"),
            })
            .collect();

        // Unhinted, then through each index: a bound is derived per path.
        for hint in core::iter::once(None)
            .chain(core::iter::once(Some(AccessHint::TableScan)))
            .chain(INDEXES.into_iter().map(|i| Some(AccessHint::Index(i))))
        {
            let mut query = Query::all().filter(filter.clone());
            query.hint = hint;
            let got = ids_of(&rt.block_on(run(&store, &query)));
            prop_assert_eq!(
                &got, &expected,
                "{:?} with hint {:?} returned the wrong rows",
                filter, hint
            );
        }
    });
}

/// `ORDER BY`, `LIMIT` and `OFFSET` agree with sorting the rows and slicing.
///
/// The comparator is written out here rather than shared with the executor, so
/// the two are independent statements of the same rule — including the part
/// that catches people out, which is that null placement is absolute and not
/// flipped by a descending sort.
#[test]
fn sorting_and_paging_agree_with_the_obvious_implementation() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let all: Vec<Row> = (0..ROWS).map(row).collect();

    proptest!(|(query in any_query())| {
        // Projection is left out of this one: an unprojected column reads back
        // null, which would make the expectation a statement about projection
        // rather than about ordering.
        let mut query = query;
        query.projection = Projection::All;

        let mut expected: Vec<Row> = all
            .iter()
            .filter(|r| query.filter.admits(r))
            .cloned()
            .collect();
        expected.sort_by(|a, b| compare(a, b, &query.sort));
        let expected: Vec<u64> = expected
            .into_iter()
            .skip(query.offset)
            .take(query.limit.unwrap_or(usize::MAX))
            .map(|r| match r.values()[0] {
                Value::U64(id) => id,
                ref other => panic!("id was {other:?}"),
            })
            .collect();

        let got: Vec<u64> = rt
            .block_on(run(&store, &query))
            .iter()
            .map(|r| match r.get(col("id")) {
                Some(Value::U64(id)) => *id,
                other => panic!("id was {other:?}"),
            })
            .collect();

        prop_assert_eq!(
            &got, &expected,
            "ordering or paging differed for {:?} sort={:?} limit={:?} offset={}",
            query.filter, query.sort, query.limit, query.offset
        );
    });
}

/// The sort rule, stated independently of the executor's implementation.
fn compare(left: &Row, right: &Row, keys: &[SortKey]) -> core::cmp::Ordering {
    use core::cmp::Ordering;
    use slate_kernel::NullsOrder;

    for key in keys {
        let (Some(a), Some(b)) = (left.get(key.column), right.get(key.column)) else {
            continue;
        };
        let ordering = match (a.is_null(), b.is_null()) {
            (true, true) => Ordering::Equal,
            (true, false) | (false, true) => {
                let nulls_low = matches!(key.nulls, NullsOrder::First);
                // Absolute, not flipped by `direction`.
                return if a.is_null() == nulls_low {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, false) => match key.direction {
                Direction::Asc => a.cmp(b),
                Direction::Desc => b.cmp(a),
            },
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

// --- what the oracle found, pinned as examples --------------------------
//
// Each of these failed when the oracle was first run. They are kept by name
// because a property test only rediscovers them if the generator happens to
// produce the shape again, and a named test says what the bug was.

/// A covering index must not return a column the query did not ask for.
///
/// `by_kind_size` holds `size`, so reconstructing a row from the index entry
/// filled it in for free — while a table scan, honouring the projection, left
/// it null. Same query, two answers, decided by the optimiser.
#[tokio::test]
async fn a_covering_index_honours_the_projection() {
    let store = seeded().await;
    let query = Query::all()
        .filter(Expr::like(col("kind"), "kind-0%"))
        .select([col("id")]);

    let rows = run(&store, &query).await;
    assert!(!rows.is_empty(), "premise: the filter matches something");
    for row in &rows {
        assert_eq!(
            row.get(col("size")),
            Some(&Value::Null),
            "a column outside the projection came back populated: {row:?}"
        );
    }
}

/// A point get must not return a column the query did not ask for.
///
/// An equality on the whole primary key becomes a point get, which read and
/// decoded the entire row regardless of the projection. Whether a column came
/// back therefore depended on whether the planner reached the row by key or by
/// index.
#[tokio::test]
async fn a_point_get_honours_the_projection() {
    let store = seeded().await;
    let query = Query::all()
        .filter(Expr::eq(col("id"), Value::U64(1)))
        .select([col("id")]);

    let rows = run(&store, &query).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get(col("id")), Some(&Value::U64(1)));
    for unwanted in ["kind", "size", "score", "label"] {
        assert_eq!(
            rows[0].get(col(unwanted)),
            Some(&Value::Null),
            "`{unwanted}` came back from a point get that did not project it"
        );
    }
}

/// Contradictory bounds select nothing rather than panicking.
///
/// `id > 28 AND id < 0` is legal and matches no rows, but the bounds derived
/// from it run backwards, and `BTreeMap::range` panics on a backwards range.
/// An ordinary query should never be able to bring the process down.
#[tokio::test]
async fn contradictory_bounds_return_nothing() {
    let store = seeded().await;

    for filter in [
        Expr::compare(col("id"), CmpOp::Gt, Value::U64(28)).and(Expr::compare(
            col("id"),
            CmpOp::Lt,
            Value::U64(0),
        )),
        Expr::compare(col("size"), CmpOp::Gt, Value::I64(5)).and(Expr::compare(
            col("size"),
            CmpOp::Lt,
            Value::I64(-5),
        )),
        Expr::eq(col("kind"), Value::Str("kind-1".into()))
            .and(Expr::eq(col("kind"), Value::Str("kind-2".into()))),
    ] {
        let rows = run(&store, &Query::all().filter(filter.clone())).await;
        assert!(rows.is_empty(), "{filter:?} returned {} rows", rows.len());
    }
}

/// The generators have to actually generate something.
///
/// A property suite whose predicates all select zero rows passes every check
/// and proves nothing, so this asserts the corpus is being exercised.
#[test]
fn the_generated_queries_select_a_range_of_row_counts() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let counts = std::cell::RefCell::new(Vec::new());

    proptest!(ProptestConfig::with_cases(64), |(filter in any_filter())| {
        let query = Query::all().filter(filter);
        counts.borrow_mut().push(rt.block_on(run(&store, &query)).len());
    });

    let counts = counts.into_inner();
    let empty = counts.iter().filter(|n| **n == 0).count();
    let full = counts.iter().filter(|n| **n == ROWS as usize).count();
    let middling = counts.len() - empty - full;
    assert!(
        middling * 2 > counts.len(),
        "most generated predicates should select some but not all rows; \
         got {empty} empty, {full} full, {middling} in between of {}",
        counts.len()
    );
}
