//! Record store behaviour: primary key operations, index maintenance and the
//! guarantees the kernel makes about both.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Expr, KernelError, RecordStore, RecordTransaction, ScanOrder, SecurityCatalog, SecurityContext,
    keys, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};
use uuid::Uuid;

const TENANT_A: u128 = 1;
const TENANT_B: u128 = 2;

fn users() -> TableDef {
    TableDef::builder("users", TableId(1))
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
        .index(
            IndexDef::builder("by_nickname", IndexId(11))
                .column("nickname")
                .unique(),
        )
        .index(IndexDef::builder("by_age", IndexId(12)).column("age"))
        .index(IndexDef::builder("by_age_desc", IndexId(13)).column_with("age", Direction::Desc))
        .schema_version(1)
        .build()
        .expect("valid schema")
}

fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([users()]).expect("catalog");
    // These tests are about the record store, not about policy; they run as
    // superuser so that authorisation never masks a storage bug. The security
    // rules have their own suite.
    RecordStore::new(MemoryStore::new(), catalog, SecurityCatalog::new())
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// Every row of `table`, read as superuser.
async fn all_rows(txn: &RecordTransaction<'_>, table: &TableDef) -> Vec<Row> {
    txn.query(&root(), table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
}

fn user(tenant: u128, id: u64, email: &str, nickname: Option<&str>, age: i64) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::U64(id),
        Value::Str(email.to_owned()),
        nickname.map_or(Value::Null, |n| Value::Str(n.to_owned())),
        Value::I64(age),
    ])
}

fn pk(tenant: u128, id: u64) -> Vec<Value> {
    vec![Value::Uuid(Uuid::from_u128(tenant)), Value::U64(id)]
}

/// Every index entry currently stored for `table`, as (index name, pk).
fn index_entries(backend: &MemoryStore, table: &TableDef) -> Vec<(String, Vec<Value>)> {
    let mut out = Vec::new();
    for (key, value) in backend.entries() {
        for index in table.indexes() {
            let prefix = keys::index_prefix(table, index, None);
            if key.starts_with(&prefix) {
                let (_, pk) =
                    keys::decode_index_entry(table, index, &key, &value).expect("decode entry");
                out.push((index.name().to_owned(), pk));
            }
        }
    }
    out.sort_by_key(|(name, pk)| (name.clone(), format!("{pk:?}")));
    out
}

/// Decoded index entries whose key starts with `prefix`, in stored key order.
///
/// Reads straight out of the backend so the assertion is about what is
/// physically stored, not about what a scan chooses to return.
fn entries_under(
    backend: &MemoryStore,
    table: &TableDef,
    index: &IndexDef,
    prefix: &[u8],
) -> Vec<(Vec<Value>, Vec<Value>)> {
    backend
        .entries()
        .into_iter()
        .filter(|(key, _)| key.starts_with(prefix))
        .map(|(key, value)| {
            keys::decode_index_entry(table, index, &key, &value).expect("decode entry")
        })
        .collect()
}

#[tokio::test]
async fn insert_then_read_back() {
    let store = store();
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
    let row = txn
        .get(&root(), &table, &pk(TENANT_A, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row, user(TENANT_A, 1, "a@x.com", Some("ay"), 30));
    assert!(
        txn.get(&root(), &table, &pk(TENANT_A, 2))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn duplicate_primary_key_is_rejected() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "a@x.com", None, 30))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let err = txn
        .insert(&root(), &table, &user(TENANT_A, 1, "b@x.com", None, 31))
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::DuplicatePrimaryKey { .. }),
        "got {err:?}"
    );
}

/// The load-bearing index guarantee: an update must retire the entries that
/// described the old values, not just add entries for the new ones.
#[tokio::test]
async fn update_retires_stale_index_entries() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 1, "old@x.com", Some("nick"), 30),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.update(
        &root(),
        &table,
        &user(TENANT_A, 1, "new@x.com", Some("nick"), 31),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // The old email must no longer resolve, and the new one must.
    let by_email = table.index_by_name("by_email").unwrap();

    let old_hits = entries_under(
        store.backend(),
        &table,
        by_email,
        &index_lookup_key(&table, by_email, "old@x.com"),
    );
    assert!(
        old_hits.is_empty(),
        "stale index entry survived: {old_hits:?}"
    );

    let new_hits = entries_under(
        store.backend(),
        &table,
        by_email,
        &index_lookup_key(&table, by_email, "new@x.com"),
    );
    assert_eq!(new_hits.len(), 1);
    assert_eq!(new_hits[0].1, pk(TENANT_A, 1));

    // And exactly one entry per index remains — no leaks, no duplicates.
    assert_eq!(
        index_entries(store.backend(), &table).len(),
        table.indexes().len()
    );
}

/// The key prefix that looks up one email in the unique `by_email` index.
fn index_lookup_key(table: &TableDef, index: &IndexDef, email: &str) -> Vec<u8> {
    let mut key = keys::index_prefix(table, index, Some(&Value::Uuid(Uuid::from_u128(TENANT_A))));
    key.extend_from_slice(&slate_tuple::encode_with(
        &[Value::Str(email.to_owned())],
        &index.directions(),
    ));
    key
}

#[tokio::test]
async fn delete_removes_row_and_every_index_entry() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 1, "a@x.com", Some("ay"), 30),
    )
    .await
    .unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 2, "b@x.com", Some("bee"), 40),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(
        index_entries(store.backend(), &table).len(),
        2 * table.indexes().len()
    );

    let txn = store.begin().await.unwrap();
    assert!(txn.delete(&root(), &table, &pk(TENANT_A, 1)).await.unwrap());
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    assert!(
        txn.get(&root(), &table, &pk(TENANT_A, 1))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        txn.get(&root(), &table, &pk(TENANT_A, 2))
            .await
            .unwrap()
            .is_some()
    );

    let remaining = index_entries(store.backend(), &table);
    assert_eq!(remaining.len(), table.indexes().len());
    for (index, key) in &remaining {
        assert_eq!(
            key,
            &pk(TENANT_A, 2),
            "index {index} still points at the deleted row"
        );
    }

    // Deleting again reports that there was nothing to delete.
    let txn = store.begin().await.unwrap();
    assert!(!txn.delete(&root(), &table, &pk(TENANT_A, 1)).await.unwrap());
}

#[tokio::test]
async fn unique_index_rejects_a_second_row_with_the_same_value() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "same@x.com", None, 30))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let err = txn
        .insert(&root(), &table, &user(TENANT_A, 2, "same@x.com", None, 31))
        .await
        .unwrap_err();
    match err {
        KernelError::UniqueViolation { ref index, .. } => assert_eq!(index, "by_email"),
        other => panic!("expected a unique violation, got {other:?}"),
    }
}

/// SQL semantics: nulls are not equal to each other, so a unique index accepts
/// any number of them.
#[tokio::test]
async fn unique_index_allows_repeated_nulls() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "a@x.com", None, 30))
        .await
        .unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 2, "b@x.com", None, 31))
        .await
        .unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 3, "c@x.com", None, 32))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let rows = all_rows(&txn, &table).await;
    assert_eq!(rows.len(), 3);
}

/// Uniqueness must not depend on the loser having *seen* the winner's write.
/// Both transactions write the same index key, so the store's own conflict
/// detection settles it even though neither read the other's row.
#[tokio::test]
async fn concurrent_inserts_of_the_same_unique_value_cannot_both_commit() {
    let store = store();
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
        "got {err:?}"
    );

    let txn = store.begin().await.unwrap();
    let rows = all_rows(&txn, &table).await;
    assert_eq!(rows.len(), 1, "only one row should have survived");
}

#[tokio::test]
async fn rollback_leaves_nothing_behind() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 1, "a@x.com", Some("ay"), 30),
    )
    .await
    .unwrap();
    txn.rollback();

    assert!(
        store.backend().is_empty(),
        "rolled-back write reached storage"
    );
}

#[tokio::test]
async fn update_of_a_missing_row_is_an_error() {
    let store = store();
    let table = users();
    let txn = store.begin().await.unwrap();
    let err = txn
        .update(&root(), &table, &user(TENANT_A, 9, "a@x.com", None, 30))
        .await
        .unwrap_err();
    assert!(
        matches!(err, KernelError::RowNotFound { .. }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn upsert_inserts_then_replaces() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.upsert(&root(), &table, &user(TENANT_A, 1, "a@x.com", None, 30))
        .await
        .unwrap();
    txn.upsert(&root(), &table, &user(TENANT_A, 1, "b@x.com", None, 31))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    assert_eq!(
        txn.get(&root(), &table, &pk(TENANT_A, 1))
            .await
            .unwrap()
            .unwrap(),
        user(TENANT_A, 1, "b@x.com", None, 31)
    );
    assert_eq!(
        index_entries(store.backend(), &table).len(),
        table.indexes().len()
    );
}

/// Tenant scoping is physical: one tenant's rows occupy a key range that does
/// not overlap another's, so a scan restricted to a tenant cannot even read a
/// neighbour's bytes.
#[tokio::test]
async fn tenants_occupy_disjoint_key_ranges() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 1, "a@x.com", None, 30))
        .await
        .unwrap();
    txn.insert(&root(), &table, &user(TENANT_A, 2, "b@x.com", None, 31))
        .await
        .unwrap();
    txn.insert(&root(), &table, &user(TENANT_B, 1, "c@x.com", None, 32))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let a_prefix =
        keys::table_tenant_prefix(&table, &Value::Uuid(Uuid::from_u128(TENANT_A))).unwrap();
    let b_prefix =
        keys::table_tenant_prefix(&table, &Value::Uuid(Uuid::from_u128(TENANT_B))).unwrap();
    assert!(!a_prefix.starts_with(&b_prefix) && !b_prefix.starts_with(&a_prefix));

    // Every stored row key for tenant A really does live under A's prefix.
    let a_keys = store
        .backend()
        .keys()
        .into_iter()
        .filter(|k| k.starts_with(&a_prefix))
        .count();
    assert_eq!(a_keys, 2);

    let txn = store.begin().await.unwrap();
    let tenant_ordinal = table.ordinal_of("tenant_id").unwrap();
    let a_rows = txn
        .query(
            &root(),
            &table,
            Expr::eq(tenant_ordinal, Value::Uuid(Uuid::from_u128(TENANT_A))),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(a_rows.len(), 2);
    for row in &a_rows {
        assert_eq!(row.values()[0], Value::Uuid(Uuid::from_u128(TENANT_A)));
    }

    // Index entries are partitioned by tenant too, not just rows.
    let by_age = table.index_by_name("by_age").unwrap();
    let a_index_prefix = keys::index_prefix(
        &table,
        by_age,
        Some(&Value::Uuid(Uuid::from_u128(TENANT_A))),
    );
    let entries = entries_under(store.backend(), &table, by_age, &a_index_prefix);
    assert_eq!(entries.len(), 2);
    for (_, key) in &entries {
        assert_eq!(key[0], Value::Uuid(Uuid::from_u128(TENANT_A)));
    }
}

/// A descending index column stores keys in reverse, so an ascending scan of it
/// yields rows in descending value order without any sorting.
#[tokio::test]
async fn descending_index_column_reverses_stored_order() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    for (i, age) in [30i64, 10, 20].into_iter().enumerate() {
        txn.insert(
            &root(),
            &table,
            &user(TENANT_A, i as u64, &format!("u{i}@x.com"), None, age),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let tenant = Value::Uuid(Uuid::from_u128(TENANT_A));

    let ages = |index: &IndexDef| -> Vec<Value> {
        entries_under(
            store.backend(),
            &table,
            index,
            &keys::index_prefix(&table, index, Some(&tenant)),
        )
        .into_iter()
        .map(|(vals, _)| vals[0].clone())
        .collect()
    };

    assert_eq!(
        ages(table.index_by_name("by_age").unwrap()),
        vec![Value::I64(10), Value::I64(20), Value::I64(30)]
    );
    // Stored in reverse, so walking the index forwards yields descending ages
    // with no sort step.
    assert_eq!(
        ages(table.index_by_name("by_age_desc").unwrap()),
        vec![Value::I64(30), Value::I64(20), Value::I64(10)]
    );
}

/// Whatever sequence of writes happens, the number of index entries must stay
/// exactly `rows * indexes`. A leak or a miss shows up here.
#[tokio::test]
async fn index_entry_count_tracks_row_count() {
    let store = store();
    let table = users();
    let indexes = table.indexes().len();

    let txn = store.begin().await.unwrap();
    for i in 0..5u64 {
        txn.insert(
            &root(),
            &table,
            &user(
                TENANT_A,
                i,
                &format!("u{i}@x.com"),
                Some(&format!("n{i}")),
                i as i64,
            ),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    assert_eq!(index_entries(store.backend(), &table).len(), 5 * indexes);

    let txn = store.begin().await.unwrap();
    txn.update(
        &root(),
        &table,
        &user(TENANT_A, 0, "changed@x.com", None, 99),
    )
    .await
    .unwrap();
    txn.delete(&root(), &table, &pk(TENANT_A, 4)).await.unwrap();
    txn.upsert(
        &root(),
        &table,
        &user(TENANT_A, 7, "seven@x.com", Some("sev"), 7),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(index_entries(store.backend(), &table).len(), 5 * indexes);
}
