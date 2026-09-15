//! Money through the ORM: a decimal column, declared and summed.
//!
//! The kernel's `decimals` suite covers the arithmetic. What is left here is
//! the layer only this crate has: that a Rust field of the right type produces
//! a decimal column with the right scale, and that reading it back gives units
//! rather than something that has been through a float.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Aggregate, Catalog, Grant, Principal, Query, Record, RecordStore, Records,
    SecurityCatalog, SecurityContext, TableId, Units, Value, memory::MemoryStore,
};

const INVOICES: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "invoices", id = 1)]
struct Invoice {
    #[record(pk)]
    id: u64,
    #[record(scale = 2)]
    total: Units,
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

async fn seeded() -> RecordStore<MemoryStore> {
    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([Invoice::table().clone()]).unwrap(),
        SecurityCatalog::new().grant(Grant::new("app", INVOICES, Action::EVERYTHING)),
    );
    let txn = store.begin().await.unwrap();
    // Three amounts that a float would not add exactly: 0.10 + 0.20 is the
    // textbook case, and 19.99 is the one every invoice table has.
    for (id, cents) in [(1u64, 10i64), (2, 20), (3, 1999)] {
        txn.insert_record(
            &context(),
            &Invoice {
                id,
                total: Units(cents),
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    store
}

#[test]
fn the_field_declares_a_decimal_column_with_its_scale() {
    let table = Invoice::table();
    let total = table.column(table.ordinal_of("total").unwrap()).unwrap();
    assert_eq!(total.value_type(), slate_orm::ValueType::Decimal);
    // The scale came from the attribute, not from the type: `Units` is the
    // same Rust type at every scale.
    assert_eq!(total.scale(), Some(2));
}

#[tokio::test]
async fn amounts_round_trip_as_units() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let invoice: Invoice = txn
        .get_record(&context(), &[Value::U64(3)])
        .await
        .unwrap()
        .expect("the row is there");
    txn.rollback();
    assert_eq!(invoice.total, Units(1999));
    // And rendering needs the scale, which is the trade the type makes.
    let scale = Invoice::table()
        .column(Invoice::COLUMNS.total)
        .unwrap()
        .scale()
        .unwrap();
    assert_eq!(invoice.total.to_string_with_scale(scale), "19.99");
}

#[tokio::test]
async fn the_total_is_exact() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let summed = txn
        .aggregate_records::<Invoice>(
            &context(),
            &Query::all(),
            &[Aggregate::Sum(Invoice::COLUMNS.total)],
        )
        .await
        .unwrap();
    txn.rollback();
    // 10 + 20 + 1999 = 2029 cents = $20.29, exactly.
    assert_eq!(summed[0], Value::Decimal(2029), "{summed:?}");
    assert_eq!(Units(2029).to_string_with_scale(2), "20.29");
}

#[test]
fn rendering_handles_a_negative_and_a_bare_fraction() {
    // Both are places a naive `format!("{}.{}", a / d, a % d)` goes wrong: the
    // sign is lost on the whole part when it is zero, and the fractional part
    // loses its leading zeros.
    assert_eq!(Units(-5).to_string_with_scale(2), "-0.05");
    assert_eq!(Units(5).to_string_with_scale(2), "0.05");
    assert_eq!(Units(-1999).to_string_with_scale(2), "-19.99");
    assert_eq!(Units(1250).to_string_with_scale(0), "1250");
    assert_eq!(Units(1250).to_string_with_scale(3), "1.250");
}
