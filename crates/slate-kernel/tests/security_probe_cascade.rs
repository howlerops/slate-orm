//! Probes of the write path's disclosure and reach, for a security review.
//!
//! Two things are asked here that the existing tests do not ask:
//!
//! 1. Does a `CASCADE` from a shared (non-tenant-scoped) parent stay inside the
//!    deleting caller's tenant? `record.rs` says the search "is confined to the
//!    caller's own tenant by the key encoding"; this checks the claim on a
//!    schema where the key encoding does not carry the tenant.
//! 2. Can a write be used to probe for the existence of a row the caller's
//!    policy hides? `security.rs` says a write against a hidden row "reports
//!    the row as missing rather than as forbidden, so the error cannot be used
//!    to probe for existence".

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, Expr, Grant, KernelError, Policy, Principal, RecordStore, SecurityCatalog,
    SecurityContext, memory::MemoryStore,
};
use slate_schema::SchemaError;
use slate_schema::{Catalog, ForeignKeyDef, ReferentialAction, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const ORGS: TableId = TableId(1);
const DOCS: TableId = TableId(2);

const TENANT_A: u64 = 10;
const TENANT_B: u64 = 20;

/// A shared reference table, not tenant-scoped: the "plans", "regions" or
/// "categories" table every multi-tenant schema has.
fn orgs() -> TableDef {
    TableDef::builder("orgs", ORGS)
        .column("org_id", ValueType::U64)
        .column("label", ValueType::Str)
        .primary_key(["org_id"])
        .build()
        .expect("valid schema")
}

/// A tenant-scoped table referencing the shared one.
fn docs(action: ReferentialAction) -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("org_id", ValueType::U64)
        .column("title", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .foreign_key(
            ForeignKeyDef::builder("docs_org", ORGS)
                .column("org_id")
                .on_delete(action),
        )
        .build()
        .expect("valid schema")
}

fn caller(tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(tenant))
            .with_tenant(Value::U64(tenant))
            .with_role("app"),
    )
}

// `security()` and `seeded()` used to live here: they built a store over
// `[orgs(), docs(action)]` so the two cascade findings could be exercised
// against real rows. That catalog is refused now, so `seeded` would panic
// rather than run — dead and broken rather than merely dead, which is why they
// are gone instead of carrying an `allow(dead_code)`.

/// FIXED: the schema that made a cross-tenant cascade possible is refused.
///
/// A shared parent — no tenant column — with a tenant-scoped child means the
/// child rows live in every tenant, and the closure walk runs as a superuser
/// because referential integrity cannot depend on who is asking. So one
/// tenant's delete reached all of them. Measured before the fix: tenant A
/// deleting org 1 left the `docs` table empty, tenant B's row included, and
/// with `Restrict` the delete was refused in a way that disclosed B's row.
///
/// Refused at `Catalog::from_tables` rather than confined at the scan.
/// Confining the walk stops the destruction and leaves other tenants' children
/// pointing at a parent that is gone, trading a security hole for a
/// correctness one. The edge is not expressible safely by either action, so
/// the catalog says so at startup and names both tables.
#[test]
fn a_shared_parent_with_a_tenant_scoped_child_is_refused() {
    for action in [ReferentialAction::Cascade, ReferentialAction::Restrict] {
        let refused = Catalog::from_tables([orgs(), docs(action)]);
        let Err(SchemaError::CrossTenantForeignKey {
            table,
            parent,
            action: reported,
            ..
        }) = refused
        else {
            panic!("ON DELETE {action:?} from a shared parent was accepted");
        };
        assert_eq!(table, "docs");
        assert_eq!(parent, "orgs");
        assert_eq!(reported, action);
    }
}

// --- the write-side existence oracle ---------------------------------------

const NOTES: TableId = TableId(3);

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("owner_id", ValueType::U64)
        .column("body", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn owner_policy() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("app", NOTES, Action::ALL))
        .policy(Policy::new(
            "own_notes",
            NOTES,
            Action::ALL,
            |ctx: &SecurityContext| Expr::eq(slate_schema::Ordinal(2), ctx.principal().id.clone()),
        ))
}

fn person(id: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(id))
            .with_tenant(Value::U64(TENANT_A))
            .with_role("app"),
    )
}

/// FINDING: `insert` distinguishes "a row your policy hides is here" from
/// "this key is free", which `security.rs` says a write must not do.
#[tokio::test]
async fn an_insert_reports_whether_a_hidden_row_occupies_the_key() {
    let catalog = Catalog::from_tables([notes()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, owner_policy());
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    // Bob owns note 7. Alice cannot read it.
    txn.insert(
        &root,
        &notes(),
        &Row::new(vec![
            Value::U64(TENANT_A),
            Value::U64(7),
            Value::U64(200),
            Value::Str("bob's".into()),
        ]),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // Alice confirms she cannot see it.
    let txn = store.begin().await.unwrap();
    let seen = txn
        .get(
            &person(100),
            &notes(),
            &[Value::U64(TENANT_A), Value::U64(7)],
        )
        .await
        .unwrap();
    assert!(seen.is_none(), "the policy should hide Bob's note");

    // ...and then learns it is there anyway, by trying to take the key.
    let taken = txn
        .insert(
            &person(100),
            &notes(),
            &Row::new(vec![
                Value::U64(TENANT_A),
                Value::U64(7),
                Value::U64(100),
                Value::Str("alice's".into()),
            ]),
        )
        .await;
    let free = txn
        .insert(
            &person(100),
            &notes(),
            &Row::new(vec![
                Value::U64(TENANT_A),
                Value::U64(8),
                Value::U64(100),
                Value::Str("alice's".into()),
            ]),
        )
        .await;

    assert!(
        matches!(taken, Err(KernelError::DuplicatePrimaryKey { .. })),
        "expected the oracle to fire, got {taken:?}"
    );
    assert!(free.is_ok(), "the free key should have been insertable");
}

// --- the bulk write path's cross-tenant oracle ------------------------------
//
// `insert` (single row) runs `check_row` — the tenant restriction and the
// policy's WITH CHECK — *before* it reads storage, so its duplicate-key error
// can only ever be about the caller's own tenant. `write_many` runs the same
// check *after* the reads, so the errors it can raise first are about rows the
// caller may not see at all.

const USERS: TableId = TableId(4);

fn users() -> TableDef {
    TableDef::builder("users", USERS)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            slate_schema::IndexDef::builder("by_email", slate_schema::IndexId(10))
                .column("email")
                .unique(),
        )
        .build()
        .expect("valid schema")
}

fn user_security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("app", USERS, Action::ALL))
}

async fn users_store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([users()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, user_security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    txn.insert(
        &root,
        &users(),
        &Row::new(vec![
            Value::U64(TENANT_B),
            Value::U64(7),
            Value::Str("secret@b.example".into()),
        ]),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    store
}

fn user_row(tenant: u64, id: u64, email: &str) -> Row {
    Row::new(vec![
        Value::U64(tenant),
        Value::U64(id),
        Value::Str(email.to_owned()),
    ])
}

/// FINDING: `insert_many` tells tenant A which primary keys exist in tenant B.
///
/// Nothing is written either way — both calls fail — so the probe is free and
/// repeatable.
#[tokio::test]
async fn insert_many_discloses_another_tenants_primary_keys() {
    let store = users_store().await;
    let a = caller(TENANT_A);

    let txn = store.begin().await.unwrap();
    let occupied = txn
        .insert_many(&a, &users(), &[user_row(TENANT_B, 7, "x@a.example")])
        .await
        .unwrap_err();
    let free = txn
        .insert_many(&a, &users(), &[user_row(TENANT_B, 8, "y@a.example")])
        .await
        .unwrap_err();
    txn.rollback();

    // FIXED. Both are the policy refusal now, because `write_many` decides
    // `WITH CHECK` before it reads anything, exactly as single-row `insert`
    // does. The occupied key and the free one are indistinguishable, so there
    // is nothing to read the boundary off.
    assert!(
        matches!(occupied, KernelError::RowCheckFailed { .. }),
        "a taken key in another tenant still answers differently: {occupied:?}"
    );
    assert!(
        matches!(free, KernelError::RowCheckFailed { .. }),
        "expected the policy refusal, got {free:?}"
    );
    assert_eq!(
        core::mem::discriminant(&occupied),
        core::mem::discriminant(&free),
        "the two answers are distinguishable, which is the oracle"
    );
}

/// FINDING: `insert_many` tells tenant A which *unique index values* exist in
/// tenant B — an email address, not just a key.
#[tokio::test]
async fn insert_many_discloses_another_tenants_unique_values() {
    let store = users_store().await;
    let a = caller(TENANT_A);

    let txn = store.begin().await.unwrap();
    // A key that is free in tenant B, carrying an email that is taken there.
    let taken = txn
        .insert_many(&a, &users(), &[user_row(TENANT_B, 99, "secret@b.example")])
        .await
        .unwrap_err();
    let untaken = txn
        .insert_many(&a, &users(), &[user_row(TENANT_B, 99, "nobody@b.example")])
        .await
        .unwrap_err();
    txn.rollback();

    // FIXED. The unique-index read no longer happens before the policy, so a
    // taken email in another tenant is the same refusal as an untaken one.
    // This was the sharper half of the finding: a key is guessable, an email
    // address is the data.
    assert!(
        matches!(taken, KernelError::RowCheckFailed { .. }),
        "a taken unique value in another tenant still answers differently: {taken:?}"
    );
    assert!(
        matches!(untaken, KernelError::RowCheckFailed { .. }),
        "expected the policy refusal, got {untaken:?}"
    );
    assert_eq!(
        core::mem::discriminant(&taken),
        core::mem::discriminant(&untaken),
        "the two answers are distinguishable, which is the oracle"
    );
}

/// The single-row path, as a control: it checks the policy first, so both
/// answers are the same refusal.
#[tokio::test]
async fn the_single_row_insert_gives_the_same_answer_both_ways() {
    let store = users_store().await;
    let a = caller(TENANT_A);

    let txn = store.begin().await.unwrap();
    let occupied = txn
        .insert(&a, &users(), &user_row(TENANT_B, 7, "x@a.example"))
        .await
        .unwrap_err();
    let free = txn
        .insert(&a, &users(), &user_row(TENANT_B, 8, "y@a.example"))
        .await
        .unwrap_err();
    txn.rollback();

    assert_eq!(
        core::mem::discriminant(&occupied),
        core::mem::discriminant(&free),
        "the single-row path leaked too: {occupied:?} vs {free:?}"
    );
}

/// FINDING: `upsert_many` has the same shape.
#[tokio::test]
async fn upsert_many_discloses_another_tenants_primary_keys() {
    let store = users_store().await;
    let a = caller(TENANT_A);

    let txn = store.begin().await.unwrap();
    let occupied = txn
        .upsert_many(&a, &users(), &[user_row(TENANT_B, 7, "x@a.example")])
        .await
        .unwrap_err();
    let free = txn
        .upsert_many(&a, &users(), &[user_row(TENANT_B, 8, "y@a.example")])
        .await
        .unwrap_err();
    txn.rollback();

    // FIXED. `Action::Insert`'s check runs before the reads for an upsert too:
    // the tenant restriction is the same expression either way, so a row
    // outside the caller's tenant is refused without reading anything.
    assert!(
        matches!(occupied, KernelError::RowCheckFailed { .. }),
        "an occupied key in another tenant still answers differently: {occupied:?}"
    );
    assert!(
        matches!(free, KernelError::RowCheckFailed { .. }),
        "got {free:?}"
    );
    assert_eq!(
        core::mem::discriminant(&occupied),
        core::mem::discriminant(&free),
        "the two answers are distinguishable, which is the oracle"
    );
}

/// `update_many`, as a control: both answers are `RowNotFound`.
#[tokio::test]
async fn update_many_gives_the_same_answer_both_ways() {
    let store = users_store().await;
    let a = caller(TENANT_A);

    let txn = store.begin().await.unwrap();
    let occupied = txn
        .update_many(&a, &users(), &[user_row(TENANT_B, 7, "x@a.example")])
        .await
        .unwrap_err();
    let free = txn
        .update_many(&a, &users(), &[user_row(TENANT_B, 8, "y@a.example")])
        .await
        .unwrap_err();
    txn.rollback();

    assert_eq!(
        core::mem::discriminant(&occupied),
        core::mem::discriminant(&free),
        "update_many leaked: {occupied:?} vs {free:?}"
    );
}
