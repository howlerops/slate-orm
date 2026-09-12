//! An in-memory [`KvStore`] with the same visible semantics as the real thing.
//!
//! This exists so the record layer can be tested end to end — including
//! uniqueness under concurrency — without object storage in the loop. It
//! reproduces the two behaviours the kernel depends on: a transaction reads a
//! consistent snapshot taken when it began, and a commit fails if any key it
//! wrote was also written by a transaction that committed in the meantime.
//!
//! It is a test and demo backend, not a storage engine: a transaction copies the
//! whole map when it begins.

use crate::error::{KernelError, Result};
use crate::store::{
    KeyRange, KeyValue, KvIterator, KvReadStore, KvSnapshot, KvStore, KvTransaction, ScanOrder,
};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

type Snapshot = BTreeMap<Vec<u8>, Bytes>;
/// A buffered write: `Some` to put, `None` to delete.
type Pending = BTreeMap<Vec<u8>, Option<Bytes>>;

#[derive(Debug, Default)]
struct Shared {
    committed: Snapshot,
    /// Monotonic commit counter; also the version a transaction snapshots at.
    version: u64,
    /// Keys written by each commit, newest last, for conflict detection.
    history: Vec<(u64, BTreeSet<Vec<u8>>)>,
    /// Snapshot versions of transactions still running, so history is only
    /// trimmed once no one can still need it.
    active: BTreeMap<u64, usize>,
}

impl Shared {
    fn register(&mut self, version: u64) {
        *self.active.entry(version).or_insert(0) += 1;
    }

    fn unregister(&mut self, version: u64) {
        if let Some(count) = self.active.get_mut(&version) {
            *count -= 1;
            if *count == 0 {
                self.active.remove(&version);
            }
        }
        self.trim_history();
    }

    fn trim_history(&mut self) {
        let watermark = self.active.keys().next().copied().unwrap_or(self.version);
        self.history.retain(|(v, _)| *v > watermark);
    }
}

/// An in-memory transactional key-value store. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    shared: Arc<Mutex<Shared>>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of committed keys. Useful for asserting that a rolled-back or
    /// failed write really left nothing behind.
    ///
    /// # Panics
    /// If another thread panicked while holding the store's lock.
    #[must_use]
    pub fn len(&self) -> usize {
        self.locked(|s| s.committed.len())
    }

    /// Whether the store holds no committed keys.
    ///
    /// # Panics
    /// If another thread panicked while holding the store's lock.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every committed key, in order. Intended for assertions about layout.
    ///
    /// # Panics
    /// If another thread panicked while holding the store's lock.
    #[must_use]
    pub fn keys(&self) -> Vec<Vec<u8>> {
        self.locked(|s| s.committed.keys().cloned().collect())
    }

    /// Every committed key and value, in key order. Intended for assertions
    /// about physical layout, which is why it is sync and needs no transaction.
    ///
    /// # Panics
    /// If another thread panicked while holding the store's lock.
    #[must_use]
    pub fn entries(&self) -> Vec<(Vec<u8>, Bytes)> {
        self.locked(|s| {
            s.committed
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
    }

    /// Start a transaction against a copy of the current committed state.
    async fn open(&self) -> MemoryTransaction {
        let (snapshot, version) = self.locked(|s| {
            s.register(s.version);
            (s.committed.clone(), s.version)
        });
        MemoryTransaction {
            shared: Arc::clone(&self.shared),
            snapshot,
            started_at: version,
            pending: Mutex::new(Pending::new()),
        }
    }

    fn locked<T>(&self, f: impl FnOnce(&mut Shared) -> T) -> T {
        // A poisoned lock means a test already failed inside a critical
        // section; surfacing the original panic is more useful than masking it.
        #[allow(clippy::expect_used)]
        let mut guard = self.shared.lock().expect("memory store lock poisoned");
        f(&mut guard)
    }
}

#[async_trait]
impl KvReadStore for MemoryStore {
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>> {
        Ok(Box::new(self.open().await))
    }

    fn visible_sequence(&self) -> Option<u64> {
        Some(self.locked(|s| s.version))
    }

    fn replica_name(&self) -> &str {
        "memory"
    }
}

#[async_trait]
impl KvStore for MemoryStore {
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>> {
        Ok(Box::new(self.open().await))
    }
}

/// A transaction over [`MemoryStore`].
#[derive(Debug)]
pub struct MemoryTransaction {
    shared: Arc<Mutex<Shared>>,
    snapshot: Snapshot,
    started_at: u64,
    pending: Mutex<Pending>,
}

impl MemoryTransaction {
    fn with_pending<T>(&self, f: impl FnOnce(&mut Pending) -> T) -> T {
        #[allow(clippy::expect_used)]
        let mut guard = self.pending.lock().expect("transaction lock poisoned");
        f(&mut guard)
    }

    /// The snapshot with this transaction's own writes applied, so a read sees
    /// what it has already written.
    fn visible(&self) -> Snapshot {
        let mut view = self.snapshot.clone();
        self.with_pending(|pending| {
            for (key, value) in pending.iter() {
                match value {
                    Some(v) => {
                        view.insert(key.clone(), v.clone());
                    }
                    None => {
                        view.remove(key);
                    }
                }
            }
        });
        view
    }
}

impl Drop for MemoryTransaction {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.unregister(self.started_at);
        }
    }
}

#[async_trait]
impl KvSnapshot for MemoryTransaction {
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        if let Some(pending) = self.with_pending(|p| p.get(key).cloned()) {
            return Ok(pending);
        }
        Ok(self.snapshot.get(key).cloned())
    }

    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<Box<dyn KvIterator + Send + '_>> {
        let mut items: Vec<KeyValue> = self
            .visible()
            .into_iter()
            .filter(|(k, _)| range.contains(k))
            .map(|(key, value)| KeyValue {
                key: Bytes::from(key),
                value,
            })
            .collect();
        if order == ScanOrder::Descending {
            items.reverse();
        }
        Ok(Box::new(MemoryIterator {
            items: items.into_iter(),
        }))
    }
}

#[async_trait]
impl KvTransaction for MemoryTransaction {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        self.with_pending(|p| p.insert(key, Some(Bytes::from(value))));
        Ok(())
    }

    fn delete(&self, key: Vec<u8>) -> Result<()> {
        self.with_pending(|p| p.insert(key, None));
        Ok(())
    }

    async fn commit(self: Box<Self>) -> Result<Option<u64>> {
        let pending = self.with_pending(core::mem::take);
        if pending.is_empty() {
            return Ok(None);
        }

        #[allow(clippy::expect_used)]
        let mut shared = self.shared.lock().expect("memory store lock poisoned");

        // Write-write conflict: did anything we wrote also change after our
        // snapshot? This is what makes a unique index collision resolve to one
        // winner without any read-check.
        let conflict = shared
            .history
            .iter()
            .filter(|(version, _)| *version > self.started_at)
            .any(|(_, keys)| pending.keys().any(|k| keys.contains(k)));
        if conflict {
            return Err(KernelError::TransactionConflict);
        }

        shared.version += 1;
        let version = shared.version;
        for (key, value) in &pending {
            match value {
                Some(v) => {
                    shared.committed.insert(key.clone(), v.clone());
                }
                None => {
                    shared.committed.remove(key);
                }
            }
        }
        shared
            .history
            .push((version, pending.into_keys().collect()));
        Ok(Some(version))
    }

    fn rollback(self: Box<Self>) {
        self.with_pending(Pending::clear);
    }
}

/// Cursor over a materialised range of [`MemoryStore`].
#[derive(Debug)]
struct MemoryIterator {
    items: std::vec::IntoIter<KeyValue>,
}

#[async_trait]
impl KvIterator for MemoryIterator {
    async fn next(&mut self) -> Result<Option<KeyValue>> {
        Ok(self.items.next())
    }
}
