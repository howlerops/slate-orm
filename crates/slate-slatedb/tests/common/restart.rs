//! What survives a restart, parameterised by what it is stored on.
//!
//! Every other test in this crate builds a store, uses it, and drops it. That
//! leaves the most basic claim a database makes entirely unexercised: that the
//! data is still there next time. Worse, it leaves *index* durability
//! unexercised, and an index that survives a restart in a different state from
//! its table is not a lost row — it is a wrong answer returned confidently.
//!
//! A restart here means closing the `SlateStore` and opening a new one over the
//! same storage. The process-level state — memtable, block cache, the catalog
//! and security objects, every cursor — is gone; the storage is what carries
//! over. That is the same boundary a crashed and restarted process crosses,
//! minus the part where it fails mid-write, which
//! [`an_uncommitted_transaction_leaves_nothing_behind`] covers from the other
//! side and which `slate-kernel`'s `crash.rs` covers directly.
//!
//! These run over an in-memory object store *and* over the S3 protocol, since
//! "reopen and read it back" is exactly the operation where a storage backend's
//! own caching and manifest handling could differ.

use super::{TENANT_A, TENANT_B, member, pk, record_store, root, user, users};
use slate_kernel::{CmpOp, Expr, Projection, Query, RecordStore, ScanOrder, SortKey};
use slate_schema::Row;
use slate_slatedb::{S3Config, SlateStore};
use slate_tuple::Value;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;

/// Where a store's bytes live, and how to open a fresh handle onto them.
///
/// Opening twice from the same `Backing` is the restart; opening from
/// [`Backing::fresh`] gives somewhere else entirely, which is what the control
/// test needs.
pub enum Backing {
    /// An object store held in memory, shared between opens.
    Memory(Arc<dyn ObjectStore>, String),
    /// A path on whichever S3 service the environment selects.
    S3(S3Config, String),
}

impl Backing {
    /// An empty backing of the same kind, at a different location.
    pub fn fresh(&self, name: &str) -> Self {
        match self {
            Self::Memory(_, _) => Self::Memory(Arc::new(InMemory::new()), "/records".to_owned()),
            Self::S3(config, path) => Self::S3(config.clone(), format!("{path}-{name}-elsewhere")),
        }
    }

    /// Open a store over these bytes, as a fresh process would.
    pub async fn open(&self) -> RecordStore<SlateStore> {
        let backend = match self {
            Self::Memory(object_store, path) => {
                SlateStore::open(path.clone(), Arc::clone(object_store))
                    .await
                    .expect("open over the in-memory object store")
            }
            Self::S3(config, path) => SlateStore::open_s3(path.clone(), config.clone())
                .await
                .expect("open over S3"),
        };
        record_store(backend)
    }
}

/// Close a store the way a clean shutdown would.
async fn close(store: RecordStore<SlateStore>) {
    store.backend().close().await.unwrap();
}

fn ordinal(name: &str) -> slate_schema::Ordinal {
    users().ordinal_of(name).expect("column exists")
}

/// Sort by primary key so two reads are comparable regardless of plan.
fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| a.values()[1].cmp(&b.values()[1]));
    rows
}

async fn all_rows(store: &RecordStore<SlateStore>, tenant: u128) -> Vec<Row> {
    let table = users();
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&member(tenant), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    sorted(rows)
}

/// The control for every check here: a store opened over *different* storage
/// sees nothing.
///
/// Without this, all of the below could pass for the wrong reason — data
/// reaching the second store through some ambient process state rather than
/// through storage, which would make "survives a restart" mean nothing at all.
pub async fn a_different_store_is_a_different_database(backing: &Backing) {
    let table = users();

    let store = backing.open().await;
    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 1, "a@example.com", None, 30),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(all_rows(&store, TENANT_A).await.len(), 1, "premise");
    close(store).await;

    // Same schema, same code, different bytes underneath.
    let elsewhere = backing.fresh("control").open().await;
    assert!(
        all_rows(&elsewhere, TENANT_A).await.is_empty(),
        "a row appeared in a store that was never written to, so these checks \
         are not measuring durability"
    );
    close(elsewhere).await;

    // And the original still has it, so the control did not disturb it.
    let reopened = backing.open().await;
    assert_eq!(all_rows(&reopened, TENANT_A).await.len(), 1);
    close(reopened).await;
}

/// Rows written before a restart are still there, byte for byte, after it.
pub async fn committed_rows_survive_a_restart(backing: &Backing) {
    let table = users();

    let before = {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        for (id, email, nick, age) in [
            (1u64, "a@example.com", Some("ay"), 30i64),
            (2, "b@example.com", None, 41),
            (3, "c@example.com", Some("see"), 22),
        ] {
            txn.insert(&root(), &table, &user(TENANT_A, id, email, nick, age))
                .await
                .unwrap();
        }
        txn.commit().await.unwrap();
        let rows = all_rows(&store, TENANT_A).await;
        close(store).await;
        rows
    };
    assert_eq!(before.len(), 3, "premise: three rows were written");

    let store = backing.open().await;
    let after = all_rows(&store, TENANT_A).await;
    assert_eq!(after, before, "the rows changed across a restart");
    close(store).await;
}

/// A unique index still refuses a duplicate after a restart.
///
/// The index is the part most likely to be reconstructed rather than read, and
/// a unique index that forgot its entries would silently accept a duplicate
/// rather than fail loudly — so this asserts the *constraint*, not just that
/// the entries decode.
pub async fn a_unique_index_still_constrains_after_a_restart(backing: &Backing) {
    let table = users();

    {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        txn.insert(
            &root(),
            &table,
            &user(TENANT_A, 1, "taken@example.com", None, 30),
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        close(store).await;
    }

    let store = backing.open().await;
    let txn = store.begin().await.unwrap();
    let clash = txn
        .insert(
            &root(),
            &table,
            // A different primary key, the same indexed email.
            &user(TENANT_A, 2, "taken@example.com", None, 31),
        )
        .await;
    assert!(
        clash.is_err(),
        "a unique index forgot its entries across a restart"
    );
    drop(txn);
    close(store).await;
}

/// An index scan after a restart returns what a table scan returns.
///
/// The two paths read different keys for the same answer, so disagreement means
/// the index and the table came back in different states — the failure mode
/// that returns wrong rows rather than no rows.
pub async fn the_index_and_the_table_agree_after_a_restart(backing: &Backing) {
    let table = users();

    {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        for id in 0..40u64 {
            txn.insert(
                &root(),
                &table,
                &user(
                    TENANT_A,
                    id,
                    &format!("user{id}@example.com"),
                    None,
                    // Spread over the policy's cutoff so both sides are exercised.
                    18 + (id as i64 % 30),
                ),
            )
            .await
            .unwrap();
        }
        txn.commit().await.unwrap();
        close(store).await;
    }

    let store = backing.open().await;
    let age = ordinal("age");
    let ctx = member(TENANT_A);
    let txn = store.begin().await.unwrap();

    // Served by `by_age_desc`.
    let by_index = sorted(
        txn.execute(
            &ctx,
            &table,
            &Query::all()
                .filter(Expr::compare(age, CmpOp::Ge, Value::I64(30)))
                .sort_by([SortKey::desc(age)]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap(),
    );

    // The same question, answered by reading every row.
    let by_scan: Vec<Row> = all_rows(&store, TENANT_A)
        .await
        .into_iter()
        .filter(|r| matches!(r.values()[age.0], Value::I64(a) if a >= 30))
        .collect();

    assert!(!by_scan.is_empty(), "premise: some rows match");
    assert_eq!(
        by_index, by_scan,
        "the index and the table disagreed after a restart"
    );
    drop(txn);
    close(store).await;
}

/// A transaction that was never committed leaves nothing behind — including no
/// index entry pointing at a row that does not exist.
pub async fn an_uncommitted_transaction_leaves_nothing_behind(backing: &Backing) {
    let table = users();

    {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        txn.insert(
            &root(),
            &table,
            &user(TENANT_A, 1, "committed@example.com", None, 30),
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();

        // A second transaction that writes and is then dropped without
        // committing, as an interrupted process would leave it.
        let abandoned = store.begin().await.unwrap();
        abandoned
            .insert(
                &root(),
                &table,
                &user(TENANT_A, 2, "abandoned@example.com", None, 31),
            )
            .await
            .unwrap();
        drop(abandoned);

        close(store).await;
    }

    let store = backing.open().await;
    let rows = all_rows(&store, TENANT_A).await;
    assert_eq!(rows.len(), 1, "an uncommitted row survived: {rows:?}");
    assert_eq!(rows[0].values()[1], Value::U64(1));

    // And no orphaned index entry: the abandoned email must be insertable,
    // which it would not be if a unique index entry had outlived its row.
    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 9, "abandoned@example.com", None, 32),
    )
    .await
    .expect("an index entry outlived the transaction that wrote it");
    txn.commit().await.unwrap();
    close(store).await;
}

/// A deletion is durable too. A row that comes back after being deleted is the
/// same class of bug as one that vanishes, and tombstones are exactly the thing
/// a restart can lose.
pub async fn a_delete_stays_deleted_across_a_restart(backing: &Backing) {
    let table = users();

    {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        txn.insert(
            &root(),
            &table,
            &user(TENANT_A, 1, "one@example.com", None, 30),
        )
        .await
        .unwrap();
        txn.insert(
            &root(),
            &table,
            &user(TENANT_A, 2, "two@example.com", None, 31),
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();

        let txn = store.begin().await.unwrap();
        txn.delete(&root(), &table, &pk(TENANT_A, 1)).await.unwrap();
        txn.commit().await.unwrap();
        close(store).await;
    }

    let store = backing.open().await;
    let rows = all_rows(&store, TENANT_A).await;
    assert_eq!(rows.len(), 1, "a deleted row came back: {rows:?}");
    assert_eq!(rows[0].values()[1], Value::U64(2));

    // The deleted row's index entry is gone too, so its email is free again.
    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &table,
        &user(TENANT_A, 3, "one@example.com", None, 33),
    )
    .await
    .expect("a deleted row's index entry survived the restart");
    txn.commit().await.unwrap();
    close(store).await;
}

/// Tenant isolation is a property of the stored keys, so it has to hold after a
/// restart without anything in memory to enforce it.
pub async fn tenant_isolation_holds_after_a_restart(backing: &Backing) {
    let table = users();

    {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        txn.insert(
            &root(),
            &table,
            &user(TENANT_A, 1, "a@example.com", None, 30),
        )
        .await
        .unwrap();
        txn.insert(
            &root(),
            &table,
            &user(TENANT_B, 1, "b@example.com", None, 30),
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        close(store).await;
    }

    let store = backing.open().await;
    for (tenant, expected) in [(TENANT_A, "a@example.com"), (TENANT_B, "b@example.com")] {
        let rows = all_rows(&store, tenant).await;
        assert_eq!(rows.len(), 1, "tenant {tenant} saw {} rows", rows.len());
        assert_eq!(rows[0].values()[2], Value::Str(expected.to_owned()));
    }
    close(store).await;
}

/// Reopening twice is still the same store. Once could hide an error that only
/// shows up when the reopened state is itself written back and reopened.
pub async fn writes_after_a_restart_survive_the_next_one(backing: &Backing) {
    let table = users();

    for round in 0..3u64 {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        txn.insert(
            &root(),
            &table,
            &user(
                TENANT_A,
                round,
                &format!("round{round}@example.com"),
                None,
                30 + round as i64,
            ),
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();

        let rows = all_rows(&store, TENANT_A).await;
        assert_eq!(
            rows.len(),
            round as usize + 1,
            "round {round} did not see every earlier round's row"
        );
        close(store).await;
    }
}

/// A covering index scan reads only index keys, so after a restart it is the
/// one path that would not notice the table being gone — and the one that
/// exposes an index holding stale column values.
pub async fn a_covering_scan_agrees_with_the_table_after_a_restart(backing: &Backing) {
    let table = users();
    let age = ordinal("age");

    {
        let store = backing.open().await;
        let txn = store.begin().await.unwrap();
        for id in 0..10u64 {
            txn.insert(
                &root(),
                &table,
                &user(
                    TENANT_A,
                    id,
                    &format!("u{id}@example.com"),
                    None,
                    20 + id as i64,
                ),
            )
            .await
            .unwrap();
        }
        txn.commit().await.unwrap();

        // Change an indexed value, so a stale index entry would be visible.
        let txn = store.begin().await.unwrap();
        txn.update(
            &root(),
            &table,
            &user(TENANT_A, 5, "u5@example.com", None, 99),
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        close(store).await;
    }

    let store = backing.open().await;
    let ctx = member(TENANT_A);
    let txn = store.begin().await.unwrap();
    let covered = txn
        .query_projected(
            &ctx,
            &table,
            Expr::compare(age, CmpOp::Ge, Value::I64(90)),
            ScanOrder::Ascending,
            &Projection::Columns(vec![age]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(
        covered.len(),
        1,
        "the index disagreed with the update after a restart: {covered:?}"
    );
    assert_eq!(covered[0].values()[age.0], Value::I64(99));
    drop(txn);
    close(store).await;
}
