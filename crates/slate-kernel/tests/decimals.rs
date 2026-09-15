//! Exact decimals: money that does not drift.
//!
//! The claim is that a decimal column is exact where an `f64` is not, so the
//! tests are arithmetic rather than plumbing: the same sums, done both ways,
//! with the float one asserted to be *wrong*. A test that only showed the
//! decimal being right would not establish that there was anything to fix.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::float_cmp
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, CmpOp, Expr, Grant, Principal, Query, RecordStore, ScanOrder,
    SecurityCatalog, SecurityContext, migrate,
};
use slate_schema::{Catalog, Ordinal, Row, SchemaError, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const LEDGER: TableId = TableId(1);

/// Amounts in cents, and the same amounts as doubles, side by side.
fn ledger() -> TableDef {
    TableDef::builder("ledger", LEDGER)
        .column("id", ValueType::U64)
        .decimal_column("amount", 2)
        .column("as_double", ValueType::F64)
        .primary_key(["id"])
        .build()
        .unwrap()
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

fn store(table: &TableDef) -> RecordStore<MemoryStore> {
    RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([table.clone()]).unwrap(),
        SecurityCatalog::new().grant(Grant::new("app", LEDGER, Action::EVERYTHING)),
    )
}

/// `0.10` a hundred times: the canonical case where binary floating point
/// cannot represent the addend and the error accumulates.
async fn seeded() -> (RecordStore<MemoryStore>, TableDef) {
    let table = ledger();
    let records = store(&table);
    let txn = records.begin().await.unwrap();
    for id in 1..=100u64 {
        txn.insert(
            &context(),
            &table,
            &Row::new(vec![
                Value::U64(id),
                // 10 cents.
                Value::Decimal(10),
                Value::F64(0.10),
            ]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    (records, table)
}

#[tokio::test]
async fn a_hundred_dimes_are_exactly_ten_dollars() {
    let (records, table) = seeded().await;
    let txn = records.begin().await.unwrap();

    let sums = txn
        .aggregate(
            &context(),
            &table,
            &Query::all(),
            &[Aggregate::Sum(Ordinal(1)), Aggregate::Sum(Ordinal(2))],
        )
        .await
        .unwrap();
    txn.rollback();

    // 100 x 10 cents = 1000 cents = $10.00, exactly, because it is integer
    // addition.
    assert_eq!(sums[0], Value::Decimal(1000), "{sums:?}");

    // And the same money as doubles, which is the reason the column exists.
    // `0.1` has no exact binary representation, so a hundred of them do not
    // add to ten — asserted rather than described, so that if it ever does the
    // test says so instead of quietly agreeing.
    let Value::F64(drifted) = sums[1] else {
        panic!("the double sum is not a double: {sums:?}")
    };
    assert_ne!(
        drifted, 10.0,
        "floating point summed to exactly ten, which would make this whole column pointless"
    );
    assert!((drifted - 10.0).abs() < 1e-9, "but it is close: {drifted}");
    // Printed as well, because the docs quote this pair. The exact digits
    // depend on summation order, so they are shown rather than asserted: the
    // claim is "not ten", and pinning the digits would make a reordering of the
    // fold read as a correctness failure.
    println!("SUM decimal {:?} vs SUM f64 {drifted}", sums[0]);
}

#[tokio::test]
async fn decimals_order_and_range_like_numbers() {
    let table = ledger();
    let records = store(&table);
    let txn = records.begin().await.unwrap();
    // Deliberately spanning zero and crossing a magnitude boundary, which is
    // where the integer encoding's length-prefixed codes change.
    for (id, cents) in [
        (1u64, -250i64),
        (2, -1),
        (3, 0),
        (4, 1),
        (5, 99),
        (6, 100000),
    ] {
        txn.insert(
            &context(),
            &table,
            &Row::new(vec![Value::U64(id), Value::Decimal(cents), Value::F64(0.0)]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = records.begin().await.unwrap();
    // Everything strictly above -0.01.
    let rows = txn
        .query(
            &context(),
            &table,
            Expr::compare(Ordinal(1), CmpOp::Gt, Value::Decimal(-1)),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    txn.rollback();
    let amounts: Vec<i64> = rows
        .iter()
        .map(|row| match row.values()[1] {
            Value::Decimal(v) => v,
            ref other => panic!("not a decimal: {other:?}"),
        })
        .collect();
    assert_eq!(amounts, vec![0, 1, 99, 100_000], "{amounts:?}");
}

#[tokio::test]
async fn a_decimal_is_not_an_integer_with_the_same_units() {
    // They encode alike underneath — a decimal is the integer encoding behind
    // one byte — so this is the property that byte makes true. Without it,
    // `amount = 1250` would match a row holding the integer 1250 in a column
    // of a different type, and a filter written against the wrong column type
    // would silently match.
    assert_ne!(Value::Decimal(1250), Value::I64(1250));
    assert_ne!(Value::Decimal(1250), Value::U64(1250));
    assert!(Value::I64(1250) < Value::Decimal(1250) || Value::Decimal(1250) < Value::I64(1250));
}

#[test]
fn the_column_carries_the_scale_and_nothing_else_does() {
    let table = ledger();
    let amount = table.column(Ordinal(1)).unwrap();
    assert_eq!(amount.scale(), Some(2));
    // Not a decimal, so no scale — rather than a zero a caller could mistake
    // for one.
    assert_eq!(table.column(Ordinal(2)).unwrap().scale(), None);
    assert_eq!(table.column(Ordinal(0)).unwrap().scale(), None);
}

#[test]
fn a_scale_with_no_room_for_a_number_is_refused() {
    let built = TableDef::builder("ledger", LEDGER)
        .column("id", ValueType::U64)
        .decimal_column("amount", 19)
        .primary_key(["id"])
        .build();
    let error = built.expect_err("scale 19 leaves no integral part");
    assert!(
        matches!(error, SchemaError::ScaleTooLarge { .. }),
        "{error}"
    );
    // 18 is the last one that can represent anything above one, so it stands.
    assert!(
        TableDef::builder("ledger", LEDGER)
            .column("id", ValueType::U64)
            .decimal_column("amount", 18)
            .primary_key(["id"])
            .build()
            .is_ok()
    );
}

#[tokio::test]
async fn changing_a_scale_is_a_migration_refusal() {
    let store = MemoryStore::new();
    let catalog = Catalog::from_tables([ledger()]).unwrap();
    migrate::migrate(&store, &catalog).await.unwrap();

    // Same columns, same types, same key — only the scale moves. Every stored
    // row now means something different: units 1250 was 12.50 and is now
    // 1.250. Nothing about the bytes changed, which is exactly why this has to
    // be caught by the fingerprint rather than by a decode failure.
    let rescaled = TableDef::builder("ledger", LEDGER)
        .column("id", ValueType::U64)
        .decimal_column("amount", 3)
        .column("as_double", ValueType::F64)
        .primary_key(["id"])
        .build()
        .unwrap();
    let plan = migrate::plan(&store, &Catalog::from_tables([rescaled]).unwrap())
        .await
        .unwrap();
    assert!(plan.is_blocked(), "a rescale slipped through: {plan:?}");
}
