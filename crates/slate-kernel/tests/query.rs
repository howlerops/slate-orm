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
    Access, Action, CmpOp, Expr, Grant, RecordStore, ScanOrder, SecurityCatalog, SecurityContext,
    memory::MemoryStore, plan,
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
/// so check separately that the planner really is choosing index paths.
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

    let cases: Vec<(&str, Expr, Access)> = vec![
        (
            "equality on an indexed column uses that index",
            Expr::eq(value, Value::F64(1.5)),
            Access::IndexScan {
                index: by_value,
                range: match_any(),
                covering: false,
            },
        ),
        (
            "IS NULL is a prefix like any other value",
            Expr::is_null(label),
            Access::IndexScan {
                index: by_label,
                range: match_any(),
                covering: false,
            },
        ),
        (
            "a key prefix keeps the query on the table",
            Expr::eq(region, Value::Str("eu".into())),
            Access::TableScan { range: match_any() },
        ),
        (
            "a composite index beats a one-column key prefix",
            Expr::eq(region, Value::Str("eu".into())).and(Expr::compare(
                value,
                CmpOp::Lt,
                Value::F64(3.0),
            )),
            Access::IndexScan {
                index: by_region_value,
                range: match_any(),
                covering: false,
            },
        ),
        (
            "a full key match is a point read, not a one-row scan",
            Expr::eq(region, Value::Str("eu".into())).and(Expr::eq(bucket, Value::I64(7))),
            Access::PointGet { key: Vec::new() },
        ),
        (
            "an impossible predicate reads nothing",
            Expr::eq(label, Value::Null),
            Access::Nothing,
        ),
    ];

    for (why, filter, expected) in cases {
        let access = plan(&table, &filter, ScanOrder::Ascending).access;
        let ok = match (&expected, &access) {
            (Access::Nothing, Access::Nothing) => true,
            (Access::PointGet { .. }, Access::PointGet { .. }) => true,
            (Access::TableScan { .. }, Access::TableScan { .. }) => true,
            (Access::IndexScan { index: a, .. }, Access::IndexScan { index: b, .. }) => a == b,
            _ => false,
        };
        assert!(ok, "{why}: expected {expected:?}, got {access:?}");
    }
}

/// A placeholder range for cases where only the access *kind* is asserted.
fn match_any() -> slate_kernel::KeyRange {
    slate_kernel::KeyRange::all()
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
