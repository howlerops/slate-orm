//! What one request can make the server allocate or compute.
//!
//! A join has [`slate_kernel::DEFAULT_BUILD_LIMIT`] and reports
//! `JoinBuildTooLarge` rather than dying. Nothing else that holds unbounded
//! state per request has an equivalent, and nothing bounds the *work* an
//! `IN` list costs per row. These are measurements rather than assertions
//! about a threshold: each records what is currently unbounded.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, Aggregate, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext, SortKey,
    memory::MemoryStore,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::time::Instant;

const T: TableId = TableId(1);
const ROWS: u64 = 2_000;

fn table() -> TableDef {
    TableDef::builder("t", T)
        .column("id", ValueType::U64)
        .column("k", ValueType::I64)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("app", T, Action::ALL))
}

fn app() -> SecurityContext {
    SecurityContext::new(slate_kernel::Principal::new(Value::U64(1)).with_role("app"))
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(
            &root,
            &table(),
            &Row::new(vec![Value::U64(id), Value::I64(id as i64)]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    store
}

/// Was: a large `IN` list cost O(list) per scanned row, so the caller chose
/// the multiplier. The list is now arranged for lookup once per plan, so a
/// row costs a binary search.
///
/// `MAX_POINT_GETS` and `MAX_INDEX_RANGES` still stop a large list becoming an
/// access path; what changed is what happens when it stays in the residual.
#[tokio::test]
async fn a_large_in_list_no_longer_costs_the_list_length_per_row() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();

    let run = |n: usize| {
        let values: Vec<Value> = (0..n).map(|i| Value::I64(1_000_000 + i as i64)).collect();
        Query::all().filter(Expr::In {
            column: Ordinal(1),
            values,
        })
    };

    let small = run(8);
    let start = Instant::now();
    let n = txn.count(&app(), &table(), &small).await.unwrap();
    let cheap = start.elapsed();
    assert_eq!(n, 0);

    let big = run(50_000);
    let start = Instant::now();
    let n = txn.count(&app(), &table(), &big).await.unwrap();
    let dear = start.elapsed();
    assert_eq!(n, 0, "neither list matches anything");

    let ratio = dear.as_secs_f64() / cheap.as_secs_f64().max(f64::MIN_POSITIVE);
    println!("IN(8) over {ROWS} rows: {cheap:?}; IN(50000): {dear:?} ({ratio:.1}x)");

    // Was 362x, measured. Now 4.0-4.7x over five runs, and what remains is
    // the one-time arrangement of 50,000 values rather than per-row work.
    // The bound is loose because this runs on a shared machine; 20x would
    // still be a regression of the kind this test exists to catch.
    assert!(
        ratio < 20.0,
        "the list length should no longer be a per-row multiplier: \
         {cheap:?} vs {dear:?} ({ratio:.1}x)"
    );
}

/// The prepared form must answer exactly what the plain one does, including
/// the three-valued cases: a null candidate in the list makes a non-match
/// `Unknown`, and a null in the *column* is `Unknown` whatever the list holds.
///
/// An oracle rather than a case list, because the interesting inputs here are
/// the ones nobody thinks to write down.
#[test]
fn the_prepared_in_agrees_with_the_plain_one_on_every_input() {
    use slate_kernel::Truth;
    use slate_schema::Row;

    let candidates = [
        Value::Null,
        Value::I64(-1),
        Value::I64(0),
        Value::I64(1),
        Value::I64(2),
        Value::I64(7),
        Value::Str("x".to_owned()),
    ];

    // Lists long enough to be rewritten, short enough to stay as they are, and
    // the awkward shapes: duplicates, nulls, unsorted, empty.
    let lists: Vec<Vec<Value>> = vec![
        vec![],
        vec![Value::I64(1)],
        vec![Value::Null],
        vec![Value::I64(2), Value::I64(1), Value::I64(2)],
        (0..40).map(|i| Value::I64(40 - i)).collect(),
        (0..40)
            .map(|i| {
                if i % 7 == 0 {
                    Value::Null
                } else {
                    Value::I64(i)
                }
            })
            .collect(),
        (0..40).map(|_| Value::I64(3)).collect(),
    ];

    for list in &lists {
        let plain = Expr::In {
            column: Ordinal(0),
            values: list.clone(),
        };
        let prepared = plain.prepared();
        for candidate in &candidates {
            let row = Row::new(vec![candidate.clone()]);
            let a = plain.evaluate(&row);
            let b = prepared.evaluate(&row);
            assert_eq!(
                a,
                b,
                "disagreement on {candidate:?} against a list of {} \
                 (plain {a:?}, prepared {b:?})",
                list.len()
            );
            // And the answer is actually right, not merely consistent.
            let expected = if candidate.is_null() {
                Truth::Unknown
            } else if list.iter().any(|v| !v.is_null() && v == candidate) {
                Truth::True
            } else if list.iter().any(Value::is_null) {
                Truth::Unknown
            } else {
                Truth::False
            };
            assert_eq!(a, expected, "the shared answer is wrong for {candidate:?}");
        }
    }
}

/// A `GROUP BY` on a unique column still holds one entry per row. What
/// changed is that there is now a ceiling; below it the behaviour is the same,
/// and this pins that the cap did not quietly become a truncation.
#[tokio::test]
async fn a_group_by_holds_every_distinct_key_below_the_ceiling() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by(
            &app(),
            &table(),
            &Query::all(),
            &[Ordinal(1)],
            &[Aggregate::Count],
        )
        .await
        .unwrap();
    assert_eq!(
        groups.len() as u64,
        ROWS,
        "one group per row, all held in memory at once, and nothing refuses it"
    );
}

/// `COUNT(DISTINCT)` holds every distinct encoded value, with no cap.
#[tokio::test]
async fn count_distinct_holds_every_value_below_the_ceiling() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let values = txn
        .aggregate(
            &app(),
            &table(),
            &Query::all(),
            &[Aggregate::CountDistinct(Ordinal(1))],
        )
        .await
        .unwrap();
    assert_eq!(values[0], Value::U64(ROWS));
}

/// An unlimited `ORDER BY` materialises the whole result before the first row.
///
/// With a limit the executor uses a bounded heap; without one it collects
/// everything, and nothing caps that.
#[tokio::test]
async fn an_unlimited_sort_still_materialises_everything_below_the_ceiling() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(
            &app(),
            &table(),
            &Query::all().sort_by([SortKey::desc(Ordinal(1))]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len() as u64, ROWS);
}

/// A repeated value in an `IN` list selects the same rows once, so the
/// estimate must not count it twice.
///
/// This is the one behavioural difference between the plain and prepared
/// forms, and it is a fix rather than a discrepancy: `predicate_selectivity`
/// multiplies the per-value equality selectivity by the list length, so
/// `IN (3, 3, 3, ...)` estimated forty times too many rows before the list
/// was deduplicated. Asserted because a comment in `stats.rs` claims it.
#[test]
fn a_repeated_value_is_not_counted_twice_in_the_estimate() {
    use slate_kernel::TableStats;

    // Defaults for the column, so the estimate is driven by the list length
    // alone — which is the thing under test.
    let stats = TableStats::with_row_count(1_000);

    let repeated = Expr::In {
        column: Ordinal(0),
        values: (0..40).map(|_| Value::I64(3)).collect(),
    };
    let once = Expr::In {
        column: Ordinal(0),
        values: vec![Value::I64(3)],
    };

    let plain = stats.predicate_selectivity(&repeated);
    let prepared = stats.predicate_selectivity(&repeated.prepared());
    let truth = stats.predicate_selectivity(&once);

    assert!(
        plain > prepared,
        "the plain form over-counts the duplicates: {plain} vs {prepared}"
    );
    assert!(
        (prepared - truth).abs() < f64::EPSILON,
        "the prepared form should estimate what one copy estimates: \
         {prepared} vs {truth}"
    );
}

/// Each unbounded accumulator now refuses at a ceiling rather than growing
/// until the node dies. Set very low so the test is about the refusal rather
/// than about allocating a gigabyte.
mod limits {
    use super::*;
    use slate_kernel::{Aggregate, ExecutionLimits, Grouping, KernelError, SortKey};

    async fn store_limited_to(limits: ExecutionLimits) -> RecordStore<MemoryStore> {
        let catalog = Catalog::from_tables([table()]).expect("catalog");
        let store = RecordStore::new(MemoryStore::new(), catalog, security()).with_limits(limits);
        let root = SecurityContext::superuser();
        let txn = store.begin().await.unwrap();
        for i in 0..100u64 {
            txn.insert(
                &root,
                &table(),
                &Row::new(vec![Value::U64(i), Value::I64(i as i64)]),
            )
            .await
            .unwrap();
        }
        txn.commit().await.unwrap();
        store
    }

    #[tokio::test]
    async fn a_group_by_past_the_ceiling_is_refused_and_names_it() {
        let limits = ExecutionLimits {
            max_groups: 10,
            ..ExecutionLimits::default()
        };
        let store = store_limited_to(limits).await;
        let txn = store.begin().await.unwrap();

        // 100 distinct keys against a ceiling of 10.
        let grouping = Grouping::by([Ordinal(1)], &[Aggregate::Count]);
        let err = txn
            .grouped(&app(), &table(), &Query::all(), &grouping)
            .await
            .expect_err("grouping past the ceiling must be refused");

        assert!(
            matches!(err, KernelError::TooManyGroups { limit: 10 }),
            "expected TooManyGroups naming the limit, got {err:?}"
        );
        assert!(
            err.to_string().contains("10"),
            "the message should name the limit: {err}"
        );
    }

    #[tokio::test]
    async fn a_count_distinct_past_the_ceiling_is_refused() {
        let limits = ExecutionLimits {
            max_distinct: 10,
            ..ExecutionLimits::default()
        };
        let store = store_limited_to(limits).await;
        let txn = store.begin().await.unwrap();

        let err = txn
            .aggregate(
                &app(),
                &table(),
                &Query::all(),
                &[Aggregate::CountDistinct(Ordinal(1))],
            )
            .await
            .expect_err("counting more distinct values than the ceiling must be refused");

        assert!(
            matches!(err, KernelError::TooManyDistinctValues { limit: 10 }),
            "expected TooManyDistinctValues, got {err:?}"
        );
    }

    #[tokio::test]
    async fn an_unlimited_sort_past_the_ceiling_is_refused_but_a_limited_one_is_not() {
        let limits = ExecutionLimits {
            max_sort_rows: 10,
            ..ExecutionLimits::default()
        };
        let store = store_limited_to(limits).await;
        let txn = store.begin().await.unwrap();

        let sorted = Query::all().sort_by([SortKey::asc(Ordinal(1))]);
        let err = txn
            .execute(&app(), &table(), &sorted)
            .await
            .expect_err("an unlimited sort past the ceiling must be refused");
        assert!(
            matches!(err, KernelError::SortTooLarge { limit: 10 }),
            "expected SortTooLarge, got {err:?}"
        );

        // The point of the error message: a LIMIT uses the bounded heap, so
        // the same query with one is not affected by this ceiling at all.
        let windowed = Query::all().sort_by([SortKey::asc(Ordinal(1))]).limit(5);
        let rows = txn
            .execute(&app(), &table(), &windowed)
            .await
            .expect("a limited sort uses the bounded heap and is not capped")
            .collect()
            .await
            .expect("collecting the window");
        assert_eq!(rows.len(), 5);
    }
}
