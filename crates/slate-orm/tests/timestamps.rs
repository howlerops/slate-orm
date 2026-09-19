//! `#[record(created_at)]` and `#[record(updated_at)]` through the ORM.
//!
//! The kernel's `managed` suite covers the stamping rules. What is left here is
//! the layer only this crate has: that a Rust field carrying the attribute
//! produces a *managed* column, and that a caller who never mentions either
//! field gets both filled — which is the whole ergonomic claim, and the one
//! that a macro emitting the attribute into the wrong builder call would break
//! while every kernel test stayed green.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, Grant, Managed, Principal, Record, RecordStore, Records, SecurityCatalog,
    SecurityContext, TableId, Value, memory::MemoryStore,
};

use slate_kernel::clock::FixedClock;
use std::sync::Arc;

const NOTES: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "notes", id = 1)]
struct Note {
    #[record(pk)]
    id: u64,
    body: String,
    #[record(created_at)]
    created_at: i64,
    #[record(updated_at)]
    updated_at: i64,
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

fn store_at(seconds: i64) -> (RecordStore<MemoryStore>, FixedClock) {
    let clock = FixedClock::at(seconds);
    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([Note::table().clone()]).unwrap(),
        SecurityCatalog::new().grant(Grant::new("app", NOTES, Action::EVERYTHING)),
    )
    .with_clock(Arc::new(clock.clone()));
    (store, clock)
}

#[test]
fn the_attribute_reaches_the_column() {
    // The macro's own output, before any store exists. A `managed_for` emitted
    // against the wrong column name would pass every behavioural test below by
    // accident on a two-timestamp table, because both stamps would still be
    // written — just to each other's columns, which on equal values is
    // invisible.
    let table = Note::table();
    let managed = |name: &str| {
        table
            .column(table.ordinal_of(name).expect("the column"))
            .expect("the column")
            .managed()
    };
    assert_eq!(managed("created_at"), Some(Managed::CreatedAt));
    assert_eq!(managed("updated_at"), Some(Managed::UpdatedAt));
    assert_eq!(managed("body"), None);
    assert_eq!(managed("id"), None);
}

#[tokio::test]
async fn a_caller_who_names_neither_timestamp_gets_both() {
    let (store, clock) = store_at(1_000);

    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Note {
            id: 1,
            body: "first".to_owned(),
            // Written because the struct requires a value, and ignored. That
            // is the point: there is no way for a caller to mean these.
            created_at: 0,
            updated_at: 0,
        },
    )
    .await
    .expect("the insert");
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let stored: Note = txn
        .get_record(&context(), &[Value::U64(1)])
        .await
        .expect("a read")
        .expect("the row");
    assert_eq!(stored.created_at, 1_000);
    assert_eq!(stored.updated_at, 1_000);

    clock.set(2_000);
    let txn = store.begin().await.unwrap();
    txn.update_record(
        &context(),
        &Note {
            id: 1,
            body: "second".to_owned(),
            created_at: 0,
            updated_at: 0,
        },
    )
    .await
    .expect("the update");
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let stored: Note = txn
        .get_record(&context(), &[Value::U64(1)])
        .await
        .expect("a read")
        .expect("the row");
    assert_eq!(stored.body, "second");
    assert_eq!(stored.created_at, 1_000, "created_at should not move");
    assert_eq!(stored.updated_at, 2_000, "updated_at should move");
}
