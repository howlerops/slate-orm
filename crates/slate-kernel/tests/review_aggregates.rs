//! Adversarial probes on aggregation over values whose *type* varies row by
//! row.
//!
//! Every column has one declared type, so until a query could compute a value
//! this was unreachable: an `i64` column produced `Value::I64` on every row and
//! `Total` never saw two domains. `Scalar` changed that. `coalesce(price,
//! amount)` over a nullable `f64` and an `i64` is an ordinary thing to write
//! and produces a float on some rows and an integer on others, and it is the
//! shape the running total was never given.
//!
//! The property is the one this project keeps reaching for: an aggregate is a
//! fold over a *set*, so nothing about the answer may depend on the order the
//! rows were folded in. Scan direction is the cheapest way to vary that order
//! without varying anything else — same rows, same predicate, same plan shape.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, Grant, Query, RecordStore, Scalar, ScanOrder, SecurityCatalog,
    SecurityContext,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const SALES: TableId = TableId(1);

fn sales() -> TableDef {
    TableDef::builder("sales", SALES)
        .column("id", ValueType::U64)
        .column("amount", ValueType::I64)
        .nullable_column("price", ValueType::F64)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    sales().ordinal_of(name).expect("column exists")
}

/// Two rows: the first priced, the second not. `coalesce(price, amount)` is
/// therefore `F64(1.5)` on the first and `I64(10)` on the second.
fn rows() -> Vec<Row> {
    vec![
        Row::new(vec![Value::U64(0), Value::I64(1), Value::F64(1.5)]),
        Row::new(vec![Value::U64(1), Value::I64(10), Value::Null]),
    ]
}

async fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([sales()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", SALES, Action::ALL));
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), SecurityCatalog::new());
    let root = SecurityContext::superuser();
    let txn = loader.begin().await.unwrap();
    for row in rows() {
        txn.insert(&root, &sales(), &row).await.unwrap();
    }
    txn.commit().await.unwrap();
    RecordStore::new(backing, catalog, security)
}

/// `coalesce(price, amount)`, appended after the table's own columns.
fn coalesced() -> (Vec<Scalar>, Ordinal) {
    let width = sales().columns().len();
    (
        vec![Scalar::Coalesce(vec![
            Scalar::Column(col("price")),
            Scalar::Column(col("amount")),
        ])],
        Ordinal(width),
    )
}

async fn total(order: ScanOrder, aggregate: Aggregate) -> Value {
    let store = store().await;
    let (compute, _) = coalesced();
    let txn = store.begin().await.unwrap();
    let query = Query::all().order(order).computing(compute);
    txn.aggregate(
        &SecurityContext::superuser(),
        &sales(),
        &query,
        &[aggregate],
    )
    .await
    .unwrap()
    .first()
    .cloned()
    .unwrap()
}

/// A sum is a fold over a set. Reading the same two rows in the other
/// direction must not change it.
#[tokio::test]
async fn a_sum_over_a_mixed_type_computed_value_does_not_depend_on_scan_order() {
    let (_, computed) = coalesced();
    let ascending = total(ScanOrder::Ascending, Aggregate::Sum(computed)).await;
    let descending = total(ScanOrder::Descending, Aggregate::Sum(computed)).await;
    assert_eq!(
        ascending, descending,
        "SUM(coalesce(price, amount)) came back as {ascending:?} reading the rows \
         forwards and {descending:?} reading them backwards"
    );
    // And the answer both should give: 1.5 + 10.
    assert_eq!(ascending, Value::F64(11.5));
}

/// The same for the mean, which divides the same running total.
#[tokio::test]
async fn an_average_over_a_mixed_type_computed_value_does_not_depend_on_scan_order() {
    let (_, computed) = coalesced();
    let ascending = total(ScanOrder::Ascending, Aggregate::Avg(computed)).await;
    let descending = total(ScanOrder::Descending, Aggregate::Avg(computed)).await;
    assert_eq!(
        ascending, descending,
        "AVG(coalesce(price, amount)) came back as {ascending:?} forwards and \
         {descending:?} backwards"
    );
    assert_eq!(ascending, Value::F64(5.75));
}
