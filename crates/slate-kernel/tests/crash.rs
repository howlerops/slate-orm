//! Failures partway through a write.
//!
//! A row and its index entries are written together. If a write can fail
//! between them and leave the result visible, the store has an index entry
//! pointing at a row that does not exist — a lookup by index then returns a
//! row that was never there, or returns nothing while a scan returns it. That
//! is worse than a lost write, because nothing reports it.
//!
//! The record layer's claim is that it never happens: every row and every index
//! entry it implies go into one transaction, so the whole set lands or none of
//! it does. Nothing tested that claim, because nothing could make a write fail.
//!
//! [`Faulty`] can: it wraps a store and fails the *n*th write operation of a
//! transaction. Running the same workload with the fault at every position in
//! turn covers each point a real failure could land — a full disk, a fenced
//! writer, a killed process — and after each one the store must still satisfy
//! [`assert_consistent`].
//!
//! What this does not test is a crash *between* SlateDB's own writes; that is
//! SlateDB's atomicity to keep, and it is tested where atomicity lives. This
//! tests the layer that decides what goes into a transaction together.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use async_trait::async_trait;
use bytes::Bytes;
use slate_kernel::error::{KernelError, Result, StorageError};
use slate_kernel::keys;
use slate_kernel::memory::MemoryStore;
use slate_kernel::store::{KeyRange, KvIterator, KvSnapshot, KvStore, KvTransaction, ScanOrder};
use slate_kernel::{Action, Expr, Grant, RecordStore, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// --- a store that can be made to fail -------------------------------------

/// Wraps a store and fails the `fail_at`th write of each transaction.
///
/// The counter is shared and per-store rather than per-transaction, so a test
/// sets it once and the next write to reach that ordinal fails. `usize::MAX`
/// means never.
struct Faulty {
    inner: MemoryStore,
    countdown: Arc<AtomicUsize>,
}

impl Faulty {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            countdown: Arc::new(AtomicUsize::new(usize::MAX)),
        }
    }

    /// Fail after `n` more successful writes.
    fn fail_after(&self, n: usize) {
        self.countdown.store(n, Ordering::SeqCst);
    }

    fn never_fail(&self) {
        self.countdown.store(usize::MAX, Ordering::SeqCst);
    }

    /// How many writes are left before the fault fires; `None` once it has.
    fn armed(&self) -> bool {
        self.countdown.load(Ordering::SeqCst) != 0
    }
}

#[async_trait]
impl KvStore for Faulty {
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>> {
        Ok(Box::new(FaultyTxn {
            inner: self.inner.begin().await?,
            countdown: Arc::clone(&self.countdown),
        }))
    }
}

struct FaultyTxn<'a> {
    inner: Box<dyn KvTransaction + Send + 'a>,
    countdown: Arc<AtomicUsize>,
}

impl FaultyTxn<'_> {
    /// Consume one write from the budget, or fail.
    fn charge(&self) -> Result<()> {
        let remaining = self.countdown.load(Ordering::SeqCst);
        if remaining == usize::MAX {
            return Ok(());
        }
        if remaining == 0 {
            return Err(KernelError::Storage(StorageError::new(
                std::io::Error::other("injected write failure"),
            )));
        }
        self.countdown.store(remaining - 1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl KvSnapshot for FaultyTxn<'_> {
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.inner.get(key).await
    }

    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<Box<dyn KvIterator + Send + '_>> {
        self.inner.scan(range, order).await
    }

    fn is_point_in_time(&self) -> bool {
        self.inner.is_point_in_time()
    }
}

#[async_trait]
impl KvTransaction for FaultyTxn<'_> {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        self.charge()?;
        self.inner.put(key, value)
    }

    fn delete(&self, key: Vec<u8>) -> Result<()> {
        self.charge()?;
        self.inner.delete(key)
    }

    async fn commit(self: Box<Self>) -> Result<Option<u64>> {
        // The commit itself is a write, and failing here is the nastiest case:
        // everything buffered, nothing applied.
        self.charge()?;
        self.inner.commit().await
    }

    fn rollback(self: Box<Self>) {
        self.inner.rollback();
    }
}

// --- the schema -----------------------------------------------------------

const USERS: TableId = TableId(1);
const TENANT: u128 = 7;

fn users() -> TableDef {
    TableDef::builder("users", USERS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .column("age", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            IndexDef::builder("by_email", IndexId(10))
                .column("email")
                .unique(),
        )
        .index(IndexDef::builder("by_age", IndexId(11)).column("age"))
        .build()
        .expect("valid schema")
}

fn user(id: u64, email: &str, age: i64) -> Row {
    Row::new(vec![
        Value::Uuid(uuid::Uuid::from_u128(TENANT)),
        Value::U64(id),
        Value::Str(email.to_owned()),
        Value::I64(age),
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn store() -> RecordStore<Faulty> {
    let catalog = Catalog::from_tables([users()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", USERS, Action::ALL));
    RecordStore::new(Faulty::new(), catalog, security)
}

// --- the invariant --------------------------------------------------------

/// Every index entry points at a row that exists, and every row has every index
/// entry it should.
///
/// Checked in both directions on purpose. A dangling entry makes an index
/// lookup return a row that is not there; a missing entry makes it silently
/// skip a row that is. Both look like a correct empty result to a caller.
async fn assert_consistent(store: &RecordStore<Faulty>, context: &str) {
    if let Err(complaint) = check_consistent(store).await {
        panic!("{context}: {complaint}");
    }
}

/// The check itself, as a value rather than a panic, so
/// [`the_consistency_check_detects_a_dangling_entry`] can prove it fires.
async fn check_consistent(store: &RecordStore<Faulty>) -> core::result::Result<(), String> {
    let table = users();
    let txn = store.begin().await.unwrap();

    let rows = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    for index in table.indexes() {
        // Forward: every entry resolves to a row that is really there, with
        // the indexed values the entry claims.
        let mut entries = Vec::new();
        // Read the index keys directly rather than through the record layer:
        // a checker that used the same code path as the thing it checks would
        // agree with it by construction.
        let raw = store.backend().begin().await.unwrap();
        let mut cursor = raw
            .scan(
                KeyRange::prefix(&keys::index_prefix(&table, index, None)),
                ScanOrder::Ascending,
            )
            .await
            .unwrap();
        while let Some(kv) = cursor.next().await.unwrap() {
            let (indexed, primary_key) =
                match keys::decode_index_entry(&table, index, &kv.key, &kv.value) {
                    Ok(pair) => pair,
                    Err(e) => {
                        return Err(format!(
                            "index `{}` holds an undecodable entry: {e}",
                            index.name()
                        ));
                    }
                };
            let row = rows.iter().find(|r| {
                table
                    .primary_key()
                    .iter()
                    .map(|o| &r.values()[o.0])
                    .eq(primary_key.iter())
            });
            let Some(row) = row else {
                return Err(format!(
                    "index `{}` points at {primary_key:?}, which is not a row",
                    index.name()
                ));
            };
            for (column, value) in index.columns().iter().zip(&indexed) {
                if &row.values()[column.ordinal.0] != value {
                    return Err(format!(
                        "index `{}` holds a stale value for {primary_key:?}",
                        index.name()
                    ));
                }
            }
            entries.push(primary_key);
        }

        drop(cursor);
        drop(raw);

        // Backward: every row is in the index exactly once.
        if entries.len() != rows.len() {
            return Err(format!(
                "index `{}` has {} entries for {} rows",
                index.name(),
                entries.len(),
                rows.len()
            ));
        }
    }
    Ok(())
}

// --- the tests ------------------------------------------------------------

/// An insert that fails partway leaves nothing behind, wherever it fails.
///
/// The fault is moved through every write position the insert performs, which
/// is the point: a failure between the row and its first index entry is a
/// different bug from one between two index entries, and only sweeping finds
/// the position that is not covered.
#[tokio::test]
async fn an_insert_failing_at_any_point_leaves_no_trace() {
    // Count what a clean insert actually costs rather than assuming, so the
    // sweep still covers every position if the write path gains a step.
    let writes = {
        let store = store();
        let table = users();
        const BUDGET: usize = 1_000;
        store.backend().fail_after(BUDGET);
        let txn = store.begin().await.unwrap();
        txn.insert(&root(), &table, &user(1, "a@example.com", 30))
            .await
            .unwrap();
        txn.commit().await.unwrap();
        BUDGET - store.backend().countdown.load(Ordering::SeqCst)
    };
    assert!(
        (1..64).contains(&writes),
        "an insert took {writes} writes, which is not a plausible number"
    );

    for fail_at in 0..writes {
        let store = store();
        let table = users();
        store.backend().fail_after(fail_at);

        let txn = store.begin().await.unwrap();
        let inserted = txn
            .insert(&root(), &table, &user(1, "a@example.com", 30))
            .await;
        let committed = if inserted.is_ok() {
            txn.commit().await.is_ok()
        } else {
            false
        };

        store.backend().never_fail();
        assert!(
            !committed,
            "the insert reported success even though write {fail_at} failed"
        );

        let context = format!("insert failing at write {fail_at}");
        assert_consistent(&store, &context).await;

        let txn = store.begin().await.unwrap();
        let rows = txn
            .query(&root(), &table, Expr::True, ScanOrder::Ascending)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert!(rows.is_empty(), "{context}: a failed insert left {rows:?}");
    }
}

/// An update that fails partway leaves the *old* row intact, not a mixture.
///
/// An update retires index entries as well as writing new ones, so a failure
/// between the two is where a row ends up reachable through its old indexed
/// value, its new one, both, or neither.
#[tokio::test]
async fn an_update_failing_at_any_point_leaves_the_old_row() {
    for fail_at in 0..6 {
        let store = store();
        let table = users();

        let txn = store.begin().await.unwrap();
        txn.insert(&root(), &table, &user(1, "old@example.com", 30))
            .await
            .unwrap();
        txn.commit().await.unwrap();

        store.backend().fail_after(fail_at);
        let txn = store.begin().await.unwrap();
        let updated = txn
            .update(&root(), &table, &user(1, "new@example.com", 31))
            .await;
        let committed = if updated.is_ok() {
            txn.commit().await.is_ok()
        } else {
            false
        };
        let fired = !store.backend().armed();
        store.backend().never_fail();

        let context = format!("update failing at write {fail_at}");
        assert_consistent(&store, &context).await;

        let txn = store.begin().await.unwrap();
        let rows = txn
            .query(&root(), &table, Expr::True, ScanOrder::Ascending)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "{context}: the row count changed");

        // Whichever way it went, the row is wholly old or wholly new — never
        // the new email with the old age, or vice versa.
        let email = &rows[0].values()[2];
        let age = &rows[0].values()[3];
        if committed {
            assert_eq!(
                email,
                &Value::Str("new@example.com".to_owned()),
                "{context}"
            );
            assert_eq!(age, &Value::I64(31), "{context}");
        } else {
            assert_eq!(
                email,
                &Value::Str("old@example.com".to_owned()),
                "{context}: a failed update was partly applied"
            );
            assert_eq!(age, &Value::I64(30), "{context}");
        }
        assert!(
            fired || committed,
            "{context}: nothing failed and nothing committed"
        );
    }
}

/// A delete that fails partway leaves the row *and* its index entries.
///
/// The dangerous half-state here is a deleted row whose unique index entry
/// survived: the slot is then occupied by nothing, and nobody can ever take
/// that email again.
#[tokio::test]
async fn a_delete_failing_at_any_point_leaves_the_row_reachable() {
    for fail_at in 0..5 {
        let store = store();
        let table = users();

        let txn = store.begin().await.unwrap();
        txn.insert(&root(), &table, &user(1, "a@example.com", 30))
            .await
            .unwrap();
        txn.commit().await.unwrap();

        store.backend().fail_after(fail_at);
        let txn = store.begin().await.unwrap();
        let pk = vec![Value::Uuid(uuid::Uuid::from_u128(TENANT)), Value::U64(1)];
        let deleted = txn.delete(&root(), &table, &pk).await;
        let committed = if deleted.is_ok() {
            txn.commit().await.is_ok()
        } else {
            false
        };
        store.backend().never_fail();

        let context = format!("delete failing at write {fail_at}");
        assert_consistent(&store, &context).await;

        if !committed {
            // The row survived, so its unique slot must still be usable by it
            // and unusable by anyone else.
            let txn = store.begin().await.unwrap();
            let clash = txn
                .insert(&root(), &table, &user(2, "a@example.com", 40))
                .await;
            assert!(
                clash.is_err(),
                "{context}: a surviving row lost its unique index entry"
            );
        }
    }
}

/// A bulk insert is one transaction, so a failure anywhere in it discards the
/// whole batch rather than leaving a prefix of it.
///
/// A partially applied batch is the shape most likely to be mistaken for
/// success by a caller that retries: re-running it then hits duplicate-key
/// errors on the rows that did land.
#[tokio::test]
async fn a_failed_bulk_insert_lands_nothing() {
    let batch: Vec<Row> = (0..8)
        .map(|i| user(i, &format!("user{i}@example.com"), 20 + i as i64))
        .collect();

    for fail_at in [0usize, 1, 5, 12, 20, 24] {
        let store = store();
        let table = users();
        store.backend().fail_after(fail_at);

        let txn = store.begin().await.unwrap();
        let inserted = txn.insert_many(&root(), &table, &batch).await;
        let committed = if inserted.is_ok() {
            txn.commit().await.is_ok()
        } else {
            false
        };
        store.backend().never_fail();

        let context = format!("bulk insert failing at write {fail_at}");
        assert_consistent(&store, &context).await;

        let txn = store.begin().await.unwrap();
        let rows = txn
            .query(&root(), &table, Expr::True, ScanOrder::Ascending)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let expected = if committed { batch.len() } else { 0 };
        assert_eq!(
            rows.len(),
            expected,
            "{context}: a batch of {} landed {} rows",
            batch.len(),
            rows.len()
        );
    }
}

/// The consistency check has to be able to fail.
///
/// Every test above passes when the store is consistent *and* when the check
/// is silently vacuous, so this writes a dangling index entry behind the
/// record layer's back and requires the check to notice. Without it the whole
/// file could be asserting nothing.
#[tokio::test]
async fn the_consistency_check_detects_a_dangling_entry() {
    let store = store();
    let table = users();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(1, "a@example.com", 30))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    check_consistent(&store)
        .await
        .expect("premise: a clean store is consistent");

    // An entry in `by_age` for a row that was never written.
    let index = table.index(IndexId(11)).expect("by_age");
    let ghost_key = vec![Value::Uuid(uuid::Uuid::from_u128(TENANT)), Value::U64(999)];
    let entry = keys::index_entry(&table, index, &[Value::I64(44)], &ghost_key);
    let raw = store.backend().begin().await.unwrap();
    raw.put(entry.key, entry.value.to_vec()).unwrap();
    raw.commit().await.unwrap();

    let complaint = check_consistent(&store)
        .await
        .expect_err("the check passed a store with a dangling index entry");
    assert!(
        complaint.contains("by_age"),
        "the complaint should name the index: {complaint}"
    );
}

/// The fault injector has to actually fault.
///
/// Without this, every test above would pass on a store that never failed —
/// which is exactly what they would look like if `charge` were wired up wrong.
#[tokio::test]
async fn the_fault_injector_injects_faults() {
    let store = store();
    let table = users();
    store.backend().fail_after(0);

    let txn = store.begin().await.unwrap();
    let outcome = txn
        .insert(&root(), &table, &user(1, "a@example.com", 30))
        .await;
    assert!(
        outcome.is_err(),
        "the first write succeeded with the fault armed at zero"
    );

    store.backend().never_fail();
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &user(1, "a@example.com", 30))
        .await
        .expect("the same insert must succeed once the fault is disarmed");
    txn.commit().await.unwrap();
    assert_consistent(&store, "after disarming").await;
}
