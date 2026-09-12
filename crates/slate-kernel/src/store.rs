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

/// A transaction: a consistent read snapshot plus a buffer of writes that
/// become visible together, or not at all.
#[async_trait]
pub trait KvTransaction: Send + Sync {
    /// Read one key.
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>>;

    /// Open a cursor over `range`.
    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<Box<dyn KvIterator + Send + '_>>;

    /// Buffer a write.
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()>;

    /// Buffer a delete.
    fn delete(&self, key: Vec<u8>) -> Result<()>;

    /// Apply every buffered write atomically.
    ///
    /// Fails if the store detects a conflict with a transaction that committed
    /// first. The record store relies on that: two inserts racing for the same
    /// unique index slot write the same key, so one of them loses here.
    async fn commit(self: Box<Self>) -> Result<()>;

    /// Discard every buffered write.
    fn rollback(self: Box<Self>);
}

/// A transactional ordered key-value store.
#[async_trait]
pub trait KvStore: Send + Sync + 'static {
    /// Begin a transaction.
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>>;
}
