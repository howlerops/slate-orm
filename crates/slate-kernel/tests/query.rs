//! Planning and execution.
//!
//! The central test here is [`bounds_never_lose_rows`]: for a spread of
//! predicate shapes, the planned query must return exactly what brute-force
//! filtering returns. Scan bounds are an optimisation, so a bound that is too
//! *wide* is only slow — but one that is too narrow silently drops rows, and
//! that is the failure this catches.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Access, Action, CmpOp, ColumnStats, Expr, Grant, Projection, RecordStore, ScanOrder,
    SecurityCatalog, SecurityContext, TableStats, memory::MemoryStore, plan, plan_with,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};

const METRICS: TableId = TableId(1);

fn metrics() -> TableDef {
    TableDef::builder("metrics", METRICS)
        .column("region", ValueType::Str)
        .column("bucket", ValueType::I64)
        .column("value", ValueType::F64)
        .nullable_column("label", ValueType::Str)
        .primary_key(["region", "bucket"])
        .index(IndexDef::builder("by_value", IndexId(10)).column("value"))
        .index(
            IndexDef::builder("by_label_desc", IndexId(11)).column_with("label", Direction::Desc),
        )
        .index(
            IndexDef::builder("by_region_value", IndexId(12))
                .column("region")
                .column_with("value", Direction::Desc),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    metrics().ordinal_of(name).expect("column exists")
}

fn row(region: &str, bucket: i64, value: f64, label: Option<&str>) -> Row {
    Row::new(vec![
        Value::Str(region.to_owned()),
        Value::I64(bucket),
        Value::F64(value),
        label.map_or(Value::Null, |l| Value::Str(l.to_owned())),
    ])
}

fn corpus() -> Vec<Row> {
    let mut rows = Vec::new();
    for (i, region) in ["ap", "eu", "us"].into_iter().enumerate() {
        for bucket in [-5i64, -1, 0, 1, 7, 100] {
            let value = (bucket as f64) * 1.5 - (i as f64);
            let label = match bucket.rem_euclid(3) {
                0 => None,
                1 => Some("alpha"),
                _ => Some("beta"),
            };
            rows.push(row(region, bucket, value, label));
        }
    }
    rows
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([metrics()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("all", METRICS, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);

    let table = metrics();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for row in corpus() {
        txn.insert(&root, &table, &row).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

/// Identify a row by its primary key, for order-insensitive comparison.
fn identity(row: &Row) -> (String, i64) {
    match (&row.values()[0], &row.values()[1]) {
        (Value::Str(r), Value::I64(b)) => (r.clone(), *b),
        other => panic!("unexpected key {other:?}"),
    }
}

async fn run(
    store: &RecordStore<MemoryStore>,
    filter: &Expr,
    order: ScanOrder,
) -> Vec<(String, i64)> {
    let table = metrics();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&SecurityContext::superuser(), &table, filter.clone(), order)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    rows.iter().map(identity).collect()
}

/// What the predicate selects, computed without any index or bound.
fn brute_force(filter: &Expr) -> Vec<(String, i64)> {
    corpus()
        .iter()
        .filter(|r| filter.admits(r))
        .map(identity)
        .collect()
}

fn predicates() -> Vec<(&'static str, Expr)> {
    let region = col("region");
    let bucket = col("bucket");
    let value = col("value");
    let label = col("label");
    let eu = || Value::Str("eu".into());

    let mut cases: Vec<(&'static str, Expr)> = vec![
        ("everything", Expr::True),
        ("nothing", Expr::False),
        ("pk prefix equality", Expr::eq(region, eu())),
        (
            "full pk equality",
            Expr::eq(region, eu()).and(Expr::eq(bucket, Value::I64(7))),
        ),
        (
            "pk prefix plus range",
            Expr::eq(region, eu()).and(Expr::compare(bucket, CmpOp::Gt, Value::I64(0))),
        ),
        (
            "pk prefix plus two-sided range",
            Expr::eq(region, eu())
                .and(Expr::compare(bucket, CmpOp::Ge, Value::I64(-1)))
                .and(Expr::compare(bucket, CmpOp::Lt, Value::I64(100))),
        ),
        ("range on pk prefix", Expr::compare(region, CmpOp::Gt, eu())),
        ("indexed equality", Expr::eq(value, Value::F64(1.5))),
        ("label is null", Expr::is_null(label)),
        ("label is not null", Expr::is_not_null(label)),
        (
            "equality against null selects nothing",
            Expr::eq(label, Value::Null),
        ),
        ("not equal", Expr::compare(region, CmpOp::Ne, eu())),
        (
            "in list",
            Expr::In {
                column: region,
                values: vec![eu(), Value::Str("us".into())],
            },
        ),
        (
            "disjunction",
            Expr::Or(vec![
                Expr::eq(region, eu()),
                Expr::eq(bucket, Value::I64(100)),
            ]),
        ),
        ("negation", Expr::Not(Box::new(Expr::eq(region, eu())))),
        (
            "descending index column range",
            Expr::compare(label, CmpOp::Ge, Value::Str("b".into())),
        ),
        (
            "composite index, equality then descending range",
            Expr::eq(region, eu()).and(Expr::compare(value, CmpOp::Le, Value::F64(1.5))),
        ),
        (
            "unsatisfiable conjunction",
            Expr::eq(region, eu()).and(Expr::eq(region, Value::Str("us".into()))),
        ),
    ];

    // Every comparison operator against several boundary values, on both an
    // ascending key column and a descending index column.
    for op in [
        CmpOp::Lt,
        CmpOp::Le,
        CmpOp::Gt,
        CmpOp::Ge,
        CmpOp::Eq,
        CmpOp::Ne,
    ] {
        for probe in [-5i64, -1, 0, 1, 6, 7, 100, 1000] {
            cases.push((
                "bucket comparison",
                Expr::compare(bucket, op, Value::I64(probe)),
            ));
            cases.push((
                "bucket comparison within a region",
                Expr::eq(region, eu()).and(Expr::compare(bucket, op, Value::I64(probe))),
            ));
        }
        for probe in [-10.0f64, -1.5, 0.0, 1.5, 148.5, 1000.0] {
            cases.push((
                "value comparison on an ascending index",
                Expr::compare(value, op, Value::F64(probe)),
            ));
        }
        for probe in ["a", "alpha", "b", "beta", "z"] {
            cases.push((
                "label comparison on a descending index",
                Expr::compare(label, op, Value::Str(probe.to_owned())),
            ));
        }
    }
    cases
}

/// Whatever access path the planner picks, the answer must match brute force.
#[tokio::test]
async fn bounds_never_lose_rows() {
    let store = seeded().await;
    for (name, filter) in predicates() {
        for order in [ScanOrder::Ascending, ScanOrder::Descending] {
            let mut got = run(&store, &filter, order).await;
            let mut want = brute_force(&filter);
            got.sort();
            want.sort();
            assert_eq!(
                got,
                want,
                "predicate `{name}` ({filter:?}) under {order:?} returned the wrong rows; \
                 plan was {:?}",
                plan(&metrics(), &filter, order).access
            );
        }
    }
}

/// The equivalence test above would still pass if every plan were a table scan,
/// so check separately that the planner reaches for an index when an index is
/// actually the cheaper thing.
///
/// These assertions are cost-based, which means they depend on the statistics.
/// That is the point: the same predicate should get a different plan on a
/// thousand rows than on a million, and a planner that always answered the same
/// way would be the bug.
#[tokio::test]
async fn the_planner_uses_the_indexes_it_should() {
    let table = metrics();
    let region = col("region");
    let bucket = col("bucket");
    let value = col("value");
    let label = col("label");

    let by_value = table.index_by_name("by_value").unwrap().id();
    let by_label = table.index_by_name("by_label_desc").unwrap().id();
    let by_region_value = table.index_by_name("by_region_value").unwrap().id();

    // A large table where an equality is highly selective: an index scan reads
    // a handful of rows, a table scan reads a million.
    let large = TableStats::with_row_count(1_000_000).with_column(
        value,
        ColumnStats {
            distinct: 100_000,
            null_fraction: 0.0,
        },
    );
    // A small table where the same column is *not* selective: forty-five point
    // reads against two hundred scanned rows is a bad trade.
    let small = TableStats::with_row_count(200).with_column(
        value,
        ColumnStats {
            distinct: 4,
            null_fraction: 0.0,
        },
    );

    let equality_on_value = Expr::eq(value, Value::F64(1.5));
    match plan_with(
        &table,
        &equality_on_value,
        ScanOrder::Ascending,
        &Projection::All,
        &large,
        None,
    )
    .access
    {
        Access::IndexScan { index, .. } => assert_eq!(index, by_value),
        other => panic!("a selective equality on a big table should use the index, got {other:?}"),
    }

    // The same predicate where it selects a quarter of a small table.
    assert!(
        matches!(
            plan_with(
                &table,
                &equality_on_value,
                ScanOrder::Ascending,
                &Projection::All,
                &small,
                None,
            )
            .access,
            Access::TableScan { .. }
        ),
        "an unselective equality should lose to a scan"
    );

    // Unless the index covers the query, in which case there are no point reads
    // to weigh at all.
    match plan_with(
        &table,
        &equality_on_value,
        ScanOrder::Ascending,
        &Projection::Columns(vec![region, bucket, value]),
        &small,
        None,
    )
    .access
    {
        Access::IndexScan {
            index, covering, ..
        } => {
            assert_eq!(index, by_value);
            assert!(covering);
        }
        other => panic!("a covering index has no lookups to pay for, got {other:?}"),
    }

    // `IS NULL` is a prefix like any other value, so an index can serve it —
    // given a table big enough and nulls rare enough for it to be worth doing.
    let rare_nulls = TableStats::with_row_count(1_000_000).with_column(
        label,
        ColumnStats {
            distinct: 1_000,
            null_fraction: 0.000_01,
        },
    );
    match plan_with(
        &table,
        &Expr::is_null(label),
        ScanOrder::Ascending,
        &Projection::All,
        &rare_nulls,
        None,
    )
    .access
    {
        Access::IndexScan { index, .. } => assert_eq!(index, by_label),
        other => panic!("a rare null should be found through the index, got {other:?}"),
    }

    // A composite index whose leading column is pinned and whose second is
    // ranged beats the one-column key prefix.
    match plan_with(
        &table,
        &Expr::eq(region, Value::Str("eu".into())).and(Expr::compare(
            value,
            CmpOp::Lt,
            Value::F64(3.0),
        )),
        ScanOrder::Ascending,
        &Projection::Columns(vec![region, value, bucket]),
        &large,
        None,
    )
    .access
    {
        Access::IndexScan { index, .. } => assert_eq!(index, by_region_value),
        other => panic!("expected the composite index, got {other:?}"),
    }

    // A full key match is a point read whatever the statistics say.
    for stats in [&large, &small] {
        assert!(matches!(
            plan_with(
                &table,
                &Expr::eq(region, Value::Str("eu".into())).and(Expr::eq(bucket, Value::I64(7))),
                ScanOrder::Ascending,
                &Projection::All,
                stats,
                None,
            )
            .access,
            Access::PointGet { .. }
        ));
    }

    // An impossible predicate reads nothing.
    assert!(matches!(
        plan(&table, &Expr::eq(label, Value::Null), ScanOrder::Ascending).access,
        Access::Nothing
    ));
}

/// A limit lowers the cost of every plan, but — with this cost model — it does
/// not change which plan wins.
///
/// That is not an oversight, it falls out of the arithmetic: reading `L` rows
/// through an index costs `L` point reads, and finding `L` matches by scanning
/// costs `L / selectivity` rows, so both scale linearly in `L` and their ratio
/// is whatever it was without the limit. The case where a limit genuinely flips
/// the decision is `ORDER BY` with a limit, where an index supplies the order
/// and a scan would have to sort everything first — which is a reason to want
/// ordered plans, not a reason to weight limits.
#[tokio::test]
async fn a_limit_lowers_the_estimate_without_changing_the_choice() {
    let table = metrics();
    let value = col("value");
    // A thousand rows match, so a limit of ten leaves most of them unread.
    let stats = TableStats::with_row_count(1_000_000).with_column(
        value,
        ColumnStats {
            distinct: 1_000,
            null_fraction: 0.0,
        },
    );
    let filter = Expr::eq(value, Value::F64(1.5));

    let unlimited = plan_with(
        &table,
        &filter,
        ScanOrder::Ascending,
        &Projection::All,
        &stats,
        None,
    );
    let limited = plan_with(
        &table,
        &filter,
        ScanOrder::Ascending,
        &Projection::All,
        &stats,
        Some(10),
    );

    assert!(matches!(unlimited.access, Access::IndexScan { .. }));
    assert!(matches!(limited.access, Access::IndexScan { .. }));
    assert!(
        limited.estimated_cost < unlimited.estimated_cost,
        "a limit should make the plan cheaper: {} vs {}",
        limited.estimated_cost,
        unlimited.estimated_cost
    );
    assert!(limited.estimated_rows <= 10.0);
}

#[tokio::test]
async fn scanning_backwards_reverses_the_rows() {
    let store = seeded().await;
    let filter = Expr::eq(col("region"), Value::Str("eu".into()));
    let forwards = run(&store, &filter, ScanOrder::Ascending).await;
    let backwards = run(&store, &filter, ScanOrder::Descending).await;

    assert!(!forwards.is_empty());
    let mut reversed = backwards.clone();
    reversed.reverse();
    assert_eq!(forwards, reversed);
}

#[tokio::test]
async fn a_limit_stops_early() {
    let store = seeded().await;
    let table = metrics();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(
            &SecurityContext::superuser(),
            &table,
            Expr::True,
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .limit(4)
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);
}

#[tokio::test]
async fn counting_matches_collecting() {
    let store = seeded().await;
    let table = metrics();
    let filter = Expr::compare(col("bucket"), CmpOp::Ge, Value::I64(0));

    let txn = store.begin().await.unwrap();
    let counted = txn
        .query(
            &SecurityContext::superuser(),
            &table,
            filter.clone(),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .count()
        .await
        .unwrap();
    assert_eq!(counted, brute_force(&filter).len());
}

/// Comparing two columns of the same row. It exists for a join's cross-side
/// condition, but it is an ordinary single-table predicate too, and this is
/// where its semantics are pinned down.
#[tokio::test]
async fn a_comparison_between_two_columns_is_a_residual_filter() {
    let store = seeded().await;
    let table = metrics();
    let txn = store.begin().await.unwrap();

    // Two string columns, both varying per row.
    let filter = Expr::compare_columns(col("label"), CmpOp::Lt, col("region"));
    let rows = txn
        .query(
            &SecurityContext::superuser(),
            &table,
            filter.clone(),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let expected: Vec<Row> = corpus()
        .into_iter()
        .filter(|r| match (r.get(col("label")), r.get(col("region"))) {
            (Some(Value::Str(l)), Some(Value::Str(g))) => l < g,
            _ => false,
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "the corpus must exercise both outcomes"
    );
    assert!(
        expected.len() < corpus().len(),
        "and must not be all of it either"
    );
    assert_eq!(rows.len(), expected.len());

    // It can never be a scan bound — there is no literal to bound on — so the
    // planner must fall back to a scan rather than deriving something wrong.
    let plan = plan(&table, &filter, ScanOrder::Ascending);
    assert!(
        matches!(plan.access, Access::TableScan { .. }),
        "got {:?}",
        plan.access
    );
    // And both columns must be listed, or an index-only scan could skip one.
    assert!(plan.predicate_columns.contains(col("label")));
    assert!(plan.predicate_columns.contains(col("region")));
}

/// `Value`'s order is type-first, which is what makes the key encoding
/// sortable. So comparing an integer column with a float one would compare the
/// *types* and give the same answer for every row — a bug that returns a
/// plausible number of rows. It is refused rather than answered.
#[tokio::test]
async fn comparing_columns_of_different_types_is_refused() {
    let store = seeded().await;
    let table = metrics();
    let txn = store.begin().await.unwrap();

    let err = txn
        .query(
            &SecurityContext::superuser(),
            &table,
            // bucket is I64, value is F64.
            Expr::compare_columns(col("bucket"), CmpOp::Lt, col("value")),
            ScanOrder::Ascending,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            slate_kernel::KernelError::ComparisonTypeMismatch { .. }
        ),
        "got {err:?}"
    );

    // Left to itself the comparison would have admitted every row, which is
    // why silence here would be worse than an error.
    let all = corpus().len();
    let unchecked = Expr::compare_columns(col("bucket"), CmpOp::Lt, col("value"));
    let admitted = corpus().iter().filter(|r| unchecked.admits(r)).count();
    assert_eq!(
        admitted, all,
        "the type-first order admits everything, as the refusal assumes"
    );
}

/// A null on either side makes it unknown, not false — the same rule as a
/// comparison with a literal, and the same reason: a deny-style policy written
/// with `NOT` must not admit the rows where the answer is not known.
#[tokio::test]
async fn a_null_makes_a_column_comparison_unknown() {
    let with_null = row("eu", 1, 2.0, None);
    let filter = Expr::compare_columns(col("label"), CmpOp::Lt, col("region"));

    assert_eq!(filter.evaluate(&with_null), slate_kernel::Truth::Unknown);
    assert!(!filter.admits(&with_null));
    // And its negation is unknown too, so neither admits the row.
    assert!(!Expr::Not(Box::new(filter)).admits(&with_null));
}
