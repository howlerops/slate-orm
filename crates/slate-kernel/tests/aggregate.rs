//! Aggregates, grouping, and ordering.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::latency::{IoCounters, LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, CmpOp, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext,
    SortKey,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};
use std::sync::Arc;

const SALES: TableId = TableId(1);
const ROWS: u64 = 120;

fn sales() -> TableDef {
    TableDef::builder("sales", SALES)
        .column("id", ValueType::U64)
        .column("region", ValueType::Str)
        .column("amount", ValueType::I64)
        .nullable_column("discount", ValueType::F64)
        .column("note", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_region_amount", IndexId(10))
                .column("region")
                .column("amount"),
        )
        .index(
            IndexDef::builder("by_amount_desc", IndexId(11)).column_with("amount", Direction::Desc),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    sales().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        // Three regions.
        Value::Str(format!("r{}", id % 3)),
        Value::I64((id % 10) as i64),
        // A third of the discounts are missing.
        if id.is_multiple_of(3) {
            Value::Null
        } else {
            Value::F64(0.5)
        },
        Value::Str("padding that no aggregate should have to read".to_owned()),
    ])
}

async fn store() -> (RecordStore<LatencyStore<MemoryStore>>, Arc<IoCounters>) {
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

    let slow = LatencyStore::new(backing, LatencyProfile::free());
    let counters = slow.counters();
    (RecordStore::new(slow, catalog, security), counters)
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

#[tokio::test]
async fn aggregates_compute_what_they_say() {
    let (store, _) = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();

    let values = txn
        .aggregate(
            &root(),
            &table,
            &Query::all(),
            &[
                Aggregate::Count,
                Aggregate::CountColumn(col("discount")),
                Aggregate::Min(col("amount")),
                Aggregate::Max(col("amount")),
                Aggregate::Sum(col("amount")),
                Aggregate::Avg(col("amount")),
            ],
        )
        .await
        .unwrap();

    assert_eq!(values[0], Value::U64(ROWS));
    // A third of discounts are null, and COUNT(column) skips them.
    assert_eq!(values[1], Value::U64(ROWS - ROWS / 3));
    assert_eq!(values[2], Value::I64(0));
    assert_eq!(values[3], Value::I64(9));
    // Amounts cycle 0..9 twelve times.
    assert_eq!(values[4], Value::I64(45 * 12));
    assert_eq!(values[5], Value::F64(4.5));
}

/// Integer sums stay integers. Turning a total into a float behind the
/// caller's back is the sort of thing only noticed in a reconciliation.
#[tokio::test]
async fn an_integer_sum_stays_exact() {
    let (store, _) = store().await;
    let txn = store.begin().await.unwrap();
    let values = txn
        .aggregate(
            &root(),
            &sales(),
            &Query::all(),
            &[Aggregate::Sum(col("amount"))],
        )
        .await
        .unwrap();
    assert!(matches!(values[0], Value::I64(_)), "got {:?}", values[0]);

    // A float column totals as a float.
    let values = txn
        .aggregate(
            &root(),
            &sales(),
            &Query::all(),
            &[Aggregate::Sum(col("discount"))],
        )
        .await
        .unwrap();
    assert!(matches!(values[0], Value::F64(_)), "got {:?}", values[0]);
}

/// The point of putting aggregates in the kernel: they read only the columns
/// they need, so an index can answer without touching a row.
#[tokio::test]
async fn counting_reads_no_rows() {
    let (store, counters) = store().await;
    let txn = store.begin().await.unwrap();

    counters.reset();
    let total = txn.count(&root(), &sales(), &Query::all()).await.unwrap();
    assert_eq!(total, ROWS);
    assert_eq!(counters.gets(), 0, "counting should not read rows");

    // Summing an indexed column likewise.
    counters.reset();
    let values = txn
        .aggregate(
            &root(),
            &sales(),
            &Query::all().filter(Expr::eq(col("region"), Value::Str("r1".into()))),
            &[Aggregate::Sum(col("amount"))],
        )
        .await
        .unwrap();
    assert_eq!(counters.gets(), 0, "region and amount are both in an index");
    assert!(matches!(values[0], Value::I64(_)));
}

#[tokio::test]
async fn aggregates_respect_the_filter() {
    let (store, _) = store().await;
    let txn = store.begin().await.unwrap();
    let counted = txn
        .count(
            &root(),
            &sales(),
            &Query::all().filter(Expr::compare(col("amount"), CmpOp::Ge, Value::I64(5))),
        )
        .await
        .unwrap();
    assert_eq!(counted, ROWS / 2);
}

#[tokio::test]
async fn empty_aggregates_are_null_not_zero() {
    let (store, _) = store().await;
    let txn = store.begin().await.unwrap();
    let impossible = Query::all().filter(Expr::eq(col("region"), Value::Str("nowhere".into())));

    let values = txn
        .aggregate(
            &root(),
            &sales(),
            &impossible,
            &[
                Aggregate::Count,
                Aggregate::Min(col("amount")),
                Aggregate::Sum(col("amount")),
                Aggregate::Avg(col("amount")),
            ],
        )
        .await
        .unwrap();

    // Counting nothing is zero; the minimum of nothing is not.
    assert_eq!(values[0], Value::U64(0));
    assert_eq!(values[1], Value::Null);
    assert_eq!(values[2], Value::Null);
    assert_eq!(values[3], Value::Null);
}

#[tokio::test]
async fn grouping_splits_the_aggregate() {
    let (store, counters) = store().await;
    let txn = store.begin().await.unwrap();

    counters.reset();
    let groups = txn
        .group_by(
            &root(),
            &sales(),
            &Query::all(),
            &[col("region")],
            &[Aggregate::Count, Aggregate::Sum(col("amount"))],
        )
        .await
        .unwrap();

    assert_eq!(groups.len(), 3);
    assert_eq!(counters.gets(), 0, "region and amount are both indexed");

    // Ordered by the grouping columns, so the result is reproducible.
    let names: Vec<&Value> = groups.iter().map(|g| &g.key[0]).collect();
    assert_eq!(
        names,
        vec![
            &Value::Str("r0".into()),
            &Value::Str("r1".into()),
            &Value::Str("r2".into())
        ]
    );
    for group in &groups {
        assert_eq!(group.values[0], Value::U64(ROWS / 3));
    }

    // The counts add back up to the whole.
    let total: u64 = groups
        .iter()
        .map(|g| match g.values[0] {
            Value::U64(n) => n,
            _ => 0,
        })
        .sum();
    assert_eq!(total, ROWS);
}

/// An index that already produces the requested order should stream, not sort —
/// once it is actually the cheaper option.
///
/// Ordering alone is not enough to earn a non-covering index scan: sorting a
/// thousand rows in memory costs microseconds, and a thousand point reads costs
/// a thousand round trips. It wins when it covers the query, or when a limit
/// means only the first few rows are read at all.
#[tokio::test]
async fn an_ordered_covering_index_avoids_a_sort() {
    let (store, counters) = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();

    // `by_amount_desc` stores amounts descending, and holds the primary key, so
    // this projection needs no rows.
    let query = Query::all()
        .sort_by([SortKey::desc(col("amount"))])
        .select([col("id"), col("amount")]);
    let explained = txn.explain(&root(), &table, &query).unwrap();
    assert!(
        explained.is_index_only() && explained.to_string().contains("by_amount_desc"),
        "expected an index-only scan on the descending index, got {explained}"
    );

    counters.reset();
    let amounts: Vec<i64> = txn
        .execute(&root(), &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .map(|r| match r.values()[col("amount").0] {
            Value::I64(v) => v,
            ref other => panic!("amount was {other:?}"),
        })
        .collect();

    assert_eq!(amounts.len(), ROWS as usize);
    assert!(
        amounts.windows(2).all(|w| w[0] >= w[1]),
        "not in descending order"
    );
    assert_eq!(counters.gets(), 0, "an index-only scan reads no rows");
}

/// `ORDER BY` with a limit is the case where an index genuinely changes the
/// plan: a sort has to see every matching row before returning the first, so a
/// limit cannot make it cheap, while an ordered index reads only what is asked
/// for.
#[test]
fn order_by_with_a_limit_prefers_an_ordered_index() {
    use slate_kernel::{Access, Projection, ScanOrder, TableStats, plan_full};

    let table = sales();
    let stats = TableStats::with_row_count(1_000_000);
    let sort = [SortKey::desc(col("amount"))];

    let plan_it = |limit: Option<usize>| {
        plan_full(
            &table,
            std::sync::Arc::new(Expr::True),
            ScanOrder::Ascending,
            &Projection::All,
            &stats,
            limit,
            &sort,
        )
    };

    let unlimited = plan_it(None);
    assert!(
        matches!(unlimited.access, Access::TableScan { .. }) && unlimited.sort.is_some(),
        "a million point reads should lose to a sort, got {:?}",
        unlimited.access
    );

    let limited = plan_it(Some(10));
    assert!(
        matches!(limited.access, Access::IndexScan { .. }) && limited.sort.is_none(),
        "ten ordered rows should come from the index without sorting, got {:?}",
        limited.access
    );
    assert!(limited.estimated_cost < unlimited.estimated_cost);
}

/// And when no index produces it, the rows are sorted — correctly.
#[tokio::test]
async fn an_unindexed_order_is_sorted() {
    let (store, _) = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();

    let rows = txn
        .execute(
            &root(),
            &table,
            &Query::all().sort_by([SortKey::asc(col("note")), SortKey::desc(col("id"))]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // Every note is identical, so the tie-break decides: ids descending.
    let ids: Vec<u64> = rows
        .iter()
        .map(|r| match r.values()[0] {
            Value::U64(v) => v,
            ref other => panic!("id was {other:?}"),
        })
        .collect();
    assert_eq!(ids.len(), ROWS as usize);
    assert!(ids.windows(2).all(|w| w[0] > w[1]), "tie-break not applied");
}

/// Nulls are placed where the caller asks, independently of the direction the
/// values are sorted in.
#[tokio::test]
async fn nulls_go_where_they_are_asked_to() {
    let (store, _) = store().await;
    let table = sales();

    async fn first_is_null(
        store: &RecordStore<LatencyStore<MemoryStore>>,
        table: &TableDef,
        key: SortKey,
    ) -> bool {
        let txn = store.begin().await.unwrap();
        let rows = txn
            .execute(&root(), table, &Query::all().sort_by([key]).limit(1))
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        rows[0].values()[col("discount").0].is_null()
    }

    let discount = col("discount");
    assert!(first_is_null(&store, &table, SortKey::asc(discount).nulls_first()).await);
    assert!(!first_is_null(&store, &table, SortKey::asc(discount).nulls_last()).await);
    assert!(first_is_null(&store, &table, SortKey::desc(discount).nulls_first()).await);
    assert!(!first_is_null(&store, &table, SortKey::desc(discount).nulls_last()).await);
}

/// Sorting must not change which rows come back, only their order.
#[tokio::test]
async fn sorting_preserves_the_result_set() {
    let (store, _) = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();
    let filter = Expr::compare(col("amount"), CmpOp::Lt, Value::I64(4));

    let ids = |rows: Vec<Row>| -> Vec<u64> {
        let mut out: Vec<u64> = rows
            .into_iter()
            .map(|r| match r.values()[0] {
                Value::U64(v) => v,
                ref other => panic!("id was {other:?}"),
            })
            .collect();
        out.sort_unstable();
        out
    };

    let unsorted = txn
        .execute(&root(), &table, &Query::all().filter(filter.clone()))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let sorted = txn
        .execute(
            &root(),
            &table,
            &Query::all()
                .filter(filter)
                .sort_by([SortKey::desc(col("note")), SortKey::asc(col("amount"))]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert!(!unsorted.is_empty());
    assert_eq!(ids(unsorted), ids(sorted));
}

/// A sort combined with a limit still has to see everything before it can
/// return the first row, and the estimate should say so.
#[tokio::test]
async fn a_sort_with_a_limit_returns_the_true_top() {
    let (store, _) = store().await;
    let table = sales();
    let txn = store.begin().await.unwrap();

    let top = txn
        .execute(
            &root(),
            &table,
            &Query::all()
                .sort_by([SortKey::desc(col("note")), SortKey::desc(col("id"))])
                .limit(3),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let ids: Vec<u64> = top
        .iter()
        .map(|r| match r.values()[0] {
            Value::U64(v) => v,
            ref other => panic!("id was {other:?}"),
        })
        .collect();
    assert_eq!(ids, vec![ROWS - 1, ROWS - 2, ROWS - 3]);
}

/// A grouping with keys and *no* aggregates is `SELECT DISTINCT`.
///
/// Nothing was added to the kernel for this. A `Grouper` keyed on the columns
/// and folding nothing already yields exactly the distinct combinations, in
/// key order, and this test exists because that was an argument until it was
/// run. The value of writing it down is that the front ends can lower
/// `DISTINCT` to a grouping instead of growing an operator, and a later change
/// that makes an empty aggregate list a refusal in the kernel now breaks a
/// test that says why it must not.
#[tokio::test]
async fn a_grouping_with_no_aggregates_is_the_distinct_keys() {
    let (store, _) = store().await;
    let txn = store.begin().await.unwrap();

    let groups = txn
        .group_by(&root(), &sales(), &Query::all(), &[col("region")], &[])
        .await
        .unwrap();

    // Three regions over 120 rows, each key once, in key order.
    let keys: Vec<&Value> = groups.iter().map(|g| &g.key[0]).collect();
    assert_eq!(
        keys,
        vec![
            &Value::Str("r0".into()),
            &Value::Str("r1".into()),
            &Value::Str("r2".into())
        ]
    );
    // And no values beside them. A group carrying a phantom count would make
    // every front end that lowers DISTINCT this way render a stray column.
    for group in &groups {
        assert!(group.values.is_empty(), "{:?}", group.values);
    }
}

/// Distinct over more than one column is the *combination*, not each column's
/// own distinct values.
///
/// The fixture is built so the two answers differ: `region` cycles every 3 rows
/// and `amount` every 10, so there are 3 regions, 10 amounts, and 30
/// combinations — a lowering that grouped each column separately and zipped
/// them would produce 10.
#[tokio::test]
async fn distinct_over_two_columns_is_the_combination() {
    let (store, _) = store().await;
    let txn = store.begin().await.unwrap();

    let groups = txn
        .group_by(
            &root(),
            &sales(),
            &Query::all(),
            &[col("region"), col("amount")],
            &[],
        )
        .await
        .unwrap();

    assert_eq!(groups.len(), 30, "3 regions x 10 amounts");
    // Every key really is distinct, which the count alone does not establish:
    // thirty groups with a repeat among them would count the same.
    let mut seen: Vec<&Vec<Value>> = groups.iter().map(|g| &g.key).collect();
    seen.sort();
    let before = seen.len();
    seen.dedup();
    assert_eq!(seen.len(), before, "a key came back twice");
}
