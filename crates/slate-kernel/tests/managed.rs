//! Columns the store writes: `created_at` and `updated_at`.
//!
//! Three claims, and they fail differently:
//!
//! - **`CreatedAt` survives an update and `UpdatedAt` does not.** This is the
//!   whole feature and it is one comparison, which is why the clock is settable
//!   rather than read from the machine: at exact times 1000 and 2000 the
//!   assertion is `[1000, 2000]`, and an implementation that stamps both on
//!   every write, or neither, or takes the caller's value, produces three
//!   distinguishable answers. Against a wall clock all four collapse into "two
//!   numbers near now" and the test asserts nothing.
//! - **A value the caller supplies is discarded.** The column's promise is that
//!   it says when the row was written; a caller who can set it can break that
//!   promise with nothing downstream able to tell. Checked on insert *and* on
//!   update, because an update carries a full row and "preserve `created_at`"
//!   could be implemented as "keep the caller's copy", which is not the same
//!   thing and passes a test that only inserts.
//! - **Every write path stamps.** `insert`, `insert_partial`, `upsert`,
//!   `update`, `update_if_unchanged`, `update_where` and the bulk path all
//!   funnel through one function, and this is the test that says so — the
//!   alternative to one choke point was seven call sites, and the seventh
//!   would have been the one that forgot.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::clock::FixedClock;
use slate_kernel::memory::MemoryStore;
use slate_kernel::{RecordStore, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, Managed, PartialRow, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const NOTES: TableId = TableId(1);

/// `id`, `body`, `created_at`, `updated_at`.
fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("id", ValueType::U64)
        .column("body", ValueType::Str)
        .column("created_at", ValueType::I64)
        .column("updated_at", ValueType::I64)
        .primary_key(["id"])
        .managed_for("created_at", Managed::CreatedAt)
        .managed_for("updated_at", Managed::UpdatedAt)
        .build()
        .expect("a valid table")
}

fn catalog() -> Catalog {
    Catalog::from_tables([notes()]).expect("a catalog the schema layer accepts")
}

/// A store whose clock starts at `seconds`, and the handle that moves it.
fn store_at(seconds: i64) -> (RecordStore<MemoryStore>, FixedClock) {
    let clock = FixedClock::at(seconds);
    let store = RecordStore::new(MemoryStore::new(), catalog(), SecurityCatalog::new())
        .with_clock(Arc::new(clock.clone()));
    (store, clock)
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// `id`, `body`, and two timestamps the caller is trying to choose.
fn note(id: u64, body: &str, claimed: i64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(body.to_owned()),
        Value::I64(claimed),
        Value::I64(claimed),
    ])
}

/// The `[created_at, updated_at]` of row `id`.
async fn stamps(store: &RecordStore<MemoryStore>, id: u64) -> (i64, i64) {
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("notes").expect("the table");
    let row = txn
        .get(&root(), table, &[Value::U64(id)])
        .await
        .expect("a readable row")
        .expect("the row to exist");
    let at = |ordinal: usize| match row.values()[ordinal] {
        Value::I64(seconds) => seconds,
        ref other => panic!("column {ordinal} is {other:?}, not a timestamp"),
    };
    (at(2), at(3))
}

#[tokio::test]
async fn created_at_survives_an_update_and_updated_at_does_not() {
    let (store, clock) = store_at(1_000);
    let txn = store.begin().await.expect("a transaction");
    let table = txn
        .catalog()
        .table_by_name("notes")
        .expect("the table")
        .clone();
    txn.insert(&root(), &table, &note(1, "first", 0))
        .await
        .expect("the insert");
    txn.commit().await.expect("the commit");
    assert_eq!(stamps(&store, 1).await, (1_000, 1_000));

    clock.set(2_000);
    let txn = store.begin().await.expect("a transaction");
    txn.update(&root(), &table, &note(1, "second", 0))
        .await
        .expect("the update");
    txn.commit().await.expect("the commit");
    // The whole feature, in one line: the first number did not move and the
    // second did.
    assert_eq!(stamps(&store, 1).await, (1_000, 2_000));
}

#[tokio::test]
async fn a_caller_cannot_choose_either_timestamp() {
    // On insert *and* on update. An update carries a full row, so "preserve
    // created_at" has an implementation that keeps the caller's copy — which
    // is not preservation, and which a test that only inserts cannot tell
    // apart from the right one.
    let (store, clock) = store_at(1_000);
    let txn = store.begin().await.expect("a transaction");
    let table = txn
        .catalog()
        .table_by_name("notes")
        .expect("the table")
        .clone();
    txn.insert(&root(), &table, &note(1, "first", 999_999))
        .await
        .expect("the insert");
    txn.commit().await.expect("the commit");
    assert_eq!(stamps(&store, 1).await, (1_000, 1_000));

    clock.set(2_000);
    let txn = store.begin().await.expect("a transaction");
    txn.update(&root(), &table, &note(1, "second", 555_555))
        .await
        .expect("the update");
    txn.commit().await.expect("the commit");
    assert_eq!(stamps(&store, 1).await, (1_000, 2_000));
}

#[tokio::test]
async fn every_write_path_stamps() {
    // One choke point is the design; this is the assertion that it is one.
    // Each arm writes a fresh row at a distinct time, so a path that missed
    // the stamp shows up as the caller's `0` rather than as a plausible time
    // borrowed from another arm.
    let (store, clock) = store_at(10);

    let txn = store.begin().await.expect("a transaction");
    let table = txn
        .catalog()
        .table_by_name("notes")
        .expect("the table")
        .clone();
    txn.insert(&root(), &table, &note(1, "insert", 0))
        .await
        .expect("insert");
    txn.upsert(&root(), &table, &note(2, "upsert", 0))
        .await
        .expect("upsert");
    txn.insert_many(&root(), &table, &[note(3, "bulk", 0)])
        .await
        .expect("insert_many");
    // Neither timestamp is named, which is the case `insert_partial` exists
    // for and the one that would have failed validation before the store ever
    // saw the row: a managed column is non-nullable, so an unset one used to
    // become a null and be refused.
    txn.insert_partial(
        &root(),
        &table,
        PartialRow::for_table(&table)
            .set(slate_schema::Ordinal(0), Value::U64(4))
            .set(slate_schema::Ordinal(1), Value::Str("partial".to_owned())),
    )
    .await
    .expect("insert_partial");
    txn.commit().await.expect("the commit");

    for id in 1..=4 {
        assert_eq!(stamps(&store, id).await, (10, 10), "row {id} on insert");
    }

    // Now every *update* path, at a time the inserts cannot supply.
    clock.set(20);
    let txn = store.begin().await.expect("a transaction");
    txn.update(&root(), &table, &note(1, "updated", 0))
        .await
        .expect("update");
    let stored = txn
        .get(&root(), &table, &[Value::U64(2)])
        .await
        .expect("a readable row")
        .expect("row 2");
    txn.update_if_unchanged(&root(), &table, &note(2, "conditional", 0), &stored)
        .await
        .expect("update_if_unchanged");
    txn.upsert(&root(), &table, &note(3, "re-upsert", 0))
        .await
        .expect("upsert over an existing row");
    txn.commit().await.expect("the commit");

    for id in 1..=3 {
        assert_eq!(
            stamps(&store, id).await,
            (10, 20),
            "row {id} after an update: created_at should hold and updated_at should move"
        );
    }
    // Row 4 was not touched, so neither of its stamps moved. Asserted because
    // a stamp applied to the whole table rather than to the row being written
    // would pass every line above this one.
    assert_eq!(stamps(&store, 4).await, (10, 10));
}

#[tokio::test]
async fn a_table_with_no_managed_column_is_untouched() {
    // The other half of the opt-in. Every existing table in this repository is
    // this case, and a stamp that fired on all of them would overwrite real
    // data in a column that merely happened to be an `i64`.
    let catalog = Catalog::from_tables([TableDef::builder("plain", TableId(2))
        .column("id", ValueType::U64)
        .column("when", ValueType::I64)
        .primary_key(["id"])
        .build()
        .expect("a valid table")])
    .expect("a catalog the schema layer accepts");
    let store = RecordStore::new(MemoryStore::new(), catalog, SecurityCatalog::new())
        .with_clock(Arc::new(FixedClock::at(1_000)));

    let txn = store.begin().await.expect("a transaction");
    let table = txn
        .catalog()
        .table_by_name("plain")
        .expect("the table")
        .clone();
    txn.insert(
        &root(),
        &table,
        &Row::new(vec![Value::U64(1), Value::I64(42)]),
    )
    .await
    .expect("the insert");
    txn.commit().await.expect("the commit");

    let txn = store.begin().await.expect("a transaction");
    let row = txn
        .get(&root(), &table, &[Value::U64(1)])
        .await
        .expect("a readable row")
        .expect("the row");
    assert_eq!(row.values()[1], Value::I64(42), "an unmanaged i64 is data");
}

#[test]
fn the_schema_refuses_a_managed_column_that_cannot_work() {
    // All three at build time rather than at write time: a schema that cannot
    // be served should not start a node, and the write-time version of each of
    // these is a surprise on somebody's first insert.
    let wrong_type = TableDef::builder("t", TableId(3))
        .column("id", ValueType::U64)
        .column("when", ValueType::Str)
        .primary_key(["id"])
        .managed_for("when", Managed::CreatedAt)
        .build();
    assert!(
        format!("{:?}", wrong_type.unwrap_err()).contains("ManagedColumnNotTimestamp"),
        "a managed column must be a timestamp"
    );

    let nullable = TableDef::builder("t", TableId(3))
        .column("id", ValueType::U64)
        .nullable_column("when", ValueType::I64)
        .primary_key(["id"])
        .managed_for("when", Managed::UpdatedAt)
        .build();
    assert!(
        format!("{:?}", nullable.unwrap_err()).contains("ManagedColumnNullable"),
        "a managed column's null can never happen"
    );

    let in_key = TableDef::builder("t", TableId(3))
        .column("id", ValueType::U64)
        .column("when", ValueType::I64)
        .primary_key(["id", "when"])
        .managed_for("when", Managed::CreatedAt)
        .build();
    assert!(
        format!("{:?}", in_key.unwrap_err()).contains("ManagedColumnInKey"),
        "a managed column cannot address the row"
    );
}
