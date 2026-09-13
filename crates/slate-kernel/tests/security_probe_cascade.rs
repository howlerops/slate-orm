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
    Action, Expr, Grant, KernelError, Policy, Principal, Query, RecordStore, SecurityCatalog,
    SecurityContext, memory::MemoryStore,
};
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

fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("app", ORGS, Action::ALL))
        .grant(Grant::new("app", DOCS, Action::ALL))
}

fn caller(tenant: u64) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(tenant))
            .with_tenant(Value::U64(tenant))
            .with_role("app"),
    )
}

async fn seeded(action: ReferentialAction) -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([orgs(), docs(action)]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    txn.insert(
        &root,
        &orgs(),
        &Row::new(vec![Value::U64(1), Value::Str("shared".into())]),
    )
    .await
    .unwrap();
    for (tenant, id, title) in [(TENANT_A, 1u64, "a's doc"), (TENANT_B, 2u64, "b's doc")] {
        txn.insert(
            &root,
            &docs(action),
            &Row::new(vec![
                Value::U64(tenant),
                Value::U64(id),
                Value::U64(1),
                Value::Str(title.into()),
            ]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    store
}

/// FINDING: a cascade from a shared parent deletes another tenant's rows.
#[tokio::test]
async fn a_cascade_from_a_shared_parent_crosses_the_tenant_boundary() {
    let action = ReferentialAction::Cascade;
    let store = seeded(action).await;

    let txn = store.begin().await.unwrap();
    let deleted = txn
        .delete(&caller(TENANT_A), &orgs(), &[Value::U64(1)])
        .await
        .unwrap();
    assert!(deleted, "the org should have been deleted");
    txn.commit().await.unwrap();

    // What is left, seen by a superuser so the policy cannot hide the answer.
    let txn = store.begin().await.unwrap();
    let left = txn
        .execute(&SecurityContext::superuser(), &docs(action), &Query::all())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let titles: Vec<String> = left
        .iter()
        .map(|r| match r.get(slate_schema::Ordinal(3)) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    // The finding, asserted as it actually behaves: tenant B's row is gone.
    // Change this to `vec!["b's doc"]` once the cascade is confined.
    assert_eq!(
        titles,
        Vec::<String>::new(),
        "tenant A's delete of a shared parent left {titles:?}; \
         if this now holds `b's doc` the cascade has been confined and the finding is fixed"
    );
}

/// FINDING: a `RESTRICT` refusal tells tenant A that tenant B has a row.
#[tokio::test]
async fn a_restrict_refusal_discloses_another_tenants_row() {
    let action = ReferentialAction::Restrict;
    let store = seeded(action).await;

    // First, tenant A deletes its own referencing row, so that the only thing
    // left pointing at the org belongs to tenant B.
    let txn = store.begin().await.unwrap();
    txn.delete(
        &caller(TENANT_A),
        &docs(action),
        &[Value::U64(TENANT_A), Value::U64(1)],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let outcome = txn
        .delete(&caller(TENANT_A), &orgs(), &[Value::U64(1)])
        .await;
    assert!(
        outcome.is_err(),
        "tenant A saw no reason not to delete the org; \
         if this passes the RESTRICT does not fire and there is nothing to disclose"
    );
    let error = outcome.unwrap_err().to_string();
    assert!(
        error.to_lowercase().contains("referenc"),
        "expected a referential refusal, got {error}"
    );
    // The refusal is the disclosure: nothing tenant A can read references the
    // org, and yet the delete is refused.
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

    assert!(
        matches!(occupied, KernelError::DuplicatePrimaryKey { .. }),
        "expected the key-taken answer, got {occupied:?}"
    );
    assert!(
        matches!(free, KernelError::RowCheckFailed { .. }),
        "expected the policy refusal, got {free:?}"
    );
    assert_ne!(
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

    assert!(
        matches!(taken, KernelError::UniqueViolation { .. }),
        "expected the unique-index answer, got {taken:?}"
    );
    assert!(
        matches!(untaken, KernelError::RowCheckFailed { .. }),
        "expected the policy refusal, got {untaken:?}"
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

    assert!(
        matches!(occupied, KernelError::RowNotFound { .. }),
        "got {occupied:?}"
    );
    assert!(
        matches!(free, KernelError::RowCheckFailed { .. }),
        "got {free:?}"
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
