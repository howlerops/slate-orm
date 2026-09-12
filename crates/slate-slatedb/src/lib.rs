//! SlateDB backend for the slate-orm record layer.
//!
//! SlateDB is a log-structured store over object storage with a single fenced
//! writer and transactional writes, which is what the record layer assumes:
//! index entries land atomically with the row they describe, and two writers
//! racing for one unique index slot are separated by write-write conflict
//! detection rather than by a read-check.
//!
//! ```no_run
//! use slate_slatedb::SlateStore;
//! use slatedb::object_store::memory::InMemory;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let store = SlateStore::open("/records", Arc::new(InMemory::new())).await?;
//! # let _ = store;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use bytes::Bytes;
use core::ops::Bound;
use slate_kernel::error::{KernelError, Result, StorageError};
use slate_kernel::store::{KeyRange, KeyValue, KvIterator, KvStore, KvTransaction, ScanOrder};
use slatedb::config::ScanOptions;
use slatedb::object_store::{ObjectStore, path::Path};
use slatedb::{
    ByteRangeBounds, Db, DbIterator, DbTransaction, DbTransactionOps, ErrorKind, IsolationLevel,
    IterationOrder,
};
use std::sync::Arc;

/// How long a commit waits before it reports success.
///
/// SlateDB applies a commit atomically and makes it visible to readers before
/// it reaches object storage, so this is a real choice rather than a tuning
/// knob: it decides whether an acknowledged write can be lost when the writer
/// dies before the next flush.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Durability {
    /// Wait until the write is in object storage.
    ///
    /// The default. Commit latency is bounded by the flush interval, which for
    /// object storage is milliseconds at best.
    #[default]
    Durable,
    /// Return as soon as the write is committed and visible.
    ///
    /// Faster, and correct for readers — but a write acknowledged this way can
    /// be lost if the writer fails before its next flush. Choose it only where
    /// the caller can replay.
    Visible,
}

/// Convert a SlateDB error, preserving the one kind the kernel acts on.
fn convert(error: slatedb::Error) -> KernelError {
    // A conflict is a retry signal, not a failure; everything else is opaque.
    if error.kind() == ErrorKind::Transaction {
        KernelError::TransactionConflict
    } else {
        KernelError::Storage(StorageError::new(error))
    }
}

/// Bounds in the shape SlateDB's scan API wants.
///
/// A local newtype because [`ByteRangeBounds`] is only implemented for the std
/// range types, none of which can express an arbitrary pair of bounds, and
/// SlateDB's own `BytesRange` is private.
struct Bounds {
    start: Bound<Vec<u8>>,
    end: Bound<Vec<u8>>,
}

impl From<KeyRange> for Bounds {
    fn from(range: KeyRange) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

fn as_slice(bound: &Bound<Vec<u8>>) -> Bound<&[u8]> {
    match bound {
        Bound::Included(v) => Bound::Included(v.as_slice()),
        Bound::Excluded(v) => Bound::Excluded(v.as_slice()),
        Bound::Unbounded => Bound::Unbounded,
    }
}

impl ByteRangeBounds for Bounds {
    fn start_bound(&self) -> Bound<&[u8]> {
        as_slice(&self.start)
    }

    fn end_bound(&self) -> Bound<&[u8]> {
        as_slice(&self.end)
    }
}

/// A [`KvStore`] backed by SlateDB.
#[derive(Clone)]
pub struct SlateStore {
    db: Arc<Db>,
    isolation: IsolationLevel,
    durability: Durability,
}

impl core::fmt::Debug for SlateStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `Db` is not Debug, so report the settings that affect behaviour.
        f.debug_struct("SlateStore")
            .field("isolation", &self.isolation)
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

impl SlateStore {
    /// Open (or create) a database at `path` in `object_store`.
    pub async fn open(path: impl Into<Path>, object_store: Arc<dyn ObjectStore>) -> Result<Self> {
        let db = Db::builder(path.into(), object_store)
            .build()
            .await
            .map_err(convert)?;
        Ok(Self::from_db(Arc::new(db)))
    }

    /// Wrap an already-open database, so an application that manages SlateDB's
    /// lifecycle itself can still use the record layer.
    #[must_use]
    pub fn from_db(db: Arc<Db>) -> Self {
        Self {
            db,
            isolation: IsolationLevel::SerializableSnapshot,
            durability: Durability::default(),
        }
    }

    /// Choose the isolation level for transactions this store opens.
    ///
    /// The default is serializable snapshot isolation. The record layer's own
    /// guarantees — unique indexes, atomic index maintenance — hold under plain
    /// snapshot isolation too, because they rest on write-write conflicts
    /// rather than on read tracking. Application invariants that read a row and
    /// then write a *different* one are what need serializable, so the safer
    /// level is the default and stepping down is explicit.
    #[must_use]
    pub const fn with_isolation(mut self, isolation: IsolationLevel) -> Self {
        self.isolation = isolation;
        self
    }

    /// Choose what a successful commit means. See [`Durability`].
    #[must_use]
    pub const fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// The underlying database.
    #[must_use]
    pub fn db(&self) -> &Arc<Db> {
        &self.db
    }

    /// Flush and close the database.
    pub async fn close(&self) -> Result<()> {
        self.db.close().await.map_err(convert)
    }
}

#[async_trait]
impl KvStore for SlateStore {
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>> {
        let txn = self.db.begin(self.isolation).await.map_err(convert)?;
        Ok(Box::new(SlateTransaction {
            txn,
            durability: self.durability,
        }))
    }
}

/// A transaction over [`SlateStore`].
pub struct SlateTransaction {
    txn: DbTransaction,
    durability: Durability,
}

impl core::fmt::Debug for SlateTransaction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlateTransaction")
            .field("durability", &self.durability)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl KvTransaction for SlateTransaction {
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.txn.get(key).await.map_err(convert)
    }

    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<Box<dyn KvIterator + Send + '_>> {
        let options = ScanOptions {
            order: match order {
                ScanOrder::Ascending => IterationOrder::Ascending,
                ScanOrder::Descending => IterationOrder::Descending,
            },
            ..ScanOptions::default()
        };
        let iter = self
            .txn
            .scan_with_options(Bounds::from(range), &options)
            .await
            .map_err(convert)?;
        Ok(Box::new(SlateIterator { iter }))
    }

    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        DbTransactionOps::put(&self.txn, key, value).map_err(convert)
    }

    fn delete(&self, key: Vec<u8>) -> Result<()> {
        DbTransactionOps::delete(&self.txn, key).map_err(convert)
    }

    async fn commit(self: Box<Self>) -> Result<()> {
        let durability = self.durability;
        let handle = self.txn.commit().await.map_err(convert)?;
        // `commit` returns None for an empty batch, and otherwise a handle that
        // has been applied but not necessarily flushed.
        if durability == Durability::Durable
            && let Some(handle) = handle
        {
            handle.await_durable().await.map_err(convert)?;
        }
        Ok(())
    }

    fn rollback(self: Box<Self>) {
        self.txn.rollback();
    }
}

/// Cursor over a SlateDB range scan.
struct SlateIterator {
    iter: DbIterator,
}

impl core::fmt::Debug for SlateIterator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlateIterator").finish_non_exhaustive()
    }
}

#[async_trait]
impl KvIterator for SlateIterator {
    async fn next(&mut self) -> Result<Option<KeyValue>> {
        let entry: Option<slatedb::KeyValue> = self.iter.next().await.map_err(convert)?;
        Ok(entry.map(|kv| KeyValue {
            key: kv.key,
            value: kv.value,
        }))
    }
}
