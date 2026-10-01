//! `#[record(foreign_key(...))]` through the ORM.
//!
//! `TableBuilder::foreign_key` has existed since the schema layer did, and the
//! TOML loader has declared one since 2026-09-19. The derive could declare
//! none, which is the gap
//! `ledger/2026-10-01-a-check-the-derive-could-not-declare.md` recorded while
//! closing the same gap for `check`: a table written in Rust had to be built
//! through `TableDef::builder` to carry a reference, and that means abandoning
//! the derive entirely, because `Record::table()` is what every write hands
//! the store.
//!
//! Three things here are not about the feature and are what the tests are
//! arranged around:
//!
//! - **The self-reference deadlock.** The parent is named as a *type*, and the
//!   obvious way to turn a type into a `TableId` is `<P as Record>::table()
//!   .id()`. That runs inside `Self::table()`'s own `OnceLock` initialiser, so
//!   `employee.manager_id -> employee` re-enters the lock and hangs — not
//!   fails, hangs, with no output. `Record::table_id()` exists for this and
//!   the derive overrides it with the literal from `#[record(id = N)]`;
//!   `an_employee_can_reference_its_own_table` is the test, and it is a test
//!   whose failure mode is a timeout rather than an assertion.
//! - **Column order is load-bearing.** The referencing columns are matched
//!   against the parent's primary key *in the order written*, so a key into a
//!   tenant-scoped parent names the tenant first. Reversing them is not a
//!   compile error and not a schema error — it is a reference that silently
//!   looks up the wrong row — so `the_composite_key_keeps_the_order_it_was_written`
//!   reads the ordinals out rather than counting them.
//! - **`ForeignKeyDef`'s `PartialEq` is a real one**, unlike `CheckDef`'s: it
//!   derives over name, columns, parent and action. So equality against a
//!   hand-written table is worth something here, and the field-by-field
//!   assertion `derive_checks.rs` needs is not repeated.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, ForeignKeyDef, Grant, Ordinal, Principal, Record, RecordStore, Records,
    ReferentialAction, SchemaError, SecurityCatalog, SecurityContext, TableDef, TableId, Value,
    ValueType, memory::MemoryStore,
};

const ACCOUNTS: TableId = TableId(2);
const INVOICES: TableId = TableId(3);
const EMPLOYEES: TableId = TableId(4);

/// A tenant-scoped parent: its primary key is `(tenant_id, id)`.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "accounts", id = 2)]
#[record(tenant = "tenant_id")]
struct Account {
    #[record(pk)]
    tenant_id: u64,
    #[record(pk)]
    id: u64,
    name: String,
}

/// A child of a composite, tenant-scoped key.
///
/// Written on the struct because there are two referencing columns and no one
/// field to hang it on — and `on_delete = cascade`, so deleting an account
/// takes its invoices with it.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "invoices", id = 3)]
#[record(tenant = "tenant_id")]
#[record(foreign_key(
    name = "invoices_account",
    parent = Account,
    // In the parent's primary key order. Reversing these compiles, builds and
    // then looks up a row that is not the one meant.
    column = "tenant_id",
    column = "account",
    on_delete = cascade,
))]
struct Invoice {
    #[record(pk)]
    tenant_id: u64,
    #[record(pk)]
    id: u64,
    #[record(rename = "account")]
    account_id: u64,
    cents: i64,
}

/// The self-reference, and the field-level position.
///
/// `manager_id` points at another row of this same table, which is the case
/// that *hangs* rather than fails if the parent's table id is read through
/// `table()`. It is also renamed, which pins that the field-level default
/// resolves to the column rather than to the field ident: a default filled in
/// before `rename` would name `manager_id`, which this table does not have.
///
/// The column is nullable on purpose. A foreign key is satisfied without a
/// lookup when any referencing column is null — SQL's `MATCH SIMPLE` — which
/// is the only thing that makes a self-reference insertable at all: the first
/// row has no manager.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "employees", id = 4)]
struct Employee {
    #[record(pk)]
    id: u64,
    #[record(rename = "manager")]
    #[record(foreign_key(name = "employees_manager", parent = Employee))]
    manager_id: Option<u64>,
    name: String,
}

/// What a person would have written by hand for `Invoice`.
fn hand_written_invoices() -> TableDef {
    TableDef::builder("invoices", INVOICES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("account", ValueType::U64)
        .column("cents", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .foreign_key(
            ForeignKeyDef::builder("invoices_account", ACCOUNTS)
                .column("tenant_id")
                .column("account")
                .on_delete(ReferentialAction::Cascade),
        )
        .build()
        .expect("hand-written schema is valid")
}

fn context() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_role("app")
            .with_tenant(Value::U64(7)),
    )
}

fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([
        Account::table().clone(),
        Invoice::table().clone(),
        Employee::table().clone(),
    ])
    .expect("the three derived tables form a valid catalog");
    let mut security = SecurityCatalog::new();
    for table in [ACCOUNTS, INVOICES, EMPLOYEES] {
        security = security.grant(Grant::new("app", table, Action::EVERYTHING));
    }
    RecordStore::new(MemoryStore::new(), catalog, security)
}

/// The derived table is the hand-written one.
///
/// Worth asserting as equality here where it was not for checks:
/// `ForeignKeyDef` derives `PartialEq` over every field it has, so this does
/// compare the columns, the parent and the action rather than only the name.
#[test]
fn the_derived_foreign_key_matches_the_hand_written_one() {
    assert_eq!(*Invoice::table(), hand_written_invoices());
}

/// A key written on a field references that field, after `rename`.
///
/// The failure this is about does not reach a test on its own: a default
/// filled in before `rename` names a column the struct does not have, and
/// `validate_foreign_keys` refuses that at expansion. So the assertion that
/// earns its place is the positive one — the key names `manager`, the name
/// the wire and the parent's key matching both use.
#[test]
fn a_field_level_key_names_the_renamed_column() {
    let employees = Employee::table();
    let key = &employees.foreign_keys()[0];
    assert_eq!(key.name(), "employees_manager");
    assert_eq!(key.columns(), &[employees.ordinal_of("manager").unwrap()]);
    assert!(
        employees.ordinal_of("manager_id").is_none(),
        "the field ident is not a column name"
    );

    // Not strictly about the default, and here because the two renames are
    // the same mistake in two positions: `account_id` is renamed and named
    // from the *struct*, where there is no default to get wrong and only a
    // string to get right.
    let invoices = Invoice::table();
    let columns = invoices.foreign_keys()[0].columns();
    assert_eq!(columns[1], invoices.ordinal_of("account").unwrap());
}

/// The composite key kept the order it was written.
///
/// Read as ordinals rather than as a length, because a reversed pair is the
/// failure this is about and it has the right length. `tenant_id` is
/// `Ordinal(0)` and `account` is `Ordinal(2)`, so a swap is visible.
#[test]
fn the_composite_key_keeps_the_order_it_was_written() {
    let key = &Invoice::table().foreign_keys()[0];
    assert_eq!(key.columns(), &[Ordinal(0), Ordinal(2)]);
    assert_eq!(key.on_delete(), ReferentialAction::Cascade);

    // And the default is `Restrict`, which is what makes writing nothing the
    // safe thing to write.
    assert_eq!(
        Employee::table().foreign_keys()[0].on_delete(),
        ReferentialAction::Restrict
    );
}

/// A table can reference itself.
///
/// The assertion is almost beside the point: if the parent's id were resolved
/// through `Record::table()`, this test would never reach an assertion at all
/// — `OnceLock::get_or_init` re-entered from inside its own initialiser does
/// not return. A reader seeing this fail should check `Record::table_id()`
/// before anything else.
#[test]
fn an_employee_can_reference_its_own_table() {
    let key = &Employee::table().foreign_keys()[0];
    assert_eq!(key.parent(), EMPLOYEES);
    assert_eq!(key.name(), "employees_manager");
}

/// End to end: a reference to a row that does not exist is refused, and the
/// refusal names the key.
#[tokio::test]
async fn an_invoice_for_an_account_that_does_not_exist_is_refused() {
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Account {
            tenant_id: 7,
            id: 1,
            name: "acme".to_owned(),
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let error = txn
        .insert_record(
            &context(),
            &Invoice {
                tenant_id: 7,
                id: 1,
                account_id: 99,
                cents: 100,
            },
        )
        .await
        .expect_err("account 99 does not exist");

    // On the typed failure rather than the sentence, for the reason
    // `derive_checks.rs` gives: `is_err()` also passes when the row is refused
    // for having the wrong number of columns.
    let message = error.to_string();
    let SchemaError::ForeignKeyViolation { foreign_key, .. } = schema_error(&error) else {
        panic!("expected a foreign key violation, got: {message}");
    };
    assert_eq!(foreign_key, "invoices_account");

    // The control: the same row against the account that does exist.
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Invoice {
            tenant_id: 7,
            id: 1,
            account_id: 1,
            cents: 100,
        },
    )
    .await
    .expect("account 1 exists");
    txn.commit().await.unwrap();
}

/// `on_delete = cascade` reached the kernel.
///
/// The derived action is a one-word attribute and the two words differ by a
/// refusal, so this deletes the parent and counts what is left rather than
/// reading the action back — reading it back is
/// `the_composite_key_keeps_the_order_it_was_written`, and that assertion
/// passes whether or not the kernel ever sees the value.
#[tokio::test]
async fn deleting_an_account_cascades_to_its_invoices() {
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Account {
            tenant_id: 7,
            id: 1,
            name: "acme".to_owned(),
        },
    )
    .await
    .unwrap();
    for id in 1..=3 {
        txn.insert_record(
            &context(),
            &Invoice {
                tenant_id: 7,
                id,
                account_id: 1,
                cents: 100,
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.delete_record::<Account>(&context(), &[Value::U64(7), Value::U64(1)])
        .await
        .expect("cascade, not restrict");
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let left = txn
        .count_records::<Invoice>(&context(), &slate_orm::Query::all())
        .await
        .unwrap();
    txn.rollback();
    assert_eq!(
        left, 0,
        "the cascade should have taken all three invoices, {left} left"
    );
}

/// The self-reference, through the write path.
///
/// `an_employee_can_reference_its_own_table` reads the definition back and
/// proves only that the expansion terminated. This one proves the kernel
/// enforces it against the same table: the root has no manager (null, so the
/// key is satisfied without a lookup), a report naming the root is accepted,
/// and a report naming nobody is refused.
#[tokio::test]
async fn a_self_reference_is_enforced_against_its_own_table() {
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Employee {
            id: 1,
            manager_id: None,
            name: "root".to_owned(),
        },
    )
    .await
    .expect("a null reference is satisfied without a lookup");
    txn.insert_record(
        &context(),
        &Employee {
            id: 2,
            manager_id: Some(1),
            name: "report".to_owned(),
        },
    )
    .await
    .expect("employee 1 exists");
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let error = txn
        .insert_record(
            &context(),
            &Employee {
                id: 3,
                manager_id: Some(99),
                name: "orphan".to_owned(),
            },
        )
        .await
        .expect_err("employee 99 does not exist");
    let message = error.to_string();
    let SchemaError::ForeignKeyViolation { foreign_key, .. } = schema_error(&error) else {
        panic!("expected a foreign key violation, got: {message}");
    };
    assert_eq!(foreign_key, "employees_manager");
}

/// Dig the `SchemaError` out of whatever the ORM wrapped it in.
fn schema_error(error: &slate_orm::OrmError) -> &SchemaError {
    match error {
        slate_orm::OrmError::Kernel(slate_orm::KernelError::Schema(schema)) => schema,
        other => panic!("expected a schema error, got: {other:?}"),
    }
}
