//! A store wrapper that charges for I/O, for modelling object storage.
//!
//! Benchmarks against an in-memory map answer the wrong question. Real reads go
//! to object storage, where a point lookup is a network round trip and a
//! sequential scan amortises one round trip over a whole block. Measured
//! without that, every plan looks equally good and the optimisations that
//! matter — avoiding point lookups, or overlapping them — measure as pure
//! overhead.
//!
//! The model is deliberately crude, because only the *shape* matters: a fixed
//! cost per point read, a fixed cost to open a scan, and one more each time a
//! scan crosses a block boundary.
//!
//! Prefer the [`IoCounters`] over the clock where you can. "This change removed
//! 9,000 point reads" is a claim that survives a different machine, a different
//! storage provider and a different timer resolution; a wall-clock figure from
//! a simulated profile survives none of them.
//!
//! ```
//! use slate_kernel::latency::{LatencyProfile, LatencyStore};
//! use slate_kernel::memory::MemoryStore;
//!
//! let store = LatencyStore::new(MemoryStore::new(), LatencyProfile::object_storage());
//! # let _ = store;
//! ```

use crate::error::Result;
use crate::store::{
    KeyRange, KeyValue, KvIterator, KvReadStore, KvSnapshot, KvStore, KvTransaction, ScanOrder,
};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// What each kind of I/O costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyProfile {
    /// Cost of a point read.
    pub get: Duration,
    /// Cost of opening a scan.
    pub scan_open: Duration,
    /// Cost of crossing a block boundary mid-scan.
    pub scan_block: Duration,
    /// Rows per block, so a scan pays `scan_block` every this many rows.
    pub rows_per_block: u64,
    /// Cost of a commit.
    pub commit: Duration,
}

impl LatencyProfile {
    /// Nothing costs anything. The baseline to compare against.
    #[must_use]
    pub const fn free() -> Self {
        Self {
            get: Duration::ZERO,
            scan_open: Duration::ZERO,
            scan_block: Duration::ZERO,
            rows_per_block: 1024,
            commit: Duration::ZERO,
        }
    }

    /// Roughly what object storage in the same region costs.
    ///
    /// A point read is a round trip; a scan pays one to start and one per
    /// block. The ratio between them is the thing being modelled, not the
    /// absolute figures.
    ///
    /// Every charge is at least a millisecond deliberately. The runtime timer
    /// has roughly millisecond granularity, so a sub-millisecond sleep is
    /// rounded up and a profile built from microseconds silently measures the
    /// timer instead of the workload. Millisecond round trips are a fair
    /// description of object storage anyway.
    #[must_use]
    pub const fn object_storage() -> Self {
        Self {
            get: Duration::from_millis(1),
            scan_open: Duration::from_millis(1),
            scan_block: Duration::from_millis(1),
            rows_per_block: 256,
            commit: Duration::from_millis(2),
        }
    }
}

/// Counts of the I/O a workload performed.
///
/// Often more useful than the wall clock: "this change removed 9,000 point
/// reads" is a claim that holds whatever the storage costs.
#[derive(Debug, Default)]
pub struct IoCounters {
    gets: AtomicU64,
    scans: AtomicU64,
    scan_rows: AtomicU64,
    commits: AtomicU64,
}

impl IoCounters {
    /// Point reads issued.
    pub fn gets(&self) -> u64 {
        self.gets.load(Ordering::Relaxed)
    }

    /// Scans opened.
    pub fn scans(&self) -> u64 {
        self.scans.load(Ordering::Relaxed)
    }

    /// Rows pulled from scans.
    pub fn scan_rows(&self) -> u64 {
        self.scan_rows.load(Ordering::Relaxed)
    }

    /// Commits performed.
    pub fn commits(&self) -> u64 {
        self.commits.load(Ordering::Relaxed)
    }

    /// Reset every counter.
    pub fn reset(&self) {
        self.gets.store(0, Ordering::Relaxed);
        self.scans.store(0, Ordering::Relaxed);
        self.scan_rows.store(0, Ordering::Relaxed);
        self.commits.store(0, Ordering::Relaxed);
    }
}

/// Wraps a store so its I/O costs time and is counted.
#[derive(Debug)]
pub struct LatencyStore<S> {
    inner: S,
    profile: LatencyProfile,
    counters: Arc<IoCounters>,
}

impl<S> LatencyStore<S> {
    /// Wrap `inner`, charging `profile` per operation.
    pub fn new(inner: S, profile: LatencyProfile) -> Self {
        Self {
            inner,
            profile,
            counters: Arc::new(IoCounters::default()),
        }
    }

    /// The I/O counters, shared with everything this store hands out.
    #[must_use]
    pub fn counters(&self) -> Arc<IoCounters> {
        Arc::clone(&self.counters)
    }

    /// The wrapped store.
    pub const fn inner(&self) -> &S {
        &self.inner
    }
}

async fn charge(duration: Duration) {
    if !duration.is_zero() {
        // Sleeping rather than spinning matters: concurrent reads must be able
        // to overlap, which is the whole point of measuring a pipelined plan
        // against a serial one.
        tokio::time::sleep(duration).await;
    }
}

#[async_trait]
impl<S: KvStore> KvStore for LatencyStore<S> {
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>> {
        Ok(Box::new(SlowTransaction {
            inner: self.inner.begin().await?,
            profile: self.profile,
            counters: Arc::clone(&self.counters),
        }))
    }
}

#[async_trait]
impl<S: KvReadStore> KvReadStore for LatencyStore<S> {
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>> {
        Ok(Box::new(SlowSnapshot {
            inner: self.inner.snapshot().await?,
            profile: self.profile,
            counters: Arc::clone(&self.counters),
        }))
    }

    fn visible_sequence(&self) -> Option<u64> {
        self.inner.visible_sequence()
    }

    fn replica_name(&self) -> &str {
        self.inner.replica_name()
    }
}

/// Shared read behaviour for the transaction and snapshot wrappers.
macro_rules! slow_reads {
    ($ty:ident) => {
        #[async_trait]
        impl KvSnapshot for $ty<'_> {
            async fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
                self.counters.gets.fetch_add(1, Ordering::Relaxed);
                charge(self.profile.get).await;
                self.inner.get(key).await
            }

            async fn scan(
                &self,
                range: KeyRange,
                order: ScanOrder,
            ) -> Result<Box<dyn KvIterator + Send + '_>> {
                self.counters.scans.fetch_add(1, Ordering::Relaxed);
                charge(self.profile.scan_open).await;
                Ok(Box::new(SlowIterator {
                    inner: self.inner.scan(range, order).await?,
                    profile: self.profile,
                    counters: Arc::clone(&self.counters),
                    seen: 0,
                }))
            }

            fn is_point_in_time(&self) -> bool {
                self.inner.is_point_in_time()
            }
        }
    };
}

struct SlowTransaction<'a> {
    inner: Box<dyn KvTransaction + Send + 'a>,
    profile: LatencyProfile,
    counters: Arc<IoCounters>,
}

impl core::fmt::Debug for SlowTransaction<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlowTransaction").finish_non_exhaustive()
    }
}

slow_reads!(SlowTransaction);

#[async_trait]
impl KvTransaction for SlowTransaction<'_> {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        self.inner.put(key, value)
    }

    fn delete(&self, key: Vec<u8>) -> Result<()> {
        self.inner.delete(key)
    }

    async fn commit(self: Box<Self>) -> Result<Option<u64>> {
        self.counters.commits.fetch_add(1, Ordering::Relaxed);
        charge(self.profile.commit).await;
        self.inner.commit().await
    }

    fn rollback(self: Box<Self>) {
        self.inner.rollback();
    }
}

struct SlowSnapshot<'a> {
    inner: Box<dyn KvSnapshot + Send + 'a>,
    profile: LatencyProfile,
    counters: Arc<IoCounters>,
}

impl core::fmt::Debug for SlowSnapshot<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlowSnapshot").finish_non_exhaustive()
    }
}

slow_reads!(SlowSnapshot);

struct SlowIterator<'a> {
    inner: Box<dyn KvIterator + Send + 'a>,
    profile: LatencyProfile,
    counters: Arc<IoCounters>,
    seen: u64,
}

#[async_trait]
impl KvIterator for SlowIterator<'_> {
    async fn next(&mut self) -> Result<Option<KeyValue>> {
        // A scan pays only when it crosses a block boundary, which is what makes
        // sequential access cheap relative to point lookups.
        if self.seen > 0 && self.seen.is_multiple_of(self.profile.rows_per_block.max(1)) {
            charge(self.profile.scan_block).await;
        }
        let item = self.inner.next().await?;
        if item.is_some() {
            self.seen += 1;
            self.counters.scan_rows.fetch_add(1, Ordering::Relaxed);
        }
        Ok(item)
    }
}
