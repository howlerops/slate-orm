//! Read replicas.
//!
//! SlateDB allows one fenced writer and any number of readers over the same
//! object storage, which is the whole scaling story: the writer is a fixed
//! resource, reads are not. A [`SlateReader`] is one replica.
//!
//! # What a replica can and cannot promise
//!
//! A replica trails the writer. It learns about new data by polling the
//! manifest, so its view is behind by up to the poll interval plus the writer's
//! flush interval. Two consequences the caller has to live with, both surfaced
//! rather than hidden:
//!
//! - **Reads are not point-in-time unless pinned.** A reader in
//!   [`ReplicaMode::Following`] or [`ReplicaMode::Latest`] can advance between
//!   two reads inside one logical operation. The kernel is told this through
//!   [`KvSnapshot::is_point_in_time`] so that, for instance, an index entry
//!   whose row has since been deleted is skipped rather than reported as
//!   corruption. [`ReplicaMode::Pinned`] gives a genuinely fixed view.
//! - **Only durable writes are visible.** A replica reads object storage, so a
//!   write acknowledged before its flush — see
//!   [`Durability::Visible`](crate::Durability) — is not on any replica yet.
//!   Read-your-writes through a replica therefore requires durable commits.
//!   [`SlateReader::wait_for_sequence`] is how a caller waits for one.

use crate::{ScanTuning, convert};
use async_trait::async_trait;
use bytes::Bytes;
use slate_kernel::error::{KernelError, Result};
use slate_kernel::store::{KeyRange, KeyValue, KvIterator, KvReadStore, KvSnapshot, ScanOrder};
use slatedb::config::{DbReaderOptions, ScanOptions};
use slatedb::object_store::{ObjectStore, path::Path};
use slatedb::{DbReader, DbReaderMode, IterationOrder};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::Bounds;

/// How a replica tracks the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplicaMode {
    /// Follow the latest state, holding a checkpoint so compaction cannot
    /// delete data out from under an in-flight read.
    ///
    /// The default, and the right choice for a serving replica.
    #[default]
    Following,
    /// Stay fixed on one checkpoint.
    ///
    /// The only mode that gives a genuine point-in-time view, which is what a
    /// consistent multi-statement read or an export needs.
    Pinned(Uuid),
    /// Follow the latest manifest without holding a checkpoint.
    ///
    /// Writes nothing to object storage, so it is the cheapest way to open many
    /// short-lived readers — at the cost of a read failing if compaction
    /// removes an object it was about to fetch.
    Latest,
}

impl ReplicaMode {
    const fn to_slatedb(self) -> DbReaderMode {
        match self {
            Self::Following => DbReaderMode::ManagedCheckpoint,
            Self::Pinned(id) => DbReaderMode::Checkpoint(id),
            Self::Latest => DbReaderMode::FollowLatest,
        }
    }

    /// Whether reads through this mode all observe the same instant.
    const fn is_point_in_time(self) -> bool {
        matches!(self, Self::Pinned(_))
    }
}

/// A read-only replica of a SlateDB database.
#[derive(Clone)]
pub struct SlateReader {
    reader: Arc<DbReader>,
    mode: ReplicaMode,
    name: Arc<str>,
    scan_tuning: ScanTuning,
}

impl core::fmt::Debug for SlateReader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlateReader")
            .field("name", &self.name)
            .field("mode", &self.mode)
            .field("scan_tuning", &self.scan_tuning)
            .field("visible_sequence", &self.visible_sequence())
            .finish_non_exhaustive()
    }
}

impl SlateReader {
    /// Open a replica of the database at `path`.
    pub async fn open(
        name: impl Into<String>,
        path: impl Into<Path>,
        object_store: Arc<dyn ObjectStore>,
        mode: ReplicaMode,
    ) -> Result<Self> {
        Self::open_with(name, path, object_store, mode, DbReaderOptions::default()).await
    }

    /// Open a replica, choosing how closely it follows the writer.
    ///
    /// [`DbReaderOptions::manifest_poll_interval`] is the main lever: it is
    /// most of the replica's lag, and trading it down costs object-store
    /// requests per replica per interval, whether or not anything changed.
    pub async fn open_with(
        name: impl Into<String>,
        path: impl Into<Path>,
        object_store: Arc<dyn ObjectStore>,
        mode: ReplicaMode,
        options: DbReaderOptions,
    ) -> Result<Self> {
        let reader = DbReader::builder(path.into(), object_store)
            .with_reader_mode(mode.to_slatedb())
            .with_options(options)
            .build()
            .await
            .map_err(convert)?;
        Ok(Self {
            reader: Arc::new(reader),
            mode,
            name: name.into().into(),
            scan_tuning: ScanTuning::default(),
        })
    }

    /// How this replica's scans should read blocks. See
    /// [`ScanTuning`](crate::ScanTuning).
    #[must_use]
    pub const fn with_scan_tuning(mut self, tuning: ScanTuning) -> Self {
        self.scan_tuning = tuning;
        self
    }

    /// Open a replica of a database in an S3-compatible bucket.
    #[cfg(feature = "aws")]
    pub async fn open_s3(
        name: impl Into<String>,
        path: impl Into<Path>,
        config: crate::S3Config,
        mode: ReplicaMode,
    ) -> Result<Self> {
        Self::open(name, path, config.build()?, mode).await
    }

    /// This replica's name, for routing and diagnostics.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How this replica tracks the writer.
    #[must_use]
    pub const fn mode(&self) -> ReplicaMode {
        self.mode
    }

    /// The highest sequence number this replica is guaranteed to reflect.
    #[must_use]
    pub fn visible_sequence(&self) -> u64 {
        self.reader.status().durable_seq
    }

    /// Wait until this replica reflects `sequence`.
    ///
    /// This is the read-your-writes primitive: a commit hands back the sequence
    /// it wrote at, and a replica is only allowed to serve a read that carries
    /// that sequence once it has caught up. Waiting rather than failing is the
    /// right default because the wait is bounded by the replica's poll interval
    /// in the ordinary case.
    ///
    /// # Errors
    /// [`KernelError::ReplicaTooStale`] if `timeout` elapses first, so the
    /// caller can fall back to the writer instead of blocking a request.
    pub async fn wait_for_sequence(&self, sequence: u64, timeout: Duration) -> Result<()> {
        if self.visible_sequence() >= sequence {
            return Ok(());
        }
        let mut updates = self.reader.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if updates.borrow_and_update().durable_seq >= sequence {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(self.too_stale(sequence));
            }
            match tokio::time::timeout(remaining, updates.changed()).await {
                // The writer handle went away; this replica will not advance.
                Ok(Err(_)) | Err(_) => return Err(self.too_stale(sequence)),
                Ok(Ok(())) => {}
            }
        }
    }

    fn too_stale(&self, required: u64) -> KernelError {
        KernelError::ReplicaTooStale {
            replica: self.name.to_string(),
            required,
            visible: self.visible_sequence(),
        }
    }

    /// Close the replica, releasing any checkpoint it holds.
    pub async fn close(&self) -> Result<()> {
        self.reader.close().await.map_err(convert)
    }
}

#[async_trait]
impl KvReadStore for SlateReader {
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>> {
        Ok(Box::new(ReplicaSnapshot {
            reader: Arc::clone(&self.reader),
            point_in_time: self.mode.is_point_in_time(),
            scan_tuning: self.scan_tuning,
        }))
    }

    fn visible_sequence(&self) -> Option<u64> {
        Some(self.visible_sequence())
    }

    async fn wait_for_sequence(&self, sequence: u64, timeout: Duration) -> Result<()> {
        Self::wait_for_sequence(self, sequence, timeout).await
    }

    fn replica_name(&self) -> &str {
        &self.name
    }
}

/// A read view over a replica.
struct ReplicaSnapshot {
    reader: Arc<DbReader>,
    point_in_time: bool,
    scan_tuning: ScanTuning,
}

impl core::fmt::Debug for ReplicaSnapshot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReplicaSnapshot")
            .field("point_in_time", &self.point_in_time)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl KvSnapshot for ReplicaSnapshot {
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.reader.get(key).await.map_err(convert)
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
            .reader
            .scan_with_options(Bounds::from(range), &options)
            .await
            .map_err(convert)?;
        Ok(Box::new(ReplicaIterator { iter }))
    }

    fn is_point_in_time(&self) -> bool {
        self.point_in_time
    }
}

struct ReplicaIterator {
    iter: slatedb::DbIterator,
}

#[async_trait]
impl KvIterator for ReplicaIterator {
    async fn next(&mut self) -> Result<Option<KeyValue>> {
        let entry: Option<slatedb::KeyValue> = self.iter.next().await.map_err(convert)?;
        Ok(entry.map(|kv| KeyValue {
            key: kv.key,
            value: kv.value,
        }))
    }
}
