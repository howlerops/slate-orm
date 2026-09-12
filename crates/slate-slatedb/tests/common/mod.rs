//! Shared fixtures and the checks every backend must pass.
//!
//! The checks live here, parameterised by store, so the in-memory object store
//! and a real S3 service run *the same* assertions rather than two suites that
//! can drift apart.

// A shared test module is compiled into each test binary, so items it exposes
// look unreachable and unused from whichever binary does not call them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    dead_code,
    unreachable_pub
)]

pub mod restart;

use slate_kernel::{
    Action, Expr, Grant, KernelError, Policy, Principal, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext,
};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_slatedb::SlateStore;
use slate_tuple::{Direction, Value, ValueType};
use uuid::Uuid;

pub const USERS: TableId = TableId(1);
pub const TENANT_A: u128 = 1;
pub const TENANT_B: u128 = 2;

pub fn users() -> TableDef {
    TableDef::builder("users", USERS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .nullable_column("nickname", ValueType::Str)
        .column("age", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            IndexDef::builder("by_email", IndexId(10))
                .column("email")
                .unique(),
        )
        .index(IndexDef::builder("by_age_desc", IndexId(11)).column_with("age", Direction::Desc))
        .schema_version(1)
        .build()
        .expect("valid schema")
}

pub fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("member", USERS, Action::ALL))
        .policy(Policy::new(
            "adults_only",
            USERS,
            [Action::Read],
            |_: &SecurityContext| {
                Expr::compare(
                    users().ordinal_of("age").unwrap(),
                    slate_kernel::CmpOp::Ge,
                    Value::I64(18),
                )
            },
        ))
}

pub fn record_store(backend: SlateStore) -> RecordStore<SlateStore> {
    let catalog = Catalog::from_tables([users()]).expect("catalog");
    RecordStore::new(backend, catalog, security())
}

/// The same, over a backend shared with a replica pool.
pub fn record_store_shared(
    backend: std::sync::Arc<SlateStore>,
) -> RecordStore<std::sync::Arc<SlateStore>> {
    let catalog = Catalog::from_tables([users()]).expect("catalog");
    RecordStore::new(backend, catalog, security())
}

pub fn user(tenant: u128, id: u64, email: &str, nickname: Option<&str>, age: i64) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::U64(id),
        Value::Str(email.to_owned()),
        nickname.map_or(Value::Null, |n| Value::Str(n.to_owned())),
        Value::I64(age),
    ])
}

pub fn pk(tenant: u128, id: u64) -> Vec<Value> {
    vec![Value::Uuid(Uuid::from_u128(tenant)), Value::U64(id)]
}

pub fn member(tenant: u128) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::Uuid(Uuid::from_u128(tenant)))
            .with_role("member"),
    )
}

pub fn root() -> SecurityContext {
    SecurityContext::superuser()
}

// --- the checks, one per behaviour, shared by every backend ---------------

pub async fn round_trip(store: &RecordStore<SlateStore>) {
    let table = users();
    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 1, "a@x.com", Some("ay"), 30),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let row = txn.get(&root(), &table, &pk(TENANT_A, 1)).await.unwrap();
    assert_eq!(row, Some(user(TENANT_A, 1, "a@x.com", Some("ay"), 30)));
}

pub async fn rollback_leaves_no_index_state(store: &RecordStore<SlateStore>) {
    let table = users();
    let email = table.ordinal_of("email").unwrap();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "keep@x.com", None, 30))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 2, "gone@x.com", None, 40))
        .await
        .unwrap();
    txn.rollback();

    let txn = store.begin().await.unwrap();
    assert!(
        txn.get(&root(), &table, &pk(TENANT_A, 2))
            .await
            .unwrap()
            .is_none()
    );
    let hits = txn
        .query(
            &root(),
            &table,
            Expr::eq(email, Value::Str("gone@x.com".into())),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(hits.is_empty(), "index entry outlived a rolled-back row");
}

pub async fn concurrent_unique_writers_conflict(store: &RecordStore<SlateStore>) {
    let table = users();
    let first = store.begin().await.unwrap();
    let second = store.begin().await.unwrap();

    first
        .insert(&root(), &table, &user(TENANT_A, 1, "race@x.com", None, 30))
        .await
        .unwrap();
    second
        .insert(&root(), &table, &user(TENANT_A, 2, "race@x.com", None, 31))
        .await
        .unwrap();

    first.commit().await.expect("first writer wins");
    let err = second.commit().await.unwrap_err();
    assert!(
        matches!(err, KernelError::TransactionConflict),
        "expected a conflict, got {err:?}"
    );

    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

pub async fn sequential_duplicate_names_the_index(store: &RecordStore<SlateStore>) {
    let table = users();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "dup@x.com", None, 30))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let err = txn
        .insert(&root(), &table, &user(TENANT_A, 2, "dup@x.com", None, 31))
        .await
        .unwrap_err();
    match err {
        KernelError::UniqueViolation { index, .. } => assert_eq!(index, "by_email"),
        other => panic!("expected a unique violation, got {other:?}"),
    }
}

pub async fn scans_are_ordered_both_ways(store: &RecordStore<SlateStore>) {
    let table = users();
    let txn = store.begin().await.unwrap();
    for id in [3u64, 1, 5, 2, 4] {
        txn.insert(
            &root(),
            &table,
            &user(TENANT_A, id, &format!("u{id}@x.com"), None, 20 + id as i64),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let ids = |rows: &[Row]| -> Vec<u64> {
        rows.iter()
            .map(|r| match r.values()[1] {
                Value::U64(v) => v,
                _ => panic!("id column"),
            })
            .collect()
    };

    let txn = store.begin().await.unwrap();
    let ascending = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(ids(&ascending), vec![1, 2, 3, 4, 5]);

    let descending = txn
        .query(&root(), &table, Expr::True, ScanOrder::Descending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(ids(&descending), vec![5, 4, 3, 2, 1]);
}

pub async fn update_retires_the_old_index_entry(store: &RecordStore<SlateStore>) {
    let table = users();
    let email = table.ordinal_of("email").unwrap();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "old@x.com", None, 30))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.update(&root(), &table, &user(TENANT_A, 1, "new@x.com", None, 30))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let old = txn
        .query(
            &root(),
            &table,
            Expr::eq(email, Value::Str("old@x.com".into())),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(old.is_empty(), "stale index entry survived a real commit");

    let new = txn
        .query(
            &root(),
            &table,
            Expr::eq(email, Value::Str("new@x.com".into())),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(new.len(), 1);
}

pub async fn tenant_scoping_and_policies_hold(store: &RecordStore<SlateStore>) {
    let table = users();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "adult@a.com", None, 40))
        .await
        .unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 2, "minor@a.com", None, 12))
        .await
        .unwrap();
    txn.insert(&root(), &table, &user(TENANT_B, 1, "adult@b.com", None, 40))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&member(TENANT_A), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let emails: Vec<String> = rows
        .iter()
        .map(|r| match &r.values()[2] {
            Value::Str(s) => s.clone(),
            other => panic!("email was {other:?}"),
        })
        .collect();
    assert_eq!(emails, vec!["adult@a.com"]);
}
