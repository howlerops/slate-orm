//! Row-level security and role-based access control.
//!
//! These tests are about what a caller *cannot* do. The interesting cases are
//! the ones where a plausible implementation leaks: probing for hidden rows via
//! error codes, escaping a policy by updating your way out of it, reaching
//! another tenant by asking for them explicitly, and null-valued columns
//! turning a negated policy inside out.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, Expr, Grant, KernelError, Policy, Principal, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use uuid::Uuid;

const DOCUMENTS: TableId = TableId(1);
const TENANT_A: u128 = 10;
const TENANT_B: u128 = 20;
const ALICE: u128 = 100;
const BOB: u128 = 200;

fn documents() -> TableDef {
    TableDef::builder("documents", DOCUMENTS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .nullable_column("owner_id", ValueType::Uuid)
        .column("title", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_owner", IndexId(10)).column("owner_id"))
        .build()
        .expect("valid schema")
}

fn owner_ordinal() -> slate_schema::Ordinal {
    documents().ordinal_of("owner_id").unwrap()
}

/// Readers see only documents they own; admins see everything in their tenant.
fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("reader", DOCUMENTS, [Action::Read]))
        .grant(Grant::new("author", DOCUMENTS, Action::ALL))
        .grant(Grant::new("admin", DOCUMENTS, Action::ALL))
        .policy(Policy::new(
            "own_documents",
            DOCUMENTS,
            Action::ALL,
            |ctx: &SecurityContext| Expr::eq(owner_ordinal(), ctx.principal().id.clone()),
        ))
        .policy(
            Policy::new(
                "admins_see_all",
                DOCUMENTS,
                Action::ALL,
                |_: &SecurityContext| Expr::True,
            )
            .for_role("admin"),
        )
}

fn store(security: SecurityCatalog) -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([documents()]).expect("catalog");
    RecordStore::new(MemoryStore::new(), catalog, security)
}

fn doc(tenant: u128, id: u64, owner: Option<u128>, title: &str) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::U64(id),
        owner.map_or(Value::Null, |o| Value::Uuid(Uuid::from_u128(o))),
        Value::Str(title.to_owned()),
    ])
}

fn pk(tenant: u128, id: u64) -> Vec<Value> {
    vec![Value::Uuid(Uuid::from_u128(tenant)), Value::U64(id)]
}

fn user(id: u128, tenant: u128, role: &str) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::Uuid(Uuid::from_u128(id)))
            .with_tenant(Value::Uuid(Uuid::from_u128(tenant)))
            .with_role(role),
    )
}

/// Seed a fixed corpus as superuser.
async fn seeded(security: SecurityCatalog) -> RecordStore<MemoryStore> {
    let store = store(security);
    let table = documents();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for row in [
        doc(TENANT_A, 1, Some(ALICE), "alice one"),
        doc(TENANT_A, 2, Some(ALICE), "alice two"),
        doc(TENANT_A, 3, Some(BOB), "bob one"),
        doc(TENANT_A, 4, None, "unowned"),
        doc(TENANT_B, 1, Some(ALICE), "other tenant"),
    ] {
        txn.insert(&root, &table, &row).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

async fn titles(
    store: &RecordStore<MemoryStore>,
    ctx: &SecurityContext,
    filter: Expr,
) -> Result<Vec<String>, KernelError> {
    let table = documents();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(ctx, &table, filter, ScanOrder::Ascending)
        .await?
        .collect()
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| match &r.values()[3] {
            Value::Str(s) => s.clone(),
            other => panic!("title was {other:?}"),
        })
        .collect())
}

/// A store with no rules denies everything. The default has to fail closed, or
/// every table is public until someone remembers to lock it.
#[tokio::test]
async fn an_empty_security_catalog_denies_everything() {
    let store = seeded(SecurityCatalog::new()).await;
    let ctx = user(ALICE, TENANT_A, "reader");
    let err = titles(&store, &ctx, Expr::True).await.unwrap_err();
    assert!(
        matches!(err, KernelError::AccessDenied { .. }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn rbac_gates_each_action_separately() {
    let store = seeded(security()).await;
    let table = documents();
    let reader = user(ALICE, TENANT_A, "reader");

    // Reader may read.
    assert!(
        !titles(&store, &reader, Expr::True)
            .await
            .unwrap()
            .is_empty()
    );

    // But not write, even to a row the policy would admit.
    let txn = store.begin().await.unwrap();
    let err = txn
        .insert(&reader, &table, &doc(TENANT_A, 9, Some(ALICE), "nope"))
        .await
        .unwrap_err();
    match err {
        KernelError::AccessDenied { action, .. } => assert_eq!(action, "insert"),
        other => panic!("expected access denied, got {other:?}"),
    }
}

#[tokio::test]
async fn a_policy_restricts_what_a_read_returns() {
    let store = seeded(security()).await;

    let mut alice = titles(&store, &user(ALICE, TENANT_A, "reader"), Expr::True)
        .await
        .unwrap();
    alice.sort();
    assert_eq!(alice, vec!["alice one", "alice two"]);

    let bob = titles(&store, &user(BOB, TENANT_A, "reader"), Expr::True)
        .await
        .unwrap();
    assert_eq!(bob, vec!["bob one"]);

    let mut admin = titles(&store, &user(ALICE, TENANT_A, "admin"), Expr::True)
        .await
        .unwrap();
    admin.sort();
    assert_eq!(
        admin,
        vec!["alice one", "alice two", "bob one", "unowned"],
        "an admin sees their whole tenant, and only their tenant"
    );
}

/// Asking for another tenant explicitly must not widen the scan. The tenant
/// term is conjoined, not defaulted, so a caller-supplied tenant filter can only
/// narrow it further.
#[tokio::test]
async fn a_caller_cannot_ask_for_another_tenant() {
    let store = seeded(security()).await;
    let table = documents();
    let tenant_ordinal = table.ordinal_of("tenant_id").unwrap();

    let rows = titles(
        &store,
        &user(ALICE, TENANT_A, "admin"),
        Expr::eq(tenant_ordinal, Value::Uuid(Uuid::from_u128(TENANT_B))),
    )
    .await
    .unwrap();
    assert!(rows.is_empty(), "reached another tenant: {rows:?}");
}

/// A tenant-scoped table refuses a context with no tenant rather than showing
/// it everything.
#[tokio::test]
async fn a_context_without_a_tenant_is_refused() {
    let store = seeded(security()).await;
    let ctx = SecurityContext::new(
        Principal::new(Value::Uuid(Uuid::from_u128(ALICE))).with_role("admin"),
    );
    let err = titles(&store, &ctx, Expr::True).await.unwrap_err();
    assert!(
        matches!(err, KernelError::TenantRequired { .. }),
        "got {err:?}"
    );
}

/// A hidden row must be indistinguishable from a missing one, or the error code
/// becomes an existence oracle.
#[tokio::test]
async fn hidden_rows_are_reported_as_missing_not_as_forbidden() {
    let store = seeded(security()).await;
    let table = documents();
    let bob = user(BOB, TENANT_A, "author");

    let txn = store.begin().await.unwrap();

    // Alice's document 1 exists, but Bob cannot see it.
    assert!(
        txn.get(&bob, &table, &pk(TENANT_A, 1))
            .await
            .unwrap()
            .is_none()
    );

    // Updating it reports "no such row" — the same answer as for id 999.
    let hidden = txn
        .update(&bob, &table, &doc(TENANT_A, 1, Some(BOB), "stolen"))
        .await
        .unwrap_err();
    let absent = txn
        .update(&bob, &table, &doc(TENANT_A, 999, Some(BOB), "new"))
        .await
        .unwrap_err();
    assert!(
        matches!(hidden, KernelError::RowNotFound { .. }),
        "got {hidden:?}"
    );
    assert!(
        matches!(absent, KernelError::RowNotFound { .. }),
        "got {absent:?}"
    );

    // Deleting reports the same "nothing to delete" as for a row that is really
    // absent.
    assert!(!txn.delete(&bob, &table, &pk(TENANT_A, 1)).await.unwrap());
    assert!(!txn.delete(&bob, &table, &pk(TENANT_A, 999)).await.unwrap());

    // And the row survived.
    txn.commit().await.unwrap();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    assert!(
        txn.get(&root, &table, &pk(TENANT_A, 1))
            .await
            .unwrap()
            .is_some()
    );
}

/// `WITH CHECK`: you may not write a row you would not be allowed to read back.
#[tokio::test]
async fn a_write_that_the_policy_would_hide_is_refused() {
    let store = seeded(security()).await;
    let table = documents();
    let bob = user(BOB, TENANT_A, "author");

    let txn = store.begin().await.unwrap();
    let err = txn
        .insert(&bob, &table, &doc(TENANT_A, 50, Some(ALICE), "planted"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::RowCheckFailed { .. }),
        "got {err:?}"
    );

    // Bob writing his own row is fine.
    txn.insert(&bob, &table, &doc(TENANT_A, 51, Some(BOB), "mine"))
        .await
        .unwrap();
}

/// You may not escape a policy by editing your way out of it.
#[tokio::test]
async fn an_update_cannot_move_a_row_out_of_the_policy() {
    let store = seeded(security()).await;
    let table = documents();
    let bob = user(BOB, TENANT_A, "author");

    let txn = store.begin().await.unwrap();
    let err = txn
        .update(&bob, &table, &doc(TENANT_A, 3, Some(ALICE), "handed off"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::RowCheckFailed { .. }),
        "got {err:?}"
    );
}

/// An upsert must not become a silent overwrite of a row the caller cannot see.
#[tokio::test]
async fn an_upsert_cannot_overwrite_a_hidden_row() {
    let store = seeded(security()).await;
    let table = documents();
    let bob = user(BOB, TENANT_A, "author");

    let txn = store.begin().await.unwrap();
    let err = txn
        .upsert(&bob, &table, &doc(TENANT_A, 1, Some(BOB), "taken"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::RowNotFound { .. }),
        "got {err:?}"
    );

    let root = SecurityContext::superuser();
    let row = txn
        .get(&root, &table, &pk(TENANT_A, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.values()[3], Value::Str("alice one".into()));
}

/// Row-level security is on but nothing admits anything: the answer is no rows,
/// not all rows.
#[tokio::test]
async fn rls_with_no_applicable_policy_returns_nothing() {
    let security = SecurityCatalog::new()
        .grant(Grant::new("reader", DOCUMENTS, [Action::Read]))
        .enable_rls(DOCUMENTS);
    let store = seeded(security).await;

    let rows = titles(&store, &user(ALICE, TENANT_A, "reader"), Expr::True)
        .await
        .unwrap();
    assert!(rows.is_empty(), "RLS failed open: {rows:?}");
}

/// Three-valued logic, and why it is a security property rather than a nicety.
///
/// Under two-valued logic `NOT (owner_id = alice)` is true for the row whose
/// owner is null, so a deny-style policy would hand out exactly the rows nobody
/// owns. Under SQL's semantics the comparison is unknown, the negation stays
/// unknown, and the row is withheld.
#[tokio::test]
async fn a_negated_policy_does_not_leak_rows_with_null_columns() {
    let security = SecurityCatalog::new()
        .grant(Grant::new("reader", DOCUMENTS, [Action::Read]))
        .policy(Policy::new(
            "not_alices",
            DOCUMENTS,
            [Action::Read],
            |_: &SecurityContext| {
                Expr::Not(Box::new(Expr::eq(
                    owner_ordinal(),
                    Value::Uuid(Uuid::from_u128(ALICE)),
                )))
            },
        ));
    let store = seeded(security).await;

    let rows = titles(&store, &user(BOB, TENANT_A, "reader"), Expr::True)
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec!["bob one"],
        "the unowned row leaked through a negated policy"
    );
}

#[tokio::test]
async fn superuser_bypasses_both_rbac_and_rls() {
    let store = seeded(security()).await;
    let root = SecurityContext::superuser();
    let mut rows = titles(&store, &root, Expr::True).await.unwrap();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            "alice one",
            "alice two",
            "bob one",
            "other tenant",
            "unowned"
        ],
        "superuser should see every tenant"
    );
}

/// The policy must survive whichever access path the planner picks. Filtering
/// on an indexed column routes the query through an index scan; the policy has
/// to come along.
#[tokio::test]
async fn a_policy_holds_on_an_index_scan_too() {
    let store = seeded(security()).await;
    let table = documents();
    let owner = owner_ordinal();

    // Bob asks, by owner, for Alice's documents.
    let rows = titles(
        &store,
        &user(BOB, TENANT_A, "reader"),
        Expr::eq(owner, Value::Uuid(Uuid::from_u128(ALICE))),
    )
    .await
    .unwrap();
    assert!(rows.is_empty(), "index scan bypassed the policy: {rows:?}");

    // The planner picks the index only once the tenant is pinned, because the
    // tenant leads every index key on a tenant-scoped table. That term comes
    // from the policy, so here the security filter does not just constrain the
    // query — it is what makes the index usable at all.
    let tenant = table.ordinal_of("tenant_id").unwrap();
    let bare = Expr::eq(owner, Value::Uuid(Uuid::from_u128(ALICE)));
    let secured = bare
        .clone()
        .and(Expr::eq(tenant, Value::Uuid(Uuid::from_u128(TENANT_A))));

    let bare_plan = slate_kernel::plan(&table, &bare, ScanOrder::Ascending);
    assert!(
        matches!(bare_plan.access, slate_kernel::Access::TableScan { .. }),
        "an unpinned tenant leaves the index unusable, got {:?}",
        bare_plan.access
    );

    let secured_plan = slate_kernel::plan(&table, &secured, ScanOrder::Ascending);
    match &secured_plan.access {
        slate_kernel::Access::IndexScan { index, range, .. } => {
            let by_owner = table.index_by_name("by_owner").unwrap();
            assert_eq!(*index, by_owner.id());
            let tenant_prefix = slate_kernel::keys::index_prefix(
                &table,
                by_owner,
                Some(&Value::Uuid(Uuid::from_u128(TENANT_A))),
            );
            // The scan cannot even address another tenant's index entries.
            assert_eq!(
                *range,
                slate_kernel::KeyRange::prefix(&tenant_prefix).intersect(range.clone())
            );
            match &range.start {
                core::ops::Bound::Included(start) => {
                    assert!(
                        start.starts_with(&tenant_prefix),
                        "scan starts outside the tenant"
                    );
                }
                other => panic!("expected an included lower bound, got {other:?}"),
            }
        }
        other => panic!("expected an index scan, got {other:?}"),
    }
}
