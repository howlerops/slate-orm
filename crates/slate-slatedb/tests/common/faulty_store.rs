//! An object store that can be made to stop working.
//!
//! `slate-kernel`'s `crash.rs` injects failures *above* SlateDB, at the
//! key-value transaction boundary, and proves the record layer never puts a row
//! and its index entries in separate transactions. That is the half this
//! project owns.
//!
//! This is the other half: a failure *below* SlateDB, where the storage engine
//! is midway through writing its own SSTs and manifest. From the record layer's
//! point of view that is a real crash — the process could not finish what it
//! started, and whatever reached the object store is what a restart will find.
//! What has to hold afterwards is not "the last write survived" (it may not)
//! but that the database still makes sense: rows and index entries agreeing,
//! nothing half-written that a reader can see.
//!
//! Writes fail rather than the process dying because a test cannot usefully
//! abort itself. A failed `put` is a strictly harder case anyway: the engine
//! learns about it and gets to react, so any recovery logic runs, whereas a
//! killed process runs none.

use bytes::Bytes;
use slatedb::object_store::path::Path;
use slatedb::object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMultipartOptions, PutOptions, PutPayload, PutResult, Result as OsResult,
};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Wraps an object store and starts failing every write after `budget` of them.
///
/// Reads keep working throughout, which is what a full disk or a revoked
/// credential looks like — and is worse than a clean stop, because the engine
/// can still make progress on the read side while its writes go nowhere.
#[derive(Debug)]
pub struct FaultyStore {
    inner: Arc<dyn ObjectStore>,
    budget: Arc<AtomicUsize>,
    tripped: Arc<AtomicBool>,
}

impl FaultyStore {
    /// A store that never fails until [`FaultyStore::fail_after`] says so.
    pub fn new(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            budget: Arc::new(AtomicUsize::new(usize::MAX)),
            tripped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Allow `n` more writes, then fail every one after that.
    pub fn fail_after(&self, n: usize) {
        self.budget.store(n, Ordering::SeqCst);
        self.tripped.store(false, Ordering::SeqCst);
    }

    /// Stop failing, so a reopened store can be read.
    pub fn recover(&self) {
        self.budget.store(usize::MAX, Ordering::SeqCst);
    }

    /// Writes still allowed before the fault fires.
    pub fn remaining(&self) -> usize {
        self.budget.load(Ordering::SeqCst)
    }

    /// Whether the fault ever actually fired. A test whose fault never fired
    /// proved nothing, so every one of them checks this.
    pub fn tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }

    fn charge(&self) -> OsResult<()> {
        let remaining = self.budget.load(Ordering::SeqCst);
        if remaining == usize::MAX {
            return Ok(());
        }
        if remaining == 0 {
            self.tripped.store(true, Ordering::SeqCst);
            return Err(slatedb::object_store::Error::Generic {
                store: "FaultyStore",
                source: "injected object-store failure".into(),
            });
        }
        self.budget.store(remaining - 1, Ordering::SeqCst);
        Ok(())
    }
}

impl fmt::Display for FaultyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FaultyStore({})", self.inner)
    }
}

#[async_trait::async_trait]
impl ObjectStore for FaultyStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> OsResult<PutResult> {
        self.charge()?;
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> OsResult<Box<dyn MultipartUpload>> {
        self.charge()?;
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> OsResult<GetResult> {
        self.inner.get_opts(location, options).await
    }

    async fn get_ranges(
        &self,
        location: &Path,
        ranges: &[core::ops::Range<u64>],
    ) -> OsResult<Vec<Bytes>> {
        self.inner.get_ranges(location, ranges).await
    }

    fn delete_stream(
        &self,
        locations: futures::stream::BoxStream<'static, OsResult<Path>>,
    ) -> futures::stream::BoxStream<'static, OsResult<Path>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&Path>,
    ) -> futures::stream::BoxStream<'static, OsResult<ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> OsResult<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(&self, from: &Path, to: &Path, options: CopyOptions) -> OsResult<()> {
        self.charge()?;
        self.inner.copy_opts(from, to, options).await
    }
}
