//! An object store that counts what goes through it.
//!
//! Wall-clock numbers move with the machine; "renewing a lease costs one GET
//! and one conditional PUT" does not. Against an in-memory object store the
//! clock says almost nothing about what a lease renewal will cost in a bucket,
//! and the request count says almost everything — the same discipline
//! `perf_report`'s I/O counts and `scan_tuning`'s S3 counts already follow.

use bytes::Bytes;
use slatedb::object_store::path::Path;
use slatedb::object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMode, PutMultipartOptions, PutOptions, PutPayload, PutResult, Result as OsResult,
};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Requests seen, by kind.
#[derive(Debug, Default)]
pub struct Counts {
    gets: AtomicU64,
    puts: AtomicU64,
    conditional_puts: AtomicU64,
    other: AtomicU64,
}

impl Counts {
    /// Reads.
    pub fn gets(&self) -> u64 {
        self.gets.load(Ordering::Relaxed)
    }

    /// Writes of every kind, conditional ones included.
    pub fn puts(&self) -> u64 {
        self.puts.load(Ordering::Relaxed)
    }

    /// Writes that carried a precondition — the compare-and-set a lease rests
    /// on, and the one thing about it an object store has to support.
    pub fn conditional_puts(&self) -> u64 {
        self.conditional_puts.load(Ordering::Relaxed)
    }

    /// Everything else: listings, deletes, copies.
    pub fn other(&self) -> u64 {
        self.other.load(Ordering::Relaxed)
    }

    /// Every request.
    pub fn total(&self) -> u64 {
        self.gets() + self.puts() + self.other()
    }

    /// Start counting again.
    pub fn reset(&self) {
        for counter in [&self.gets, &self.puts, &self.conditional_puts, &self.other] {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

/// Wraps an object store and counts the requests reaching it.
#[derive(Debug)]
pub struct CountingStore {
    inner: Arc<dyn ObjectStore>,
    counts: Arc<Counts>,
}

impl CountingStore {
    /// Wrap `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            counts: Arc::new(Counts::default()),
        }
    }

    /// The counters, which can be read while the store is in use.
    #[must_use]
    pub fn counts(&self) -> Arc<Counts> {
        Arc::clone(&self.counts)
    }
}

impl fmt::Display for CountingStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CountingStore({})", self.inner)
    }
}

#[async_trait::async_trait]
impl ObjectStore for CountingStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> OsResult<PutResult> {
        self.counts.puts.fetch_add(1, Ordering::Relaxed);
        if !matches!(opts.mode, PutMode::Overwrite) {
            self.counts.conditional_puts.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> OsResult<Box<dyn MultipartUpload>> {
        self.counts.puts.fetch_add(1, Ordering::Relaxed);
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> OsResult<GetResult> {
        self.counts.gets.fetch_add(1, Ordering::Relaxed);
        self.inner.get_opts(location, options).await
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[core::ops::Range<u64>],
    ) -> OsResult<Vec<Bytes>> {
        self.counts.gets.fetch_add(1, Ordering::Relaxed);
        self.inner.get_ranges(location, ranges).await
    }

    fn delete_stream(
        &self,
        locations: futures::stream::BoxStream<'static, OsResult<Path>>,
    ) -> futures::stream::BoxStream<'static, OsResult<Path>> {
        self.counts.other.fetch_add(1, Ordering::Relaxed);
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&Path>,
    ) -> futures::stream::BoxStream<'static, OsResult<ObjectMeta>> {
        self.counts.other.fetch_add(1, Ordering::Relaxed);
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> OsResult<ListResult> {
        self.counts.other.fetch_add(1, Ordering::Relaxed);
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(&self, from: &Path, to: &Path, options: CopyOptions) -> OsResult<()> {
        self.counts.other.fetch_add(1, Ordering::Relaxed);
        self.inner.copy_opts(from, to, options).await
    }
}
