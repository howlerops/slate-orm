//! Range estimates, and the plans that turn on them.
//!
//! Without a histogram a range gets a fixed guess, so the planner cannot tell
//! `at < 10` from `at < 500` and costs both the same. On the benchmark corpus
//! that meant scanning 2500 rows in 58 ms to return the ten an index finds in
//! 4.5 ms.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    AccessSummary, Action, CmpOp, Expr, Grant, Histogram, Query, RecordStore, ScanOrder,
    SecurityCatalog, SecurityContext, Statistics, TableStats, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const READINGS: TableId = TableId(1);
const ROWS: u64 = 2_000;

fn readings() -> TableDef {
    TableDef::builder("readings", READINGS)
        .column("id", ValueType::U64)
        // Uniform over 0..ROWS, so the true selectivity of a range is exactly
        // known and the estimate can be held to it.
        .column("at", ValueType::I64)
        // Heavily skewed: nine tenths of the rows share one value. A fixed
        // guess is wrong here in the other direction.
        .column("bucket", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_at", IndexId(10)).column("at"))
        .index(IndexDef::builder("by_bucket", IndexId(11)).column("bucket"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    readings().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::I64(id as i64),
        Value::I64(if id.is_multiple_of(10) { id as i64 } else { 0 }),
        if id.is_multiple_of(4) {
            Value::Null
        } else {
            Value::Str("note".to_owned())
        },
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn seeded() -> (RecordStore<MemoryStore>, TableStats) {
    let catalog = Catalog::from_tables([readings()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", READINGS, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);

    let txn = store.begin().await.unwrap();
    let rows: Vec<Row> = (0..ROWS).map(row).collect();
    txn.insert_many(&root(), &readings(), &rows).await.unwrap();
    txn.commit().await.unwrap();

    let analyzed = {
        let txn = store.begin().await.unwrap();
        txn.analyze(&root(), &readings()).await.unwrap()
    };
    let store = store.with_statistics(Statistics::new().with(READINGS, analyzed.clone()));
    (store, analyzed)
}

/// `analyze` measures the distribution, and the estimate it produces has to be
/// close to the truth or it is just a different guess.
#[tokio::test]
async fn a_range_estimate_tracks_the_data() {
    let (_, stats) = seeded().await;
    assert!(
        stats.histogram(col("at")).is_some(),
        "a uniform column should get a histogram"
    );

    // `at` is 0..2000, uniform, no nulls: the fraction below v is v / 2000.
    for cut in [10i64, 100, 500, 1000, 1900] {
        let estimated = stats.bounded_selectivity(col("at"), &[(CmpOp::Lt, &Value::I64(cut))]);
        let truth = cut as f64 / ROWS as f64;
        assert!(
            (estimated - truth).abs() < 0.05,
            "at < {cut}: estimated {estimated:.3}, true {truth:.3}"
        );
    }

    // And an interval is the difference of its ends, not a fresh guess.
    let interval = stats.bounded_selectivity(
        col("at"),
        &[(CmpOp::Ge, &Value::I64(400)), (CmpOp::Lt, &Value::I64(600))],
    );
    assert!(
        (interval - 0.1).abs() < 0.05,
        "a fifth of a fifth of the table, got {interval:.3}"
    );
}

/// The old fixed guess said a third, whatever was asked. That is what made the
/// planner unable to tell a selective range from a broad one.
#[tokio::test]
async fn the_estimate_distinguishes_narrow_from_broad() {
    let (_, stats) = seeded().await;
    let narrow = stats.bounded_selectivity(col("at"), &[(CmpOp::Lt, &Value::I64(10))]);
    let broad = stats.bounded_selectivity(col("at"), &[(CmpOp::Lt, &Value::I64(1900))]);
    assert!(
        narrow * 10.0 < broad,
        "narrow {narrow:.3} and broad {broad:.3} are barely different"
    );
    // The fallback, for a column with no histogram, cannot tell them apart at
    // all — which is exactly the failure the histogram exists to fix.
    let bare = TableStats::with_row_count(ROWS);
    let bare_narrow = bare.bounded_selectivity(col("at"), &[(CmpOp::Lt, &Value::I64(10))]);
    let bare_broad = bare.bounded_selectivity(col("at"), &[(CmpOp::Lt, &Value::I64(1900))]);
    assert_eq!(
        bare_narrow, bare_broad,
        "with no histogram the value should not matter, and it does not"
    );
}

/// Equi-depth buckets put their resolution where the rows are, so a column
/// where nine tenths of the values are identical is still described usefully.
#[tokio::test]
async fn a_skewed_column_is_described_by_where_its_rows_are() {
    let (_, stats) = seeded().await;
    // Nine tenths of `bucket` is zero, so almost nothing is below 1.
    let below_one = stats.bounded_selectivity(col("bucket"), &[(CmpOp::Lt, &Value::I64(1))]);
    assert!(
        below_one > 0.7,
        "most of the table is at zero, got {below_one:.3}"
    );
    let above = stats.bounded_selectivity(col("bucket"), &[(CmpOp::Gt, &Value::I64(1))]);
    assert!(above < 0.3, "the tail is a tenth, got {above:.3}");
}

/// The plan that follows from the estimate. This is the whole point: the
/// selective range should reach for the index and the broad one should not.
#[tokio::test]
async fn a_selective_range_uses_the_index_and_a_broad_one_does_not() {
    let (store, _) = seeded().await;
    let table = readings();
    let txn = store.begin().await.unwrap();

    let narrow = txn
        .explain(
            &root(),
            &table,
            &Query::all().filter(Expr::compare(col("at"), CmpOp::Lt, Value::I64(10))),
        )
        .unwrap();
    assert!(
        matches!(narrow.access, AccessSummary::IndexScan { .. }),
        "0.5% of the table should go through the index, got {narrow}"
    );

    let broad = txn
        .explain(
            &root(),
            &table,
            &Query::all().filter(Expr::compare(col("at"), CmpOp::Lt, Value::I64(1600))),
        )
        .unwrap();
    assert!(
        matches!(broad.access, AccessSummary::TableScan),
        "80% of the table should be scanned, got {broad}"
    );

    // And the estimates differ by roughly the ratio of the two ranges.
    assert!(
        narrow.estimated_rows * 20.0 < broad.estimated_rows,
        "{} vs {}",
        narrow.estimated_rows,
        broad.estimated_rows
    );
}

/// Whichever plan the estimate picks, the rows have to be the same. An
/// estimate can be wrong and cost time; it must never change the answer.
#[tokio::test]
async fn the_estimate_never_changes_the_answer() {
    let (store, _) = seeded().await;
    let table = readings();
    let txn = store.begin().await.unwrap();

    for cut in [1i64, 10, 250, 1000, 1999, 5000] {
        for op in [CmpOp::Lt, CmpOp::Le, CmpOp::Gt, CmpOp::Ge] {
            let filter = Expr::compare(col("at"), op, Value::I64(cut));
            let planned = txn
                .query(&root(), &table, filter.clone(), ScanOrder::Ascending)
                .await
                .unwrap()
                .count()
                .await
                .unwrap();
            let scanned = txn
                .execute(
                    &root(),
                    &table,
                    &Query::all().filter(filter.clone()).using_table_scan(),
                )
                .await
                .unwrap()
                .count()
                .await
                .unwrap();
            assert_eq!(planned, scanned, "disagreed on {op:?} {cut}");
        }
    }
}

/// Too few values, or all the same value, is not a distribution. Claiming one
/// would be worse than the guess it replaces.
#[test]
fn a_histogram_needs_something_to_describe() {
    assert!(Histogram::from_values(Vec::new()).is_none());
    assert!(Histogram::from_values(vec![Value::I64(1); 8]).is_none());
    assert!(
        Histogram::from_values(vec![Value::I64(7); 500]).is_none(),
        "one distinct value is not a spread"
    );
    assert!(
        Histogram::from_values((0..500).map(Value::I64).collect()).is_some(),
        "five hundred distinct values is"
    );
    // Nulls are not part of the distribution; they are counted separately.
    let mixed: Vec<Value> = (0..500)
        .map(|i| {
            if i % 2 == 0 {
                Value::Null
            } else {
                Value::I64(i)
            }
        })
        .collect();
    let histogram = Histogram::from_values(mixed).expect("enough non-null values");
    assert!(histogram.bounds().iter().all(|v| !v.is_null()));
}

/// The boundaries and the ends behave. A value below everything is nothing;
/// above everything is all of it.
#[test]
fn the_ends_of_a_histogram_are_the_ends_of_the_data() {
    let histogram =
        Histogram::from_values((0..1000).map(Value::I64).collect()).expect("a histogram");
    assert_eq!(histogram.fraction_below(&Value::I64(-1)), 0.0);
    assert_eq!(histogram.fraction_below(&Value::I64(10_000)), 1.0);
    let middle = histogram.fraction_below(&Value::I64(500));
    assert!((middle - 0.5).abs() < 0.05, "got {middle}");
    // Monotonic, or a wider range could estimate fewer rows than a narrower.
    let mut last = 0.0;
    for cut in (0..1000).step_by(50) {
        let at = histogram.fraction_below(&Value::I64(cut));
        assert!(at >= last, "went backwards at {cut}: {at} after {last}");
        last = at;
    }
}

/// `analyze` twice on the same data gives the same statistics. A planner whose
/// choices move between runs is one nobody can reason about.
#[tokio::test]
async fn analyze_is_reproducible() {
    let (store, first) = seeded().await;
    let txn = store.begin().await.unwrap();
    let second = txn.analyze(&root(), &readings()).await.unwrap();
    assert_eq!(
        first.histogram(col("at")).map(Histogram::bounds),
        second.histogram(col("at")).map(Histogram::bounds),
    );
    assert_eq!(first.row_count, second.row_count);
}
