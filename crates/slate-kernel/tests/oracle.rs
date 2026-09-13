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
    AccessHint, Action, CmpOp, Expr, Grant, Projection, Query, RecordStore, Scalar, ScanOrder,
    SecurityCatalog, SecurityContext, SortKey, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};

const ITEMS: TableId = TableId(1);
const ROWS: u64 = 80;

/// Indexes are declared here so the oracle can force each one in turn; a new
/// index added to the table without being added here is simply not exercised.
///
/// The last two key on a value no column holds. Forcing one of those for a
/// query that does not compute the same expression is not an error — the index
/// simply is not a candidate and the hint falls through to the primary key —
/// so they cost nothing on the queries that ignore them and are the whole
/// point of the ones that do not.
const INDEXES: [IndexId; 6] = [
    IndexId(10),
    IndexId(11),
    IndexId(12),
    IndexId(13),
    BY_LOWER_LABEL,
    BY_LABEL_LENGTH,
];

const BY_LOWER_LABEL: IndexId = IndexId(14);
const BY_LABEL_LENGTH: IndexId = IndexId(15);

/// The table's own columns, by position.
///
/// `col` resolves a name by building the table, and the table now names
/// expressions over these columns, so an expression cannot go back through
/// `col` without recursing forever. [`the_ordinals_are_where_they_are_claimed`]
/// pins them, so a reordered schema fails a test rather than silently indexing
/// something else.
const ID: Ordinal = Ordinal(0);
const KIND: Ordinal = Ordinal(1);
const LABEL: Ordinal = Ordinal(4);
/// Columns the table has. Anything at or past this is a computed value.
const WIDTH: usize = 5;

fn lower_label() -> Scalar {
    Scalar::Lower(Box::new(Scalar::Column(LABEL)))
}

fn label_length() -> Scalar {
    Scalar::Length(Box::new(Scalar::Column(LABEL)))
}

fn upper_kind() -> Scalar {
    Scalar::Upper(Box::new(Scalar::Column(KIND)))
}

/// Where the `n`th computed value of a query lands.
const fn computed(n: usize) -> Ordinal {
    Ordinal(WIDTH + n)
}

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
        // Two expression indexes, keyed on a value the row does not hold. They
        // are deliberately over the *nullable* column: `lower(null)` is exempt
        // from the declared-type check, so a null is a live entry rather than
        // a missing one, and a covering scan that recomputed from a row rebuilt
        // out of an entry would get exactly that null for every row.
        //
        // Chosen so they cannot be confused with each other: one produces text
        // and one an integer, and no row has the same value in both.
        .index(
            IndexDef::builder("by_lower_label", BY_LOWER_LABEL)
                .expression(lower_label(), ValueType::Str),
        )
        // Descending, and deliberately: an expression index's key direction is
        // declared separately from an ordinary index's, so a bound built for a
        // descending expression has to be inverted by code the ascending case
        // never runs. `by_score` does the same job for an ordinary index.
        .index(
            IndexDef::builder("by_label_length", BY_LABEL_LENGTH).expression_with(
                label_length(),
                ValueType::I64,
                Direction::Desc,
            ),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    items().ordinal_of(name).expect("column exists")
}

#[test]
fn the_ordinals_are_where_they_are_claimed() {
    assert_eq!(ID, col("id"));
    assert_eq!(KIND, col("kind"));
    assert_eq!(LABEL, col("label"));
    assert_eq!(WIDTH, items().columns().len());
}

/// Values overlap heavily, so a predicate selects a varied slice rather than
/// everything or nothing, and duplicates exist on every indexed column.
///
/// `label` is deliberately awkward for the expression indexes: it is null a
/// quarter of the time, it is spelled in two cases so `lower(label)` is not the
/// column back again, and it comes in two lengths so `length(label)` is not one
/// value for the whole table. A fixture where the expression is the identity
/// would let a covering scan return the source column and still agree.
fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 5)),
        Value::I64((id % 13) as i64 - 6),
        Value::F64((id % 7) as f64 / 2.0),
        if id.is_multiple_of(4) {
            Value::Null
        } else if id.is_multiple_of(2) {
            Value::Str(format!("Label-{}", id % 9))
        } else {
            Value::Str(format!("lbl{}", id % 9))
        },
    ])
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([items()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", ITEMS, Action::ALL));
    let mut store = RecordStore::new(MemoryStore::new(), catalog, security);
    let table = items();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table, &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    // Analysed rather than left on the defaults, so the planner's *own* choice
    // is made against numbers that describe this table — including an
    // expression index's, which `analyze` measures by evaluating the expression
    // over the rows it samples. Without it every expression index looks like a
    // hundred distinct values and the unhinted arm of the oracle would never
    // pick one, leaving the new surface proved only where it is forced.
    let txn = store.begin().await.unwrap();
    let stats = txn.analyze(&root, &table).await.unwrap();
    txn.commit().await.unwrap();
    let mut statistics = slate_kernel::Statistics::new();
    statistics.set(ITEMS, stats);
    store.set_statistics(statistics);
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

// --- computed values ------------------------------------------------------
//
// A query may compute extra values, which land after the table's own columns
// and are addressed by ordinal like everything else. That is the surface the
// expression indexes above are reachable through, and it is what the generator
// below produces.
//
// The compute list is drawn *first* and everything else is drawn against it,
// because a predicate on the third computed value of a query that computes one
// is not a hard case, it is a null. The type each position produces is carried
// alongside for the same reason: `Value`'s order is type-first, so an integer
// compared against a string answers the same way for every row and the case
// tests nothing.

/// What one position of a compute list produces, so a generated predicate can
/// compare it against a literal of the right type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Produces {
    /// `lower(label)`, or `lower` of something already lowered.
    LowerLabel,
    /// `length(...)` of a label: an integer, or null for a null label.
    LabelLength,
    /// `upper(kind)`: text no expression index keys on.
    UpperKind,
    /// The primary key, copied into a computed slot: a value every index can
    /// produce, which is what lets a computed position sit in front of the one
    /// an entry answers for.
    Id,
}

/// How many compute lists the generator draws from.
const PROGRAMS: usize = 7;

/// One compute list, and what each of its positions produces.
///
/// Written out rather than generated freely so the *shape* of each case is
/// something that can be reasoned about: which index can supply the value, and
/// which cannot. Between them these cover an expression index supplying the
/// value it keys on, two computed values where only one comes out of the entry,
/// a computed value no expression index has (which an ordinary index can still
/// cover, since it holds what the expression reads), and a computed value that
/// reads an *earlier* computed value the entry supplies.
fn program(which: usize) -> (Vec<Scalar>, Vec<Produces>) {
    match which {
        0 => (Vec::new(), Vec::new()),
        1 => (vec![lower_label()], vec![Produces::LowerLabel]),
        2 => (vec![label_length()], vec![Produces::LabelLength]),
        3 => (
            vec![lower_label(), label_length()],
            vec![Produces::LowerLabel, Produces::LabelLength],
        ),
        4 => (vec![upper_kind()], vec![Produces::UpperKind]),
        // `length(lower(label))` written as a chain: position 1 reads position
        // 0, which `by_lower_label` supplies out of its entry. This is the case
        // the covering rule used to give up on.
        5 => (
            vec![
                lower_label(),
                Scalar::Length(Box::new(Scalar::Column(computed(0)))),
            ],
            vec![Produces::LowerLabel, Produces::LabelLength],
        ),
        // The index's own value *second*, behind one the primary key alone can
        // produce. Which computed value an entry stands for is answered by one
        // function for the planner and the executor alike, and an answer that
        // was right only when the expression came first would agree with every
        // program above.
        6 => (
            vec![Scalar::Column(ID), lower_label()],
            vec![Produces::Id, Produces::LowerLabel],
        ),
        other => panic!("no program {other}"),
    }
}

/// A predicate over one computed value, drawn from the domain it produces.
fn computed_leaf(at: Ordinal, produces: Produces) -> BoxedStrategy<Expr> {
    match produces {
        Produces::LowerLabel => prop_oneof![
            (0..9u64, any::<bool>()).prop_map(move |(k, long)| Expr::eq(
                at,
                Value::Str(if long {
                    format!("label-{k}")
                } else {
                    format!("lbl{k}")
                })
            )),
            (0..9u64, cmp_op()).prop_map(move |(k, op)| Expr::compare(
                at,
                op,
                Value::Str(format!("label-{k}"))
            )),
            // The value the *source column* is null for. An index entry
            // carries it like any other, and a covering scan that recomputed
            // from the rebuilt row would find every row matching this.
            Just(Expr::is_null(at)),
            Just(Expr::Not(Box::new(Expr::is_null(at)))),
            (0..9u64).prop_map(move |k| Expr::like(at, format!("label-{k}%"))),
            proptest::collection::vec(0..9u64, 1..4).prop_map(move |ks| Expr::In {
                column: at,
                values: ks
                    .into_iter()
                    .map(|k| Value::Str(format!("label-{k}")))
                    .collect(),
            }),
        ]
        .boxed(),
        Produces::LabelLength => prop_oneof![
            (2..9i64, cmp_op()).prop_map(move |(n, op)| Expr::compare(at, op, Value::I64(n))),
            Just(Expr::is_null(at)),
            proptest::collection::vec(2..9i64, 1..3).prop_map(move |ns| Expr::In {
                column: at,
                values: ns.into_iter().map(Value::I64).collect(),
            }),
        ]
        .boxed(),
        Produces::Id => prop_oneof![
            (0..ROWS, cmp_op()).prop_map(move |(n, op)| Expr::compare(at, op, Value::U64(n))),
            proptest::collection::vec(0..ROWS, 1..4).prop_map(move |ns| Expr::In {
                column: at,
                values: ns.into_iter().map(Value::U64).collect(),
            }),
        ]
        .boxed(),
        Produces::UpperKind => prop_oneof![
            (0..6u64).prop_map(move |k| Expr::eq(at, Value::Str(format!("KIND-{k}")))),
            (0..6u64, cmp_op()).prop_map(move |(k, op)| Expr::compare(
                at,
                op,
                Value::Str(format!("KIND-{k}"))
            )),
            (0..6u64).prop_map(move |k| Expr::like(at, format!("KIND-{k}%"))),
        ]
        .boxed(),
    }
}

/// A predicate over the table's columns *and* whatever this query computes.
fn filter_over(produces: &[Produces]) -> BoxedStrategy<Expr> {
    if produces.is_empty() {
        return any_filter().boxed();
    }
    // Weighted, not uniform, and the weights were chosen by measurement rather
    // than by eye. A covering scan of an expression index needs *every* leaf,
    // sort key and projected column to be one the entry holds, so the chance of
    // reaching one falls off as a power of the chance that any single leaf is a
    // table column. At equal weights the whole generator reached an index-only
    // scan of an expression index in 3% of cases; at four to one it reaches one
    // in about half, which is what
    // [`the_generated_queries_reach_a_covering_expression_scan`] holds it to.
    let mut options: Vec<(u32, BoxedStrategy<Expr>)> = vec![(1, leaf().boxed())];
    options.extend(
        produces
            .iter()
            .enumerate()
            .map(|(i, p)| (4, computed_leaf(computed(i), *p))),
    );
    proptest::strategy::Union::new_weighted(options)
        // Two deep rather than three: every extra leaf is another chance for a
        // table column to appear and take the covering scan away, and the
        // conjunction and disjunction handling this exercises is already
        // covered at depth three by `any_filter`.
        .prop_recursive(2, 6, 2, |inner| {
            prop_oneof![
                (inner.clone(), inner.clone()).prop_map(|(a, b)| a.and(b)),
                (inner.clone(), inner.clone()).prop_map(|(a, b)| Expr::Or(vec![a, b])),
                inner.prop_map(|a| Expr::Not(Box::new(a))),
            ]
        })
        .boxed()
}

/// Sort keys over the table's columns and this query's computed values.
fn sort_over(produces: &[Produces]) -> BoxedStrategy<Vec<SortKey>> {
    if produces.is_empty() {
        return any_sort().boxed();
    }
    let mut options: Vec<(u32, BoxedStrategy<SortKey>)> = vec![
        (1, Just(SortKey::asc(col("size"))).boxed()),
        (1, Just(SortKey::desc(col("score"))).boxed()),
        (1, Just(SortKey::asc(col("label"))).boxed()),
    ];
    for i in 0..produces.len() {
        // Weighted for the same reason as the filter's leaves: a sort key on a
        // column the entry does not hold is on its own enough to force the row
        // read, whatever the projection asked for.
        options.push((4, Just(SortKey::asc(computed(i))).boxed()));
        options.push((4, Just(SortKey::desc(computed(i))).boxed()));
    }
    proptest::collection::vec(proptest::strategy::Union::new_weighted(options), 0..2).boxed()
}

/// Projections, including the one that is a control rather than a case.
///
/// `[id, label]` reaches for the column an expression index keys *on* but does
/// not hold. No path may answer it from an entry, and if one ever did, the
/// answers part company here: the covering scan would return null where the
/// table scan returns the label.
fn projection_over(produces: &[Produces]) -> BoxedStrategy<Projection> {
    if produces.is_empty() {
        return any_projection().boxed();
    }
    let mut options: Vec<(u32, BoxedStrategy<Projection>)> = vec![
        (1, Just(Projection::All).boxed()),
        (3, Just(Projection::Columns(vec![ID])).boxed()),
        (2, Just(Projection::Columns(vec![ID, LABEL])).boxed()),
        (1, Just(Projection::Columns(vec![KIND])).boxed()),
        (2, Just(Projection::none()).boxed()),
    ];
    for i in 0..produces.len() {
        options.push((3, Just(Projection::Columns(vec![ID, computed(i)])).boxed()));
        options.push((
            2,
            Just(Projection::Columns(vec![LABEL, computed(i)])).boxed(),
        ));
    }
    proptest::strategy::Union::new_weighted(options).boxed()
}

/// A whole query. The primary key is appended to every sort so the order is
/// total: without it, `ORDER BY size LIMIT 5` has many correct answers and two
/// access paths may each return a different one, legitimately.
///
/// The program index comes back alongside, because the model the other two
/// properties compare against has to compute the same values without going
/// through the evaluator under test.
fn any_query() -> impl Strategy<Value = (usize, Query)> {
    (0..PROGRAMS).prop_flat_map(|which| {
        let (scalars, produces) = program(which);
        (
            Just(which),
            Just(scalars),
            filter_over(&produces),
            sort_over(&produces),
            projection_over(&produces),
            prop_oneof![Just(None), (0..12usize).prop_map(Some)],
            0..4usize,
            prop_oneof![Just(ScanOrder::Ascending), Just(ScanOrder::Descending)],
        )
            .prop_map(
                |(which, compute, filter, mut sort, projection, limit, offset, order)| {
                    sort.push(SortKey::asc(ID));
                    let mut query = Query::all().filter(filter).order(order).offset(offset);
                    query.sort = sort;
                    query.projection = projection;
                    query.limit = limit;
                    query.compute = compute;
                    (which, query)
                },
            )
    })
}

/// A compute list and a predicate over it, for the properties that need no
/// order.
fn any_computed_filter() -> impl Strategy<Value = (usize, Vec<Scalar>, Expr)> {
    (0..PROGRAMS).prop_flat_map(|which| {
        let (scalars, produces) = program(which);
        (Just(which), Just(scalars), filter_over(&produces))
    })
}

/// What each program computes, over a row, written out here rather than run
/// through [`Scalar::evaluate`].
///
/// The point of a model is to be a second opinion. Calling the evaluator under
/// test would make the expectation agree with the executor by construction,
/// which is how a test comes to assert nothing — the same mistake as sharing
/// the sort comparator, which `compare` below deliberately does not do.
fn model_computed(which: usize, row: &Row) -> Vec<Value> {
    let lower = |value: &Value| match value {
        Value::Str(s) => Value::Str(s.to_lowercase()),
        _ => Value::Null,
    };
    let upper = |value: &Value| match value {
        Value::Str(s) => Value::Str(s.to_uppercase()),
        _ => Value::Null,
    };
    let length = |value: &Value| match value {
        Value::Str(s) => Value::I64(s.chars().count() as i64),
        _ => Value::Null,
    };
    let label = &row.values()[LABEL.0];
    let kind = &row.values()[KIND.0];
    match which {
        0 => Vec::new(),
        1 => vec![lower(label)],
        2 => vec![length(label)],
        3 => vec![lower(label), length(label)],
        4 => vec![upper(kind)],
        5 => {
            let lowered = lower(label);
            let n = length(&lowered);
            vec![lowered, n]
        }
        6 => vec![row.values()[ID.0].clone(), lower(label)],
        other => panic!("no program {other}"),
    }
}

/// A row with its computed values appended, which is the shape everything
/// downstream of the executor sees.
fn extended(which: usize, row: &Row) -> Row {
    let mut values = row.values().to_vec();
    values.extend(model_computed(which, row));
    Row::new(values)
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

    proptest!(|((_which, query) in any_query())| {
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
                "index {:?} disagreed with a table scan for {:?} computing {:?} \
                 projecting {:?}",
                index, query.filter, query.compute, query.projection
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

    proptest!(|((which, compute, filter) in any_computed_filter())| {
        // The predicate may name a computed value, so the model has to compute
        // it too — from `model_computed`, which is written out rather than run
        // through the evaluator the executor uses.
        let expected: Vec<u64> = all
            .iter()
            .filter(|r| filter.admits(&extended(which, r)))
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
            query.compute = compute.clone();
            query.hint = hint;
            let got = ids_of(&rt.block_on(run(&store, &query)));
            prop_assert_eq!(
                &got, &expected,
                "{:?} computing {:?} with hint {:?} returned the wrong rows",
                filter, compute, hint
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

    proptest!(|((which, query) in any_query())| {
        // Projection is left out of this one: an unprojected column reads back
        // null, which would make the expectation a statement about projection
        // rather than about ordering.
        let mut query = query;
        query.projection = Projection::All;

        // Computed values are appended to the model row, so a sort key naming
        // one is compared the same way a column is.
        let all: Vec<Row> = all.iter().map(|r| extended(which, r)).collect();
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
            "ordering or paging differed for {:?} computing {:?} sort={:?} \
             limit={:?} offset={}",
            query.filter, query.compute, query.sort, query.limit, query.offset
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
/// Enough cases that the observed rate is stable to a couple of percent.
const SAMPLE: u32 = 400;

#[test]
fn the_generated_queries_select_a_range_of_row_counts() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let counts = std::cell::RefCell::new(Vec::new());

    proptest!(ProptestConfig::with_cases(SAMPLE), |((_which, compute, filter) in any_computed_filter())| {
        let mut query = Query::all().filter(filter);
        query.compute = compute;
        counts.borrow_mut().push(rt.block_on(run(&store, &query)).len());
    });

    let counts = counts.into_inner();
    let empty = counts.iter().filter(|n| **n == 0).count();
    let full = counts.iter().filter(|n| **n == ROWS as usize).count();
    let middling = counts.len() - empty - full;
    // Observed around 70%. The bar is 40%, which is more than five standard
    // errors away at this sample size — a generator-quality check that fires
    // one run in twenty is worse than no check, because it teaches people to
    // rerun the suite.
    assert!(
        middling * 5 > counts.len() * 2,
        "most generated predicates should select some but not all rows; \
         got {empty} empty, {full} full, {middling} in between of {}",
        counts.len()
    );
}

/// The new surface has to actually be reached.
///
/// Every property above is only as strong as what the generator produces, and
/// an index-only scan of an expression index is the narrowest thing it has to
/// reach: it needs the predicate, the sort keys *and* the projection all to
/// stay inside what the entry holds, so the chance of one falls off as a power
/// of the chance that any single leaf is a table column. The first version of
/// the weights reached one in 3% of cases, which would have left the whole
/// point of this extension resting on a handful of samples.
///
/// Both directions are counted, because both are load-bearing. A covering scan
/// is the case; a query reaching for the source column with the same index
/// available is the control, and if the planner ever called *that* one covering
/// the answers would part company in
/// [`every_access_path_returns_the_same_rows`].
#[test]
fn the_generated_queries_reach_a_covering_expression_scan() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let table = items();
    let tally = std::cell::RefCell::new((0usize, 0usize, 0usize));

    proptest!(ProptestConfig::with_cases(SAMPLE), |((_which, query) in any_query())| {
        let txn = rt.block_on(store.begin()).unwrap();
        let mut covering = false;
        let mut control = false;
        for index in [BY_LOWER_LABEL, BY_LABEL_LENGTH] {
            let mut forced = query.clone();
            forced.hint = Some(AccessHint::Index(index));
            let access = txn.explain(&root(), &table, &forced).unwrap().access.to_string();
            let name = table.index(index).unwrap().name().to_owned();
            if !access.contains(&name) {
                continue;
            }
            if access.contains("Index Only") {
                covering = true;
            } else if query.projection.columns().is_some_and(|c| c.contains(&LABEL)) {
                control = true;
            }
        }
        let mut tally = tally.borrow_mut();
        tally.0 += 1;
        tally.1 += usize::from(covering);
        tally.2 += usize::from(control);
    });

    let (cases, covering, control) = tally.into_inner();
    // Measured at 19% for each, across runs. The bars are 10%, which is four
    // and a half standard errors clear at this sample size — a check that
    // guards the generators must not be the flakiest thing in the suite.
    assert!(
        covering * 10 > cases,
        "only {covering} of {cases} generated queries reached an index-only \
         scan of an expression index"
    );
    assert!(
        control * 10 > cases,
        "only {control} of {cases} generated queries reached for the source \
         column with the expression index available"
    );
}
