//! `#[record(soft_delete)]` through the ORM.
//!
//! The kernel's `soft_delete` suite covers the retirement rules — every read
//! path hiding a retired row, a covering scan not resurrecting one, the
//! privileged read that shows them. What is left here is the layer only this
//! crate has: that a Rust field carrying the attribute produces a table the
//! kernel treats as soft-deleting.
//!
//! Before this, the Rust library could declare a soft-delete column only
//! through `TableDef::builder`, which meant the one surface most likely to be
//! read as "how you declare a table in Rust" could not express the convention
//! at all. Recorded as a gap in
//! `ledger/2026-09-19-a-row-that-is-gone-but-still-there.md`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, Grant, Principal, Record, RecordStore, Records, SecurityCatalog,
    SecurityContext, TableId, Value, memory::MemoryStore,
};

const NOTES: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "notes", id = 1)]
struct Note {
    #[record(pk)]
    id: u64,
    body: String,
    /// Nullable and `i64`, which the schema layer requires and explains: a
    /// non-nullable column has no value meaning "not deleted", so every row
    /// would read as retired the moment the table was declared.
    #[record(soft_delete)]
    deleted_at: Option<i64>,
}

/// A table with no `soft_delete`, to pin the negative half.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "plain", id = 2)]
struct Plain {
    #[record(pk)]
    id: u64,
    deleted_at: Option<i64>,
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

fn store() -> RecordStore<MemoryStore> {
    RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([Note::table().clone(), Plain::table().clone()]).unwrap(),
        SecurityCatalog::new()
            .grant(Grant::new("app", NOTES, Action::EVERYTHING))
            .grant(Grant::new("app", TableId(2), Action::EVERYTHING)),
    )
}

#[test]
fn the_attribute_reaches_the_table() {
    // The macro's own output, before any store exists. This is the assertion
    // that would catch the attribute being parsed and then dropped on the way
    // to the builder — which every behavioural test below would also catch,
    // but only by way of a confusing "the row is gone" rather than by naming
    // the thing that is missing.
    let table = Note::table();
    let ordinal = table.ordinal_of("deleted_at").expect("the column");
    assert_eq!(table.soft_delete(), Some(ordinal));

    // And a column merely *named* `deleted_at` is not one: the convention is
    // the attribute, not the name. A macro that sniffed names would pass every
    // other test here and silently retire rows in somebody's unrelated table.
    assert_eq!(Plain::table().soft_delete(), None);
}

#[tokio::test]
async fn a_delete_retires_the_row_rather_than_erasing_it() {
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Note {
            id: 1,
            body: "first".to_owned(),
            deleted_at: None,
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let removed = txn
        .delete(&context(), Note::table(), &[Value::U64(1)])
        .await
        .unwrap();
    assert!(removed, "the row was there");
    txn.commit().await.unwrap();

    // Hidden from an ordinary read...
    let txn = store.begin().await.unwrap();
    assert!(
        txn.get(&context(), Note::table(), &[Value::U64(1)])
            .await
            .unwrap()
            .is_none(),
        "a retired row is not returned"
    );

    // ...and still there, which is the half that distinguishes this from a
    // hard delete. Asked for through the privileged read rather than by
    // reaching into storage, so the test exercises the path a caller would.
    let mut query = slate_kernel::Query::all();
    query.include_deleted = true;
    let rows = txn
        .execute(&context(), Note::table(), &query)
        .await
        .expect("the `app` role holds every action, ReadDeleted among them")
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "the retired row is still stored");
}

#[tokio::test]
async fn a_table_without_the_attribute_still_deletes_for_real() {
    // The control. Without it, "the row is gone" proves nothing about the
    // attribute, because a hard delete looks identical from an ordinary read.
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Plain {
            id: 1,
            deleted_at: None,
        },
    )
    .await
    .unwrap();
    txn.delete(&context(), Plain::table(), &[Value::U64(1)])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let mut query = slate_kernel::Query::all();
    query.include_deleted = true;
    let rows = txn
        .execute(&context(), Plain::table(), &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "a table with no soft-delete column erases on delete"
    );
}

// --- the two shapes the schema layer refuses ---------------------------------
//
// Both are `should_panic` rather than `compile_fail`, because the macro does
// not check them and deliberately does not: the rules live in
// `TableBuilder::build`, which explains each one, and restating them in the
// macro would put the same rule in two places that can drift. What the macro
// owes is that the refusal *arrives* — a macro that emitted the builder call
// against the wrong column, or swallowed the error, would turn a loud schema
// bug into a table that silently reads empty.

#[derive(Record)]
#[record(table = "bad_nullable", id = 3)]
struct NotNullable {
    #[record(pk)]
    id: u64,
    #[record(soft_delete)]
    deleted_at: i64,
}

#[test]
#[should_panic(expected = "nullable")]
fn a_non_nullable_soft_delete_column_is_refused() {
    // The dangerous shape: with no null there is no value meaning "not
    // deleted", so every row reads as retired and the table goes silently
    // empty. Loud is the only acceptable behaviour.
    let _ = NotNullable::table();
}

#[derive(Record)]
#[record(table = "bad_type", id = 4)]
struct NotATimestamp {
    #[record(pk)]
    id: u64,
    #[record(soft_delete)]
    deleted_at: Option<String>,
}

#[test]
#[should_panic(expected = "bad_type")]
fn a_soft_delete_column_that_is_not_a_timestamp_is_refused() {
    let _ = NotATimestamp::table();
}
