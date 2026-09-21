//! Window functions: one value per input row, computed over a partition.
//!
//! **The oracle here is `GROUP BY`.** A window aggregate over a partition and
//! a grouped aggregate over the same key are the same number reached by
//! completely different code — `Grouper` hashes keys and folds rows away,
//! `window::evaluate` sorts indices and keeps every row — so agreeing is
//! evidence rather than a restatement. That is what
//! `a_partition_aggregate_agrees_with_the_same_group_by` checks, over every
//! aggregate the two share.
//!
//! The ranking functions have no such sibling, so they are checked against
//! properties that a wrong implementation cannot satisfy by accident: the row
//! numbers of a partition are exactly `1..=n`, and ordering by the window's
//! own keys must give the same sequence as ordering by the number it assigned.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::window::{Window, WindowFunction};
use slate_kernel::{
    Action, Aggregate, Grant, KernelError, Query, RecordStore, SecurityCatalog, SecurityContext,
    SortKey,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const SALES: TableId = TableId(1);
const ROWS: u64 = 60;

/// Three regions, ten amounts, and a `year` that **ties on purpose**.
///
/// The ties are the point. Peer handling is the half of window semantics that
/// a plausible implementation gets wrong, and a table whose order column is
/// unique cannot tell a correct `RANK` from a `ROW_NUMBER` wearing its name,
/// or a `RANGE` running total from a `ROWS` one.
fn sales() -> TableDef {
    TableDef::builder("sales", SALES)
        .column("id", ValueType::U64)
        .column("region", ValueType::Str)
        .column("amount", ValueType::I64)
        .column("year", ValueType::I64)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    sales().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("r{}", id % 3)),
        Value::I64((id % 10) as i64),
        // Five distinct years across twenty rows per region, so every peer
        // group has four members.
        Value::I64(2000 + (id % 5) as i64),
    ])
}

async fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([sales()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", SALES, Action::ALL));
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());

    let root = SecurityContext::superuser();
    let txn = loader.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &sales(), &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    RecordStore::new(backing, catalog, security)
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// Where the `index`th window lands on a query with no computed values.
fn at(index: usize) -> Ordinal {
    Query::windowed(&sales(), 0, index)
}

async fn run(query: &Query) -> Vec<Row> {
    let store = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();
    let cursor = txn.execute(&root(), &table, query).await.unwrap();
    let rows = cursor.collect().await.unwrap();
    txn.rollback();
    rows
}

#[tokio::test]
async fn a_row_number_numbers_each_partition_from_one() {
    let rows = run(&Query::all().windowing([Window::new(
        WindowFunction::RowNumber,
        vec![col("region")],
        vec![SortKey::asc(col("id"))],
    )
    .unwrap()]))
    .await;
    assert_eq!(rows.len() as u64, ROWS);

    // Every partition's numbers are exactly 1..=n, with no gap and no repeat.
    // A wrong implementation that numbered the whole result and then split it
    // would produce 1..=60 spread across three partitions and fail here.
    for region in 0..3 {
        let name = Value::Str(format!("r{region}"));
        let mut numbers: Vec<u64> = rows
            .iter()
            .filter(|r| r.get(col("region")) == Some(&name))
            .map(|r| match r.get(at(0)) {
                Some(&Value::U64(n)) => n,
                other => panic!("row number is {other:?}"),
            })
            .collect();
        numbers.sort_unstable();
        assert_eq!(numbers, (1..=numbers.len() as u64).collect::<Vec<_>>());
    }

    // And the numbering follows the window's order rather than some other one:
    // within a partition, sorting by `id` and sorting by the assigned number
    // give the same sequence.
    for region in 0..3 {
        let name = Value::Str(format!("r{region}"));
        let mut by_id: Vec<(u64, u64)> = rows
            .iter()
            .filter(|r| r.get(col("region")) == Some(&name))
            .map(|r| {
                let (Some(&Value::U64(id)), Some(&Value::U64(n))) =
                    (r.get(col("id")), r.get(at(0)))
                else {
                    panic!("unexpected row {r:?}")
                };
                (id, n)
            })
            .collect();
        by_id.sort_unstable();
        for (expected, (_, actual)) in by_id.iter().enumerate() {
            assert_eq!(*actual, expected as u64 + 1, "{by_id:?}");
        }
    }
}

/// `RANK` skips and `DENSE_RANK` does not, over a column with real ties.
#[tokio::test]
async fn rank_leaves_the_gap_that_dense_rank_closes() {
    let rows = run(&Query::all().windowing([
        Window::new(
            WindowFunction::Rank,
            vec![col("region")],
            vec![SortKey::asc(col("year"))],
        )
        .unwrap(),
        Window::new(
            WindowFunction::DenseRank,
            vec![col("region")],
            vec![SortKey::asc(col("year"))],
        )
        .unwrap(),
    ]))
    .await;

    // Twenty rows per region, five distinct years, four rows each. So the
    // ranks are 1, 5, 9, 13, 17 and the dense ranks are 1..=5.
    let mut ranks: Vec<u64> = Vec::new();
    let mut dense: Vec<u64> = Vec::new();
    for r in &rows {
        if r.get(col("region")) != Some(&Value::Str("r0".to_owned())) {
            continue;
        }
        let (Some(&Value::U64(rank)), Some(&Value::U64(d))) = (r.get(at(0)), r.get(at(1))) else {
            panic!("unexpected row {r:?}")
        };
        ranks.push(rank);
        dense.push(d);
    }
    ranks.sort_unstable();
    ranks.dedup();
    dense.sort_unstable();
    dense.dedup();
    assert_eq!(ranks, vec![1, 5, 9, 13, 17]);
    assert_eq!(dense, vec![1, 2, 3, 4, 5]);
}

/// A whole-partition aggregate is the grouped aggregate, on every row.
///
/// The oracle this file exists for. `Grouper` and `window::evaluate` share the
/// accumulator arithmetic and nothing else — one hashes the key and drops the
/// row, the other sorts indices and keeps it — so a disagreement is a real
/// defect in one of them rather than two spellings of one mistake.
#[tokio::test]
async fn a_partition_aggregate_agrees_with_the_same_group_by() {
    let amount = col("amount");
    let aggregates = [
        Aggregate::Count,
        Aggregate::CountColumn(amount),
        Aggregate::Min(amount),
        Aggregate::Max(amount),
        Aggregate::Sum(amount),
        Aggregate::Avg(amount),
        // Allowed here because there is no `ORDER BY`: one distinct set per
        // partition rather than one per peer group.
        Aggregate::CountDistinct(amount),
    ];

    let windows: Vec<Window> = aggregates
        .iter()
        .map(|&a| Window::new(WindowFunction::Over(a), vec![col("region")], Vec::new()).unwrap())
        .collect();
    let rows = run(&Query::all().windowing(windows)).await;

    let store = store().await;
    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by(
            &root(),
            &sales(),
            &Query::all(),
            &[col("region")],
            &aggregates,
        )
        .await
        .unwrap();
    txn.rollback();
    assert_eq!(groups.len(), 3);

    for group in &groups {
        let matching: Vec<&Row> = rows
            .iter()
            .filter(|r| r.get(col("region")) == Some(&group.key[0]))
            .collect();
        assert!(!matching.is_empty(), "no rows for {:?}", group.key);
        for row in matching {
            for (index, expected) in group.values.iter().enumerate() {
                assert_eq!(
                    row.get(at(index)),
                    Some(expected),
                    "aggregate {index} on {row:?} for group {:?}",
                    group.key
                );
            }
        }
    }
}

/// An ordered window aggregate is a *running* total, and peers share a value.
///
/// This is SQL's default frame changing under an `ORDER BY`, and the peer half
/// is what separates `RANGE` from `ROWS`. With four rows per year, a `ROWS`
/// implementation would give the four rows of 2000 four different totals; a
/// `RANGE` one gives them all the same.
#[tokio::test]
async fn a_running_total_gives_peers_the_same_value() {
    let rows = run(&Query::all().windowing([Window::new(
        WindowFunction::Over(Aggregate::Count),
        vec![col("region")],
        vec![SortKey::asc(col("year"))],
    )
    .unwrap()]))
    .await;

    // Region r0 has twenty rows, four in each of five years. The running count
    // through each year's peer group is therefore 4, 8, 12, 16, 20 — and every
    // row of a year carries its year's number, not its own position.
    let mut by_year: Vec<(i64, u64)> = rows
        .iter()
        .filter(|r| r.get(col("region")) == Some(&Value::Str("r0".to_owned())))
        .map(|r| {
            let (Some(&Value::I64(year)), Some(&Value::U64(running))) =
                (r.get(col("year")), r.get(at(0)))
            else {
                panic!("unexpected row {r:?}")
            };
            (year, running)
        })
        .collect();
    by_year.sort_unstable();
    by_year.dedup();
    assert_eq!(
        by_year,
        vec![(2000, 4), (2001, 8), (2002, 12), (2003, 16), (2004, 20)]
    );

    // And the last peer group's value is the whole partition's, which is the
    // frame's other end.
    assert_eq!(by_year.last().unwrap().1, 20);
}

/// No `PARTITION BY` is one partition over the whole result, not one per row.
#[tokio::test]
async fn an_unpartitioned_window_sees_every_row() {
    let rows = run(&Query::all().windowing([Window::new(
        WindowFunction::Over(Aggregate::Count),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()]))
    .await;
    assert_eq!(rows.len() as u64, ROWS);
    for row in &rows {
        assert_eq!(row.get(at(0)), Some(&Value::U64(ROWS)));
    }
}

/// `LAG` and `LEAD` step through the partition's own order.
#[tokio::test]
async fn lag_and_lead_step_within_the_partition() {
    let rows = run(&Query::all().windowing([
        Window::new(
            WindowFunction::Lag {
                column: col("id"),
                offset: 1,
            },
            vec![col("region")],
            vec![SortKey::asc(col("id"))],
        )
        .unwrap(),
        Window::new(
            WindowFunction::Lead {
                column: col("id"),
                offset: 1,
            },
            vec![col("region")],
            vec![SortKey::asc(col("id"))],
        )
        .unwrap(),
    ]))
    .await;

    // Region r0 is ids 0, 3, 6, …, so the previous id is three lower and the
    // next three higher — which a `LAG` that stepped through the *result*
    // rather than the partition would get wrong by one.
    for row in &rows {
        if row.get(col("region")) != Some(&Value::Str("r0".to_owned())) {
            continue;
        }
        let Some(&Value::U64(id)) = row.get(col("id")) else {
            panic!("unexpected row {row:?}")
        };
        let expected_lag = if id == 0 {
            Value::Null
        } else {
            Value::U64(id - 3)
        };
        let expected_lead = if id + 3 < ROWS {
            Value::U64(id + 3)
        } else {
            Value::Null
        };
        assert_eq!(row.get(at(0)), Some(&expected_lag), "lag on {row:?}");
        assert_eq!(row.get(at(1)), Some(&expected_lead), "lead on {row:?}");
    }
}

/// The query's `ORDER BY` runs *after* the window, so it can order by one.
///
/// **Partitioned by `year`, not by `region`, and that is load-bearing.** The
/// first version of this test partitioned by region, whose three totals are
/// all 90 — the seed's amounts cycle with period 30 and the regions with
/// period 3 — so every row carried the same value and *any* order satisfied
/// "descending". Sorting before the window instead of after it survived the
/// mutation run against that version. The per-year totals are 30, 42, 54, 66
/// and 78, and the distinctness is asserted below so the next change to the
/// seed cannot quietly restore the hole.
#[tokio::test]
async fn a_query_can_order_by_the_value_a_window_produced() {
    let rows = run(&Query::all()
        .windowing([Window::new(
            WindowFunction::Over(Aggregate::Sum(col("amount"))),
            vec![col("year")],
            Vec::new(),
        )
        .unwrap()])
        .sort_by([SortKey::desc(at(0)), SortKey::asc(col("id"))]))
    .await;

    let totals: Vec<&Value> = rows.iter().map(|r| r.get(at(0)).unwrap()).collect();
    let mut distinct: Vec<&&Value> = totals.iter().collect();
    distinct.sort();
    distinct.dedup();
    assert!(
        distinct.len() > 1,
        "every row carries the same value, so any order would pass: {totals:?}"
    );
    assert!(
        totals.windows(2).all(|pair| pair[0] >= pair[1]),
        "not descending by the window's value: {totals:?}"
    );
}

/// A `LIMIT` trims the answer and does not truncate the window's input.
///
/// The interaction the executor's own comment is about: `ORDER BY … LIMIT 10`
/// normally keeps only the best ten rows, and doing that here would compute
/// the window over ten rows instead of sixty. The count that comes back says
/// which happened.
#[tokio::test]
async fn a_limit_does_not_shrink_what_the_window_sees() {
    let rows = run(&Query::all()
        .windowing([Window::new(
            WindowFunction::Over(Aggregate::Count),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()])
        .sort_by([SortKey::asc(col("id"))])
        .limit(10))
    .await;

    assert_eq!(rows.len(), 10);
    for row in &rows {
        assert_eq!(
            row.get(at(0)),
            Some(&Value::U64(ROWS)),
            "the window counted the page rather than the query"
        );
    }
}

/// A projection that leaves out the partition column still partitions by it.
///
/// The column has to be decoded whether or not the caller asked to see it —
/// the rule already written for sort keys. Left out, it would read back null
/// on every row, put the whole result in one partition, and answer without
/// saying anything was wrong.
#[tokio::test]
async fn a_window_partitions_by_a_column_outside_the_projection() {
    let rows = run(&Query::all().select([col("id")]).windowing([Window::new(
        WindowFunction::Over(Aggregate::Count),
        vec![col("region")],
        Vec::new(),
    )
    .unwrap()]))
    .await;

    for row in &rows {
        assert_eq!(
            row.get(at(0)),
            Some(&Value::U64(ROWS / 3)),
            "one partition instead of three: {row:?}"
        );
    }
}

/// And a window *orders* by a column outside the projection too.
///
/// The same rule as the test above and a separate failure: a partition column
/// read as null collapses the partitions, an order column read as null makes
/// every row a peer of every other. Both are silent, and they are caught by
/// different assertions — dropping the order columns from
/// `Window::collect_columns` left the partition test passing.
#[tokio::test]
async fn a_window_orders_by_a_column_outside_the_projection() {
    let rows = run(&Query::all()
        .select([col("id"), col("region")])
        .windowing([Window::new(
            WindowFunction::Rank,
            vec![col("region")],
            vec![SortKey::asc(col("year"))],
        )
        .unwrap()]))
    .await;

    // Twenty rows per region over five years, so 1, 5, 9, 13, 17. With `year`
    // undecoded every row is a peer of every other and every rank is 1.
    let mut ranks: Vec<u64> = rows
        .iter()
        .filter(|r| r.get(col("region")) == Some(&Value::Str("r0".to_owned())))
        .map(|r| match r.get(at(0)) {
            Some(&Value::U64(rank)) => rank,
            other => panic!("rank is {other:?}"),
        })
        .collect();
    ranks.sort_unstable();
    ranks.dedup();
    assert_eq!(ranks, vec![1, 5, 9, 13, 17]);
}

/// Every specification that has no answer is refused, by name.
#[test]
fn a_meaningless_window_is_refused_rather_than_answered() {
    let region = vec![Ordinal(1)];
    let by_year = vec![SortKey::asc(Ordinal(3))];

    // A rank with nothing to rank by.
    for function in [
        WindowFunction::RowNumber,
        WindowFunction::Rank,
        WindowFunction::DenseRank,
        WindowFunction::Lag {
            column: Ordinal(0),
            offset: 1,
        },
        WindowFunction::Lead {
            column: Ordinal(0),
            offset: 1,
        },
    ] {
        let refused = Window::new(function, region.clone(), Vec::new());
        assert!(
            matches!(refused, Err(KernelError::WindowNeedsOrder { .. })),
            "{function:?} was accepted without an order: {refused:?}"
        );
    }

    // A running distinct count, which would hold one value set per peer group.
    let refused = Window::new(
        WindowFunction::Over(Aggregate::CountDistinct(Ordinal(2))),
        region.clone(),
        by_year.clone(),
    );
    assert!(
        matches!(refused, Err(KernelError::RunningDistinctCount)),
        "{refused:?}"
    );
    // The same thing unordered is one set for the partition, and is allowed.
    assert!(
        Window::new(
            WindowFunction::Over(Aggregate::CountDistinct(Ordinal(2))),
            region.clone(),
            Vec::new(),
        )
        .is_ok()
    );

    // An offset of zero, which is the current row written obscurely.
    for function in [
        WindowFunction::Lag {
            column: Ordinal(0),
            offset: 0,
        },
        WindowFunction::Lead {
            column: Ordinal(0),
            offset: 0,
        },
    ] {
        let refused = Window::new(function, region.clone(), by_year.clone());
        assert!(
            matches!(refused, Err(KernelError::WindowOffsetZero { .. })),
            "{function:?} was accepted at offset zero: {refused:?}"
        );
    }
}

/// `EXPLAIN` says a window runs, how many, and that nothing streams.
///
/// The count rather than a flag, because each distinct specification is its
/// own permutation of the held rows and "four windows" costs differently from
/// "one". `streams()` is the part a reader acts on: a plan with a window
/// buffers the whole result even when the access path already gives the
/// requested order and there is no sort at all — which is exactly this case,
/// and is why the flag could not be left as `!sorts`.
#[tokio::test]
async fn explain_reports_the_window_and_that_the_plan_stops_streaming() {
    let store = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();
    let plain = txn.explain(&root(), &table, &Query::all()).unwrap();
    let windowed = txn
        .explain(
            &root(),
            &table,
            &Query::all().windowing([
                Window::new(
                    WindowFunction::Over(Aggregate::Count),
                    vec![col("region")],
                    Vec::new(),
                )
                .unwrap(),
                Window::new(
                    WindowFunction::RowNumber,
                    vec![col("region")],
                    vec![SortKey::asc(col("id"))],
                )
                .unwrap(),
            ]),
        )
        .unwrap();
    txn.rollback();

    // The same access path, and a plain scan streams.
    assert_eq!(plain.windows, 0);
    assert!(plain.streams(), "{plain}");
    assert!(!plain.to_string().contains("Window"), "{plain}");

    assert_eq!(windowed.windows, 2);
    assert!(!windowed.sorts, "the query has no ORDER BY: {windowed}");
    assert!(
        !windowed.streams(),
        "a window buffers even with no sort: {windowed}"
    );
    assert!(windowed.to_string().contains("Window x2 -> "), "{windowed}");
}

/// A paged read carrying a window is refused, not answered per page.
#[tokio::test]
async fn a_window_over_a_page_is_refused() {
    let store = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();
    let query = Query::all()
        .windowing([Window::new(
            WindowFunction::Over(Aggregate::Count),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()])
        .paging()
        .limit(10);
    let refused = txn.execute(&root(), &table, &query).await.err();
    assert!(
        matches!(refused, Some(KernelError::InvalidCursor { .. })),
        "a window over a page was answered: {refused:?}"
    );
}

/// The ceiling is the window's own, and a `LIMIT` does not evade it.
#[tokio::test]
async fn too_many_rows_for_a_window_is_refused_and_a_limit_does_not_help() {
    let catalog = Catalog::from_tables([sales()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", SALES, Action::ALL));
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let txn = loader.begin().await.unwrap();
    for id in 0..10u64 {
        txn.insert(&root(), &sales(), &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    let limits = slate_kernel::ExecutionLimits {
        max_window_rows: 5,
        ..Default::default()
    };
    let store = RecordStore::new(backing, catalog, security).with_limits(limits);

    // With a limit well under the ceiling, which is the whole point: the
    // window runs before the limit applies, so the limit cannot rescue it the
    // way it rescues a sort.
    let query = Query::all()
        .windowing([Window::new(
            WindowFunction::Over(Aggregate::Count),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()])
        .limit(2);
    let table = sales();
    let txn = store.begin().await.unwrap();
    let refused = match txn.execute(&root(), &table, &query).await {
        Err(error) => Err(error),
        Ok(cursor) => cursor.collect().await,
    };
    assert!(
        matches!(refused, Err(KernelError::WindowTooLarge { limit: 5 })),
        "{refused:?}"
    );
}
