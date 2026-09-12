//! The storage abstraction the kernel is written against.
//!
//! The kernel needs exactly four things from a store: transactional point
//! reads, ordered range scans, buffered writes, and an atomic commit. Keeping
//! that surface small is what lets index maintenance be provably atomic — every
//! index write for a row goes into the same transaction as the row itself — and
//! lets the whole record layer be tested against an in-memory backend without
//! object storage in the loop.

use crate::error::Result;
use async_trait::async_trait;
use bytes::Bytes;
use core::ops::Bound;
use core::time::Duration;

/// One key-value pair returned by a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    /// The stored key.
    pub key: Bytes,
    /// The stored value.
    pub value: Bytes,
}

/// Direction a scan walks the keyspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanOrder {
    /// Least key first.
    #[default]
    Ascending,
    /// Greatest key first. Used to serve a descending sort from an index
    /// without materialising and re-sorting the rows.
    Descending,
}

/// A half-open, bounded region of the keyspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRange {
    /// Lower bound.
    pub start: Bound<Vec<u8>>,
    /// Upper bound.
    pub end: Bound<Vec<u8>>,
}

impl KeyRange {
    /// The whole keyspace.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            start: Bound::Unbounded,
            end: Bound::Unbounded,
        }
    }

    /// Exactly the keys beginning with `prefix`.
    #[must_use]
    pub fn prefix(prefix: &[u8]) -> Self {
        let (start, end) = slate_tuple::prefix_range(prefix);
        Self { start, end }
    }

    /// An arbitrary pair of bounds.
    #[must_use]
    pub const fn new(start: Bound<Vec<u8>>, end: Bound<Vec<u8>>) -> Self {
        Self { start, end }
    }

    /// Narrow this range to the intersection with `other`.
    ///
    /// Used to combine a scan's own bounds with the bounds a security policy
    /// forces on it: the result can only ever be narrower than either input,
    /// which is what makes policy restriction safe to apply mechanically.
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        Self {
            start: max_start(self.start, other.start),
            end: min_end(self.end, other.end),
        }
    }

    /// Whether `key` falls inside the range.
    #[must_use]
    pub fn contains(&self, key: &[u8]) -> bool {
        let above_start = match &self.start {
            Bound::Unbounded => true,
            Bound::Included(s) => key >= s.as_slice(),
            Bound::Excluded(s) => key > s.as_slice(),
        };
        let below_end = match &self.end {
            Bound::Unbounded => true,
            Bound::Included(e) => key <= e.as_slice(),
            Bound::Excluded(e) => key < e.as_slice(),
        };
        above_start && below_end
    }

    /// Whether the range can contain no keys at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match (&self.start, &self.end) {
            (Bound::Included(s), Bound::Included(e)) => s > e,
            (Bound::Included(s) | Bound::Excluded(s), Bound::Excluded(e))
            | (Bound::Excluded(s), Bound::Included(e)) => s >= e,
            _ => false,
        }
    }
}

fn max_start(a: Bound<Vec<u8>>, b: Bound<Vec<u8>>) -> Bound<Vec<u8>> {
    match (&a, &b) {
        (Bound::Unbounded, _) => b,
        (_, Bound::Unbounded) => a,
        (Bound::Included(x) | Bound::Excluded(x), Bound::Included(y) | Bound::Excluded(y)) => {
            if x > y {
                a
            } else if y > x {
                b
            } else if matches!(a, Bound::Excluded(_)) {
                a
            } else {
                b
            }
        }
    }
}

fn min_end(a: Bound<Vec<u8>>, b: Bound<Vec<u8>>) -> Bound<Vec<u8>> {
    match (&a, &b) {
        (Bound::Unbounded, _) => b,
        (_, Bound::Unbounded) => a,
        (Bound::Included(x) | Bound::Excluded(x), Bound::Included(y) | Bound::Excluded(y)) => {
            if x < y {
                a
            } else if y < x {
                b
            } else if matches!(a, Bound::Excluded(_)) {
                a
            } else {
                b
            }
        }
    }
}

/// An open cursor over a range of the keyspace.
///
/// Modelled as an async cursor rather than a `Stream` because that is what the
/// underlying stores expose: a scan may have to fetch blocks over the network
/// between one key and the next.
#[async_trait]
pub trait KvIterator: Send {
    /// The next pair, or `None` at the end of the range.
    async fn next(&mut self) -> Result<Option<KeyValue>>;
}

/// A consistent read view of the keyspace.
///
/// Split out from [`KvTransaction`] because a read replica has one of these and
/// no way to write. Keeping them separate means a replica-backed store simply
/// lacks the write methods at the type level, rather than having them and
/// failing at runtime.
#[async_trait]
pub trait KvSnapshot: Send + Sync {
    /// Read one key.
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>>;

    /// Open a cursor over `range`.
    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<Box<dyn KvIterator + Send + '_>>;

    /// Whether every read through this view observes the same instant.
    ///
    /// A transaction does. A replica that follows the manifest does not: it can
    /// advance between two reads, so an index entry read a moment ago may point
    /// at a row that has since been deleted. The executor needs to tell those
    /// apart, because on a point-in-time view a missing row means the index and
    /// the table have genuinely diverged, and on a moving view it means only
    /// that time passed.
    fn is_point_in_time(&self) -> bool {
        true
    }
}

/// A transaction: a consistent read snapshot plus a buffer of writes that
/// become visible together, or not at all.
#[async_trait]
pub trait KvTransaction: KvSnapshot {
    /// Buffer a write.
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()>;

    /// Buffer a delete.
    fn delete(&self, key: Vec<u8>) -> Result<()>;

    /// Apply every buffered write atomically.
    ///
    /// Returns the sequence number the writes landed at, or `None` if there
    /// were none. That sequence is what lets a later read on a replica prove it
    /// has caught up; see [`KvReadStore::visible_sequence`].
    ///
    /// Fails if the store detects a conflict with a transaction that committed
    /// first. The record store relies on that: two inserts racing for the same
    /// unique index slot write the same key, so one of them loses here.
    async fn commit(self: Box<Self>) -> Result<Option<u64>>;

    /// Discard every buffered write.
    fn rollback(self: Box<Self>);
}

/// A transactional ordered key-value store: the writer.
#[async_trait]
pub trait KvStore: Send + Sync + 'static {
    /// Begin a transaction.
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>>;
}

/// A shared writer is still a writer.
#[async_trait]
impl<T: KvStore + ?Sized> KvStore for std::sync::Arc<T> {
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>> {
        (**self).begin().await
    }
}

/// A store that can be read but not written: a read replica.
///
/// A writer implements this too, so the same code paths serve both; what
/// differs is that a replica implements *only* this.
#[async_trait]
pub trait KvReadStore: Send + Sync + 'static {
    /// Open a consistent read view.
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>>;

    /// The highest sequence number this view is guaranteed to include.
    ///
    /// A replica lags the writer, so this is how a caller can tell whether a
    /// replica has caught up to a write it already knows about. Returns `None`
    /// for a store that does not track one, which callers must treat as "cannot
    /// prove it has caught up".
    fn visible_sequence(&self) -> Option<u64> {
        None
    }

    /// Wait until this view reflects `sequence`.
    ///
    /// The default only succeeds if it already does, which is the safe answer
    /// for a store that cannot observe progress: better to report staleness and
    /// let the caller route elsewhere than to block for a wakeup that will not
    /// come.
    async fn wait_for_sequence(&self, sequence: u64, _timeout: Duration) -> Result<()> {
        match self.visible_sequence() {
            Some(visible) if visible >= sequence => Ok(()),
            visible => Err(crate::error::KernelError::ReplicaTooStale {
                replica: self.replica_name().to_owned(),
                required: sequence,
                visible: visible.unwrap_or(0),
            }),
        }
    }

    /// A stable name, used for routing and diagnostics.
    ///
    /// Routing hashes this, so it must be stable for a given replica across the
    /// fleet; otherwise two processes disagree about where a tenant belongs and
    /// the cache locality that routing exists to create never materialises.
    fn replica_name(&self) -> &str {
        "unnamed"
    }
}

/// A shared replica is still a replica.
///
/// Replicas are held behind `Arc` in practice — a pool shares them, and so does
/// anything that opens one and hands it around — so forwarding through the
/// pointer saves every caller a wrapper type.
#[async_trait]
impl<T: KvReadStore + ?Sized> KvReadStore for std::sync::Arc<T> {
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>> {
        (**self).snapshot().await
    }

    fn visible_sequence(&self) -> Option<u64> {
        (**self).visible_sequence()
    }

    async fn wait_for_sequence(&self, sequence: u64, timeout: Duration) -> Result<()> {
        (**self).wait_for_sequence(sequence, timeout).await
    }

    fn replica_name(&self) -> &str {
        (**self).replica_name()
    }
}
