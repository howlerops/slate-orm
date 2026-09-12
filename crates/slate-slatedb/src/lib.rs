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

pub mod reader;
#[cfg(feature = "aws")]
pub mod s3;

pub use reader::{ReplicaMode, SlateReader};
#[cfg(feature = "aws")]
pub use s3::S3Config;

use async_trait::async_trait;
use bytes::Bytes;
use core::ops::Bound;
use slate_kernel::error::{KernelError, Result, StorageError};
use slate_kernel::store::{
    KeyRange, KeyValue, KvIterator, KvReadStore, KvSnapshot, KvStore, KvTransaction, ScanOrder,
};
use slatedb::config::ScanOptions;
use slatedb::object_store::{ObjectStore, path::Path};
use slatedb::{
    ByteRangeBounds, CloseReason, Db, DbIterator, DbTransaction, DbTransactionOps, ErrorKind,
    IsolationLevel, IterationOrder,
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

/// Convert a SlateDB error, preserving the two kinds the kernel acts on.
pub(crate) fn convert(error: slatedb::Error) -> KernelError {
    match error.kind() {
        // A conflict is a retry signal, not a failure.
        ErrorKind::Transaction => KernelError::TransactionConflict,
        // Fencing means another writer took over. Retrying past this is a split
        // brain still trying to write, so it has to be terminal and distinct.
        ErrorKind::Closed(CloseReason::Fenced) => KernelError::WriterFenced,
        _ => KernelError::Storage(StorageError::new(error)),
    }
}

/// Bounds in the shape SlateDB's scan API wants.
///
/// A local newtype because [`ByteRangeBounds`] is only implemented for the std
/// range types, none of which can express an arbitrary pair of bounds, and
/// SlateDB's own `BytesRange` is private.
pub(crate) struct Bounds {
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

/// How a scan reads blocks out of object storage.
///
/// SlateDB can fetch several blocks per request and several requests at once,
/// but ships both off: its defaults are one block per fetch and one fetch in
/// flight, so a scan pays a round trip per block, in turn. That is the right
/// default for a library that cannot know its caller's access pattern. A
/// record layer does know — a table scan reads forward, from the first block
/// to the last — so it says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanTuning {
    /// Bytes to pull per fetch. Each fetch reads whole blocks until it has at
    /// least this much, or reaches the end of the file.
    ///
    /// The unit that matters on object storage is the request, not the byte:
    /// one 1 MiB `GET` costs about what a 4 KiB one does, so a scan that asks
    /// for a block at a time pays a round trip to save bandwidth nobody is
    /// short of.
    pub read_ahead_bytes: usize,
    /// Fetches to keep in flight. Overlapping them is what turns a scan's
    /// latency from the sum of its blocks into roughly the slowest of each
    /// batch — the same reasoning as the read pipeline in the kernel.
    pub max_fetch_tasks: usize,
    /// Whether blocks a scan pulls should stay in the block cache.
    ///
    /// Off by default, following SlateDB. A scan touches blocks once and in
    /// order, so caching them evicts whatever was being reused to hold data
    /// nothing will ask for again. Turn it on for a table small enough and hot
    /// enough that the whole thing is worth keeping.
    pub cache_blocks: bool,
}

impl Default for ScanTuning {
    /// A megabyte per fetch, four in flight, blocks not cached.
    ///
    /// Chosen for object storage, where a request costs far more than the
    /// bytes it carries. Against a local disk the same settings read more than
    /// they need; see [`ScanTuning::conservative`].
    fn default() -> Self {
        Self {
            read_ahead_bytes: 1024 * 1024,
            max_fetch_tasks: 4,
            cache_blocks: false,
        }
    }
}

impl ScanTuning {
    /// SlateDB's own defaults: one block per fetch, one fetch at a time.
    ///
    /// For a caller who has measured and found the readahead reads more than
    /// it saves, or who is running against something with cheap round trips.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            read_ahead_bytes: 1,
            max_fetch_tasks: 1,
            cache_blocks: false,
        }
    }

    fn apply(self, options: &mut ScanOptions) {
        options.read_ahead_bytes = self.read_ahead_bytes.max(1);
        options.max_fetch_tasks = self.max_fetch_tasks.max(1);
        options.cache_blocks = self.cache_blocks;
    }
}

/// A [`KvStore`] backed by SlateDB.
#[derive(Clone)]
pub struct SlateStore {
    db: Arc<Db>,
    isolation: IsolationLevel,
    durability: Durability,
    scan_tuning: ScanTuning,
}

impl core::fmt::Debug for SlateStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `Db` is not Debug, so report the settings that affect behaviour.
        f.debug_struct("SlateStore")
            .field("isolation", &self.isolation)
            .field("durability", &self.durability)
            .field("scan_tuning", &self.scan_tuning)
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

    /// Open (or create) a database in an S3-compatible bucket.
    ///
    /// Works against AWS S3, MinIO, Tigris and Cloudflare R2; see [`S3Config`]
    /// for what differs between them.
    #[cfg(feature = "aws")]
    pub async fn open_s3(path: impl Into<Path>, config: S3Config) -> Result<Self> {
        Self::open(path, config.build()?).await
    }

    /// Wrap an already-open database, so an application that manages SlateDB's
    /// lifecycle itself can still use the record layer.
    #[must_use]
    pub fn from_db(db: Arc<Db>) -> Self {
        Self {
            db,
            isolation: IsolationLevel::SerializableSnapshot,
            durability: Durability::default(),
            scan_tuning: ScanTuning::default(),
        }
    }

    /// How scans should read blocks. See [`ScanTuning`].
    #[must_use]
    pub const fn with_scan_tuning(mut self, tuning: ScanTuning) -> Self {
        self.scan_tuning = tuning;
        self
    }

    /// The scan settings in force.
    #[must_use]
    pub const fn scan_tuning(&self) -> ScanTuning {
        self.scan_tuning
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
impl KvReadStore for SlateStore {
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>> {
        // The writer reads through a transaction like everything else, so a
        // read served here sees exactly what a read-write path would.
        Ok(self.begin().await?)
    }

    fn visible_sequence(&self) -> Option<u64> {
        Some(self.db.status().durable_seq)
    }

    fn replica_name(&self) -> &str {
        // The writer is always current, so routing never needs to distinguish
        // it from a replica by name.
        "writer"
    }
}

#[async_trait]
impl KvStore for SlateStore {
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>> {
        let txn = self.db.begin(self.isolation).await.map_err(convert)?;
        Ok(Box::new(SlateTransaction {
            txn,
            durability: self.durability,
            scan_tuning: self.scan_tuning,
        }))
    }
}

/// A transaction over [`SlateStore`].
pub struct SlateTransaction {
    txn: DbTransaction,
    durability: Durability,
    scan_tuning: ScanTuning,
}

impl core::fmt::Debug for SlateTransaction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlateTransaction")
            .field("durability", &self.durability)
            .field("scan_tuning", &self.scan_tuning)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl KvSnapshot for SlateTransaction {
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.txn.get(key).await.map_err(convert)
    }

    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<Box<dyn KvIterator + Send + '_>> {
        let mut options = ScanOptions {
            order: match order {
                ScanOrder::Ascending => IterationOrder::Ascending,
                ScanOrder::Descending => IterationOrder::Descending,
            },
            ..ScanOptions::default()
        };
        self.scan_tuning.apply(&mut options);
        let iter = self
            .txn
            .scan_with_options(Bounds::from(range), &options)
            .await
            .map_err(convert)?;
        Ok(Box::new(SlateIterator { iter }))
    }
}

#[async_trait]
impl KvTransaction for SlateTransaction {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        DbTransactionOps::put(&self.txn, key, value).map_err(convert)
    }

    fn delete(&self, key: Vec<u8>) -> Result<()> {
        DbTransactionOps::delete(&self.txn, key).map_err(convert)
    }

    async fn commit(self: Box<Self>) -> Result<Option<u64>> {
        let durability = self.durability;
        // `commit` returns None for an empty batch, and otherwise a handle that
        // has been applied but not necessarily flushed.
        let Some(handle) = self.txn.commit().await.map_err(convert)? else {
            return Ok(None);
        };
        if durability == Durability::Durable {
            handle.await_durable().await.map_err(convert)?;
        }
        Ok(Some(handle.seqnum()))
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
