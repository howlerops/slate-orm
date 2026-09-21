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
    AccessSummary, Action, CmpOp, Expr, Grant, Histogram, Query, RecordStore, Scalar, ScanOrder,
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

    // And the estimates differ by roughly the ratio of the two ranges, which is
    // the whole point of a histogram: without one both would come back as the
    // same fixed guess.
    assert!(
        narrow.estimated_rows * 20.0 < broad.estimated_rows,
        "{} vs {}",
        narrow.estimated_rows,
        broad.estimated_rows
    );

    // The narrow range no longer goes through the index, and that is correct
    // rather than a regression. On object storage a scan returns about eight
    // thousand rows per request and a point read costs one, so an index only
    // pays off when it saves scanning some eight thousand rows *per row
    // fetched*. Against a two-thousand-row table nothing clears that bar — and
    // that conclusion survived `POINT_READ_COST` moving from 3.0 to 1.0 in
    // #269, which is what turned twenty-four thousand into eight.
    //
    // The consequence is worth stating because it is counter-intuitive:
    // usefulness of an index here is about the *absolute* number of rows
    // fetched, not the percentage. Half a percent of a table is never selective
    // enough by itself, at any table size — fetching `k` rows beats scanning
    // `n` only when `n > 8000k`, and `k = 0.005n` never satisfies that.
    assert!(
        matches!(narrow.access, AccessSummary::TableScan),
        "a two-thousand-row table is cheaper to scan whole, got {narrow}"
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

// --- statistics for a computed value ---------------------------------------
//
// An expression index keys on a value no column holds, so nothing `analyze`
// measured described it and the planner fell back to the hundred distinct
// values an unseen column gets. That is not a small error in one place: it is
// the *only* number behind every decision about such an index.
//
// The tests below are oracles rather than expectations wherever they can be.
// What `analyze` records is checked against the same expression computed by
// hand over the same rows, and what the histogram says is checked against
// counting the rows. A statistic that agrees with a hardcoded number agrees
// with whoever wrote it down.

const EVENTS: TableId = TableId(2);
const BY_LOWER_TAG: IndexId = IndexId(20);
const BY_TAG_LENGTH: IndexId = IndexId(21);
/// `id`, `tag`. By position, because `events()` names an expression over them.
const EVENT_ID: Ordinal = Ordinal(0);
const TAG: Ordinal = Ordinal(1);

/// Fifty distinct lowered tags, spelled three ways each, so `lower(tag)` has a
/// third of the distinct values `tag` has. That gap is the oracle: statistics
/// that merely copied the column's would come back with a hundred and fifty.
const TAGS: u64 = 50;

fn lower_tag() -> Scalar {
    Scalar::Lower(Box::new(Scalar::Column(TAG)))
}

fn tag_length() -> Scalar {
    Scalar::Length(Box::new(Scalar::Column(TAG)))
}

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("id", ValueType::U64)
        .nullable_column("tag", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_lower_tag", BY_LOWER_TAG).expression(lower_tag(), ValueType::Str),
        )
        // A second one, so the statistics of two expression indexes on one
        // table have to be told apart. `analyze` counts them in slots past the
        // table's own columns, and an off-by-one there would describe one
        // index with the other's numbers — which nothing with a single index
        // could ever notice.
        .index(
            IndexDef::builder("by_tag_length", BY_TAG_LENGTH)
                .expression(tag_length(), ValueType::I64),
        )
        .build()
        .expect("valid schema")
}

#[test]
fn the_event_ordinals_are_where_they_are_claimed_to_be() {
    let table = events();
    assert_eq!(EVENT_ID, table.ordinal_of("id").expect("id"));
    assert_eq!(TAG, table.ordinal_of("tag").expect("tag"));
}

/// One row. Zero-padded so the lexicographic order the index stores is the
/// numeric one the test reasons about, and every tenth tag null so a null
/// fraction is a real number rather than zero.
fn event(id: u64) -> Row {
    let bucket = id % TAGS;
    let tag = if bucket.is_multiple_of(10) {
        Value::Null
    } else {
        // Three spellings of the same lowered value.
        Value::Str(match id % 3 {
            0 => format!("Tag-{bucket:02}"),
            1 => format!("tag-{bucket:02}"),
            _ => format!("TAG-{bucket:02}"),
        })
    };
    Row::new(vec![Value::U64(id), tag])
}

fn event_rows() -> Vec<Row> {
    (0..ROWS).map(event).collect()
}

/// `lower(tag)` for every row, computed here rather than read out of the
/// index — the oracle the recorded statistics are held to.
fn lowered_tags() -> Vec<Value> {
    event_rows()
        .iter()
        .map(|row| lower_tag().evaluate(row.values()))
        .collect()
}

/// A store over `kv` planning with `stats`.
///
/// Rebuilt rather than cloned because a `RecordStore` is not `Clone`, and the
/// tests below need the same rows read through two different sets of
/// statistics — which is the whole comparison.
fn events_store(kv: &MemoryStore, stats: TableStats) -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([events()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", EVENTS, Action::ALL));
    RecordStore::new(kv.clone(), catalog, security)
        .with_statistics(Statistics::new().with(EVENTS, stats))
}

/// The rows loaded, and what `analyze` makes of them.
async fn analysed_events() -> (MemoryStore, TableStats) {
    let kv = MemoryStore::new();
    let loader = events_store(&kv, TableStats::assumed());
    let txn = loader.begin().await.unwrap();
    txn.insert_many(&root(), &events(), &event_rows())
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = loader.begin().await.unwrap();
    let stats = txn.analyze(&root(), &events()).await.unwrap();
    (kv, stats)
}

/// How many rows a query really returns, which is what an estimate is judged
/// against.
async fn count_through(store: &RecordStore<MemoryStore>, query: Query) -> usize {
    let txn = store.begin().await.unwrap();
    txn.execute(&root(), &events(), &query)
        .await
        .unwrap()
        .count()
        .await
        .unwrap()
}

/// `analyze` evaluates the expression, and what it records matches computing
/// the expression by hand over the same rows.
#[tokio::test]
async fn analyze_measures_what_an_expression_index_keys_on() {
    let (_kv, stats) = analysed_events().await;
    let measured = stats
        .expression(BY_LOWER_TAG)
        .expect("the expression index was analysed");

    let values = lowered_tags();
    let nulls = values.iter().filter(|v| v.is_null()).count();
    let distinct: std::collections::BTreeSet<&Value> =
        values.iter().filter(|v| !v.is_null()).collect();

    assert_eq!(
        measured.distinct,
        distinct.len() as u64,
        "distinct computed values"
    );
    assert!(
        (measured.null_fraction - nulls as f64 / ROWS as f64).abs() < 1e-9,
        "null fraction {} against {}",
        measured.null_fraction,
        nulls as f64 / ROWS as f64
    );

    // And it is not the source column's statistics under another name, which
    // is what a version of this that never ran the expression would record.
    // Three spellings per tag, so the column has three times the values.
    assert_eq!(stats.column(TAG).distinct, measured.distinct * 3);
}

/// The histogram describes the computed values, checked against counting them.
///
/// Bucket resolution is about 1.5%, and the sample is the whole table here, so
/// the bar is tight enough to fail on a histogram built from the wrong values
/// and loose enough not to fail on the bucket midpoint.
#[tokio::test]
async fn an_expression_histogram_agrees_with_counting_the_rows() {
    let (_kv, stats) = analysed_events().await;
    let histogram = stats
        .expression_histogram(BY_LOWER_TAG)
        .expect("enough distinct computed values for a histogram");

    let values: Vec<Value> = lowered_tags()
        .into_iter()
        .filter(|v| !v.is_null())
        .collect();
    for cut in [1u64, 5, 10, 25, 40, 49] {
        let cut = Value::Str(format!("tag-{cut:02}"));
        let below = values.iter().filter(|v| **v < cut).count() as f64 / values.len() as f64;
        let estimated = histogram.fraction_below(&cut);
        assert!(
            (estimated - below).abs() < 0.03,
            "below {cut:?}: estimated {estimated:.3}, counted {below:.3}"
        );
    }
}

/// A table with no expression index gets no expression statistics, and one
/// that was never analysed reports none rather than a default dressed up as a
/// measurement.
#[tokio::test]
async fn an_unanalysed_expression_has_no_statistics() {
    let bare = TableStats::with_row_count(ROWS);
    assert!(bare.expression(BY_LOWER_TAG).is_none());
    assert!(bare.expression_histogram(BY_LOWER_TAG).is_none());

    // And the ones that are recorded do not leak into the columns: `analyze`
    // appends the expressions after the table's own columns while it counts,
    // and an off-by-one there would write a computed value's statistics at a
    // column's ordinal.
    let (_kv, stats) = analysed_events().await;
    let table = events();
    assert!(
        stats.histogram(Ordinal(table.columns().len())).is_none(),
        "nothing should be recorded past the table's own columns"
    );
}

/// The planner's estimate for a query on the computed value, before and after
/// `analyze` has described it.
///
/// The claim being tested is not "the estimate changed" but "the estimate got
/// closer to the truth", so the truth is counted and both estimates are held
/// to it. Measured on this fixture — two thousand rows over fifty tag buckets,
/// five of which are null, so forty-five distinct computed values:
///
/// | predicate | rows | estimate before | after |
/// |---|---:|---:|---:|
/// | `lower(tag) = 'tag-07'` | 40 | 18 (2.22x out) | 40 (1.00x) |
/// | `lower(tag) < 'tag-10'` | 360 | 594 (1.65x out) | 352 (1.02x) |
///
/// The equality was wrong because a hundred distinct values is the default for
/// a column nobody measured, and there are fifty; the range because without a
/// histogram a one-sided bound is a flat third of the table whatever it asks.
#[tokio::test]
async fn the_planner_estimates_a_computed_predicate_from_its_own_statistics() {
    let (kv, analysed) = analysed_events().await;
    let table = events();
    let computed = Query::computed(&table, 0);
    let informed = events_store(&kv, analysed.clone());
    // Statistics that know the row count and the columns and nothing about the
    // expression — which is exactly what `analyze` produced before this change.
    let blind = events_store(&kv, blind_stats(&analysed));

    // An equality on a tag that exists, and a range over a fifth of the tags.
    let cases: [(&str, Expr); 2] = [
        ("lower(tag) = 'tag-07'", Expr::eq(computed, tag_value(7))),
        (
            "lower(tag) < 'tag-10'",
            Expr::compare(computed, CmpOp::Lt, tag_value(10)),
        ),
    ];

    for (label, filter) in cases {
        let query = Query::all().filter(filter.clone()).computing([lower_tag()]);

        // The truth, by running it.
        let actual = count_through(&informed, query.clone().using_table_scan()).await as f64;
        assert!(actual > 0.0, "{label} should match something");

        let with = informed
            .begin()
            .await
            .unwrap()
            .explain(&root(), &table, &query)
            .unwrap()
            .estimated_rows;
        let without = blind
            .begin()
            .await
            .unwrap()
            .explain(&root(), &table, &query)
            .unwrap()
            .estimated_rows;

        let error = |estimate: f64| (estimate / actual).max(actual / estimate);
        assert!(
            error(with) < error(without),
            "{label}: {actual} rows; analysed estimate {with} ({:.2}x out), \
             unanalysed {without} ({:.2}x out)",
            error(with),
            error(without),
        );
        // And close, not merely closer.
        assert!(
            error(with) < 1.2,
            "{label}: {actual} rows, estimated {with}"
        );
    }
}

fn tag_value(bucket: u64) -> Value {
    Value::Str(format!("tag-{bucket:02}"))
}

/// The same statistics with everything about the expression index removed:
/// the row count and the column measurements stay, so the comparison is about
/// the expression and not about the size of the table.
fn blind_stats(analysed: &TableStats) -> TableStats {
    let mut blind = TableStats::with_row_count(analysed.row_count);
    for ordinal in [EVENT_ID, TAG] {
        blind = blind.with_column(ordinal, analysed.column(ordinal));
        if let Some(histogram) = analysed.histogram(ordinal) {
            blind = blind.with_histogram(ordinal, histogram.clone());
        }
    }
    blind
}

/// Whatever the estimate, the answer is the same. The point of the whole
/// exercise is a better plan, never a different result.
#[tokio::test]
async fn an_expression_estimate_never_changes_the_answer() {
    let (kv, analysed) = analysed_events().await;
    let table = events();
    let computed = Query::computed(&table, 0);
    let informed = events_store(&kv, analysed.clone());
    let blind = events_store(&kv, blind_stats(&analysed));

    for bucket in [1u64, 7, 23, 49] {
        for op in [CmpOp::Eq, CmpOp::Lt, CmpOp::Ge] {
            let query = Query::all()
                .filter(Expr::compare(computed, op, tag_value(bucket)))
                .computing([lower_tag()]);
            let planned = count_through(&informed, query.clone()).await;
            let unplanned = count_through(&blind, query.clone()).await;
            let scanned = count_through(&informed, query.using_table_scan()).await;
            assert_eq!(planned, scanned, "disagreed on {op:?} {bucket}");
            assert_eq!(unplanned, scanned, "disagreed on {op:?} {bucket}");
        }
    }
}

/// Two expression indexes on one table are described separately.
///
/// The two are chosen to be impossible to confuse: every tag is the same
/// length, so `length(tag)` has exactly one distinct value and no histogram at
/// all, while `lower(tag)` has forty-five and does. Swap the two slots and both
/// halves of this fail.
#[tokio::test]
async fn two_expression_indexes_get_their_own_statistics() {
    let (_kv, stats) = analysed_events().await;
    let lowered = stats.expression(BY_LOWER_TAG).expect("analysed");
    let lengths = stats.expression(BY_TAG_LENGTH).expect("analysed");

    let by_hand = |scalar: Scalar| {
        event_rows()
            .iter()
            .map(|row| scalar.evaluate(row.values()))
            .filter(|v| !v.is_null())
            .collect::<std::collections::BTreeSet<Value>>()
            .len() as u64
    };
    assert_eq!(lowered.distinct, by_hand(lower_tag()));
    assert_eq!(lengths.distinct, by_hand(tag_length()));
    assert!(
        lowered.distinct > lengths.distinct,
        "{} against {}",
        lowered.distinct,
        lengths.distinct
    );

    // One distinct value is not a distribution, so the length index gets no
    // histogram where the lowered one does.
    assert!(stats.expression_histogram(BY_LOWER_TAG).is_some());
    assert!(stats.expression_histogram(BY_TAG_LENGTH).is_none());
}
