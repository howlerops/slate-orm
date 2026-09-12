//! The record store: primary key operations with atomic index maintenance.
//!
//! Every write puts the row and all of its index entries into one transaction,
//! so an index can never lag the table it describes. There is no background
//! index builder to fall behind and no repair path to get wrong: either the
//! whole write lands or none of it does.

use crate::aggregate::{Aggregate, Group};
use crate::error::{KernelError, Result};
use crate::exec::QueryCursor;
use crate::explain::Explanation;
use crate::expr::Expr;
use crate::keys::{self, IndexEntry};
use crate::plan::Projection;
use crate::query::Query;
use crate::read::{self, SecuredReads};
use crate::retry::{RetryPolicy, with_retries};
use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::stats::{ColumnStats, Statistics, TableStats};
use crate::store::{KvReadStore, KvSnapshot, KvStore, KvTransaction, ScanOrder};
use crate::token::ReadToken;
use slate_schema::{Catalog, IndexDef, Ordinal, Row, TableDef, encode_body};
use slate_tuple::Value;
use std::collections::HashSet;

/// A typed record store over a key-value backend.
/// How many distinct values [`RecordTransaction::analyze`] counts per column
/// before giving up and calling the column unique.
pub const DISTINCT_TRACKING_LIMIT: usize = 10_000;

/// A typed record store over a key-value backend.
#[derive(Debug)]
pub struct RecordStore<S> {
    store: S,
    catalog: Catalog,
    security: SecurityCatalog,
    retry: RetryPolicy,
    statistics: Statistics,
}

impl<S> RecordStore<S> {
    /// Create a record store over `store`, serving `catalog` under `security`.
    ///
    /// An empty [`SecurityCatalog`] denies every non-superuser action, so a
    /// store wired up without rules is closed rather than open.
    pub const fn new(store: S, catalog: Catalog, security: SecurityCatalog) -> Self {
        Self {
            store,
            catalog,
            security,
            retry: RetryPolicy::DEFAULT,
            statistics: Statistics::new(),
        }
    }

    /// Supply table statistics for the planner.
    ///
    /// Without these every table is assumed to hold a thousand rows with a
    /// hundred distinct values per column, which is wrong for any real table
    /// but is at least a claim about *data* rather than about the shape of a
    /// predicate. See [`Statistics`] and [`RecordTransaction::analyze`].
    #[must_use]
    pub fn with_statistics(mut self, statistics: Statistics) -> Self {
        self.statistics = statistics;
        self
    }

    /// The statistics the planner is using.
    pub const fn statistics(&self) -> &Statistics {
        &self.statistics
    }

    /// Replace the statistics, for example after re-analysing.
    pub fn set_statistics(&mut self, statistics: Statistics) {
        self.statistics = statistics;
    }

    /// Change how [`RecordStore::transact`] retries conflicts.
    #[must_use]
    pub const fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The retry policy [`RecordStore::transact`] uses.
    pub const fn retry_policy(&self) -> RetryPolicy {
        self.retry
    }

    /// The catalog this store serves.
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The access rules this store enforces.
    pub const fn security(&self) -> &SecurityCatalog {
        &self.security
    }

    /// The underlying key-value store.
    pub const fn backend(&self) -> &S {
        &self.store
    }
}

impl<S: KvReadStore> RecordStore<S> {
    /// Open a read-only view.
    ///
    /// Available for any backend that can be read, including a replica that
    /// cannot be written. The security checks are the same ones the writer
    /// applies, because both go through the same read layer.
    pub async fn snapshot(&self) -> Result<RecordSnapshot<'_>> {
        Ok(RecordSnapshot {
            snapshot: self.store.snapshot().await?,
            catalog: &self.catalog,
            security: &self.security,
            statistics: &self.statistics,
        })
    }

    /// The highest sequence number this backend's reads are guaranteed to
    /// include, when it tracks one. See [`KvReadStore::visible_sequence`].
    pub fn visible_sequence(&self) -> Option<u64> {
        self.store.visible_sequence()
    }
}

impl<S: KvStore> RecordStore<S> {
    /// Run `operation` in a transaction, retrying it if it loses a conflict.
    ///
    /// This is the intended way to write. Conflicts are ordinary here — a unique
    /// index is enforced by two writers colliding on one key — so the retry loop
    /// belongs in one place rather than at every call site.
    ///
    /// `operation` may run more than once and gets a fresh transaction each
    /// time, so it must do its own reading rather than close over values read
    /// earlier: a retry that reused the previous snapshot would commit a
    /// decision made from data that has since changed. It should also avoid
    /// side effects outside the transaction, for the same reason.
    ///
    /// ```no_run
    /// # use slate_kernel::{KvStore, RecordStore, Result, SecurityContext};
    /// # use slate_schema::{Row, TableDef};
    /// # async fn example<S: KvStore>(
    /// #     store: &RecordStore<S>, ctx: &SecurityContext, table: &TableDef, row: &Row,
    /// # ) -> Result<()> {
    /// store
    ///     .transact(async |txn| txn.insert(ctx, table, row).await)
    ///     .await
    /// # }
    /// ```
    pub async fn transact<F, T>(&self, operation: F) -> Result<T>
    where
        // `AsyncFn` rather than a boxed-future bound: the body borrows the
        // transaction it is handed *and* the caller's locals, and a
        // `for<'t> Fn(&'t _) -> BoxFuture<'t, _>` bound forces those locals to
        // be `'static`, which makes the helper unusable for the case it exists
        // to serve.
        F: AsyncFn(&RecordTransaction<'_>) -> Result<T>,
    {
        self.transact_tracked(operation)
            .await
            .map(|(value, _)| value)
    }

    /// [`RecordStore::transact`], also returning the commit's read token.
    ///
    /// Use this when the caller will read its own write back from a replica;
    /// the token is what a replica checks before serving that read.
    pub async fn transact_tracked<F, T>(&self, operation: F) -> Result<(T, Option<ReadToken>)>
    where
        F: AsyncFn(&RecordTransaction<'_>) -> Result<T>,
    {
        with_retries(self.retry, |_attempt| async {
            let txn = self.begin().await?;
            match operation(&txn).await {
                Ok(value) => {
                    let token = txn.commit().await?;
                    Ok((value, token))
                }
                Err(error) => {
                    txn.rollback();
                    Err(error)
                }
            }
        })
        .await
    }

    /// Begin a record-level transaction.
    ///
    /// Prefer [`RecordStore::transact`], which handles the conflict retry that
    /// a correct writer needs anyway.
    pub async fn begin(&self) -> Result<RecordTransaction<'_>> {
        Ok(RecordTransaction {
            txn: self.store.begin().await?,
            catalog: &self.catalog,
            security: &self.security,
            statistics: &self.statistics,
        })
    }
}

/// A transaction over a [`RecordStore`].
///
/// Writes are buffered until [`RecordTransaction::commit`]. Reads see the
/// transaction's own uncommitted writes.
pub struct RecordTransaction<'a> {
    txn: Box<dyn KvTransaction + Send + 'a>,
    catalog: &'a Catalog,
    security: &'a SecurityCatalog,
    statistics: &'a Statistics,
}

impl core::fmt::Debug for RecordTransaction<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecordTransaction").finish_non_exhaustive()
    }
}

impl<'a> RecordTransaction<'a> {
    /// The catalog this transaction resolves tables against.
    #[must_use]
    pub const fn catalog(&self) -> &'a Catalog {
        self.catalog
    }

    /// The access rules this transaction enforces.
    #[must_use]
    pub const fn security(&self) -> &'a SecurityCatalog {
        self.security
    }

    /// The underlying key-value transaction.
    ///
    /// Exposed so the executor can scan without going back through the store.
    #[must_use]
    pub fn raw(&self) -> &(dyn KvTransaction + Send + 'a) {
        self.txn.as_ref()
    }

    /// The read half of this transaction, sharing its snapshot.
    fn reads(&self) -> SecuredReads<'_> {
        SecuredReads {
            snapshot: self.snapshot(),
            security: self.security,
            statistics: self.statistics,
        }
    }

    /// This transaction viewed as a plain snapshot.
    fn snapshot(&self) -> &(dyn KvSnapshot + 'a) {
        self.txn.as_ref()
    }

    /// Read one row by primary key.
    ///
    /// A row the caller's policy hides reads as absent, so this cannot be used
    /// to test whether a forbidden row exists.
    pub async fn get(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        self.reads().get(context, table, primary_key).await
    }

    /// Run `query`, with the caller's security filter folded in.
    ///
    /// The policy is conjoined onto the filter *before* planning, so it can
    /// narrow the scan as well as filter it, and it is re-evaluated on every
    /// candidate row regardless.
    pub async fn execute<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        query: &Query,
    ) -> Result<QueryCursor<'q>> {
        self.reads().execute(context, table, query).await
    }

    /// Every row matching `filter`, in `order`.
    pub async fn query<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<QueryCursor<'q>> {
        self.execute(context, table, &Query::all().filter(filter).order(order))
            .await
    }

    /// [`RecordTransaction::query`], reading only the columns named.
    ///
    /// When an index holds every column the query touches, the row lookup is
    /// skipped entirely. Columns outside the projection come back null.
    pub async fn query_projected<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
        projection: &Projection,
    ) -> Result<QueryCursor<'q>> {
        let mut query = Query::all().filter(filter).order(order);
        query.projection = projection.clone();
        self.execute(context, table, &query).await
    }

    /// The plan `query` would run under, without running it.
    pub fn explain(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<Explanation> {
        let plan = self.reads().plan(context, table, query)?;
        Ok(Explanation::of(table, &plan, query))
    }

    /// Compute `aggregates` over the rows `query` selects.
    ///
    /// The projection is narrowed to exactly the columns the aggregates read,
    /// so an index holding them answers without reading any row —
    /// `COUNT(*)` reads no columns at all.
    pub async fn aggregate(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>> {
        self.reads()
            .aggregate(context, table, query, aggregates)
            .await
    }

    /// Rows matching `query`, counted.
    pub async fn count(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<u64> {
        let values = self
            .aggregate(context, table, query, &[Aggregate::Count])
            .await?;
        Ok(match values.first() {
            Some(Value::U64(n)) => *n,
            _ => 0,
        })
    }

    /// Compute `aggregates` per distinct combination of `group`, ordered by
    /// the grouping columns.
    pub async fn group_by(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>> {
        self.reads()
            .group_by(context, table, query, group, aggregates)
            .await
    }

    /// Collect statistics for `table` by reading it.
    ///
    /// The planner needs to know how many rows a predicate selects, and there
    /// is no way to know without looking. This is the equivalent of `ANALYZE`:
    /// run it after a bulk load, and periodically after that.
    ///
    /// It is an ordinary read, so it sees what `context` is allowed to see.
    /// Statistics gathered under a restrictive policy describe that slice
    /// rather than the table, which would make the planner optimise for the
    /// wrong shape — analyse as a superuser unless you mean otherwise.
    ///
    /// Distinct values are counted exactly up to
    /// [`DISTINCT_TRACKING_LIMIT`]; a column with more than that is treated as
    /// unique, which is the right answer for the identifiers and timestamps
    /// that usually exceed it.
    pub async fn analyze(&self, context: &SecurityContext, table: &TableDef) -> Result<TableStats> {
        let column_count = table.columns().len();
        let mut distinct: Vec<HashSet<Vec<u8>>> = vec![HashSet::new(); column_count];
        let mut overflowed = vec![false; column_count];
        let mut nulls = vec![0u64; column_count];
        let mut row_count = 0u64;

        let mut cursor = self.execute(context, table, &Query::all()).await?;
        while let Some(row) = cursor.next().await? {
            row_count += 1;
            for (ordinal, value) in row.values().iter().enumerate() {
                if value.is_null() {
                    if let Some(count) = nulls.get_mut(ordinal) {
                        *count += 1;
                    }
                    continue;
                }
                let Some(seen) = distinct.get_mut(ordinal) else {
                    continue;
                };
                if overflowed.get(ordinal).copied().unwrap_or(false) {
                    continue;
                }
                if seen.len() >= DISTINCT_TRACKING_LIMIT {
                    // Stop counting and stop paying for the set.
                    seen.clear();
                    seen.shrink_to_fit();
                    if let Some(flag) = overflowed.get_mut(ordinal) {
                        *flag = true;
                    }
                    continue;
                }
                seen.insert(slate_tuple::encode(core::slice::from_ref(value)));
            }
        }

        let mut stats = TableStats::with_row_count(row_count);
        for ordinal in 0..column_count {
            let null_count = nulls.get(ordinal).copied().unwrap_or(0);
            let counted = distinct.get(ordinal).map_or(0, HashSet::len) as u64;
            let distinct_values = if overflowed.get(ordinal).copied().unwrap_or(false) {
                row_count.max(1)
            } else {
                counted.max(1)
            };
            stats = stats.with_column(
                Ordinal(ordinal),
                ColumnStats {
                    distinct: distinct_values,
                    null_fraction: if row_count == 0 {
                        0.0
                    } else {
                        null_count as f64 / row_count as f64
                    },
                },
            );
        }
        Ok(stats)
    }

    /// Read a row with no authorisation or policy applied, for the write paths
    /// that need to see the row they are about to replace.
    async fn read_row_unchecked(
        &self,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        read::read_row_unchecked(self.snapshot(), table, primary_key).await
    }

    /// Insert a row, failing if its primary key is already taken.
    pub async fn insert(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: &Row,
    ) -> Result<()> {
        self.security.authorize(context, table, Action::Insert)?;
        row.validate(table)?;
        self.check_row(context, table, Action::Insert, row)?;

        let primary_key = row.primary_key_values(table);
        if self
            .read_row_unchecked(table, &primary_key)
            .await?
            .is_some()
        {
            return Err(KernelError::DuplicatePrimaryKey {
                table: table.name().to_owned(),
            });
        }
        self.write_row(table, row, None).await
    }

    /// Replace an existing row, failing if there is nothing to replace.
    ///
    /// The row's own primary key values identify which row is being replaced,
    /// so an update never moves a row to a different key.
    pub async fn update(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: &Row,
    ) -> Result<()> {
        self.security.authorize(context, table, Action::Update)?;
        row.validate(table)?;

        let primary_key = row.primary_key_values(table);
        let existing = self
            .visible_row(context, table, Action::Update, &primary_key)
            .await?;
        let Some(existing) = existing else {
            return Err(KernelError::RowNotFound {
                table: table.name().to_owned(),
            });
        };
        // The row the update leaves behind must also be one the caller could
        // have written, or a policy could be escaped by editing your way out.
        self.check_row(context, table, Action::Update, row)?;
        self.write_row(table, row, Some(existing)).await
    }

    /// Insert the row, or replace it if its primary key is already present.
    pub async fn upsert(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: &Row,
    ) -> Result<()> {
        self.security.authorize(context, table, Action::Insert)?;
        self.security.authorize(context, table, Action::Update)?;
        row.validate(table)?;

        let primary_key = row.primary_key_values(table);
        // Read without the policy: a hidden row still occupies the key, so
        // treating it as absent would turn an upsert into a silent overwrite of
        // a row the caller may not touch.
        let existing = self.read_row_unchecked(table, &primary_key).await?;
        let action = if existing.is_some() {
            Action::Update
        } else {
            Action::Insert
        };
        if let Some(current) = &existing
            && !self
                .security
                .permits_row(context, table, Action::Update, current)?
        {
            return Err(KernelError::RowNotFound {
                table: table.name().to_owned(),
            });
        }
        self.check_row(context, table, action, row)?;
        self.write_row(table, row, existing).await
    }

    /// Delete a row and every index entry that pointed at it.
    ///
    /// Returns whether a row was there to delete.
    pub async fn delete(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<bool> {
        self.security.authorize(context, table, Action::Delete)?;
        let existing = self
            .visible_row(context, table, Action::Delete, primary_key)
            .await?;
        let Some(existing) = existing else {
            return Ok(false);
        };
        for index in table.indexes() {
            let entry = self.entry_for(table, index, &existing);
            self.txn.delete(entry.key)?;
        }
        self.txn.delete(keys::row_key(table, primary_key))?;
        Ok(true)
    }

    /// Read a row only if the caller's policy for `action` admits it.
    ///
    /// A row that exists but is hidden comes back as `None`, which is what makes
    /// an update or delete against it report "no such row" rather than
    /// "forbidden" — the latter would confirm the row exists.
    async fn visible_row(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        let Some(row) = self.read_row_unchecked(table, primary_key).await? else {
            return Ok(None);
        };
        if self.security.permits_row(context, table, action, &row)? {
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    /// Postgres's `WITH CHECK`: refuse to store a row the policy would hide.
    fn check_row(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
        row: &Row,
    ) -> Result<()> {
        if self.security.permits_row(context, table, action, row)? {
            Ok(())
        } else {
            Err(KernelError::RowCheckFailed {
                table: table.name().to_owned(),
            })
        }
    }

    /// Write a row and reconcile its index entries against `previous`.
    async fn write_row(&self, table: &TableDef, row: &Row, previous: Option<Row>) -> Result<()> {
        let primary_key = row.primary_key_values(table);

        for index in table.indexes() {
            let new_entry = self.entry_for(table, index, row);
            let old_entry = previous
                .as_ref()
                .map(|old| self.entry_for(table, index, old));

            // An index entry only needs touching when the row's indexed values
            // changed. Rewriting an unchanged key would add a spurious
            // write-write conflict against concurrent writers of other rows
            // that happen to share the slot.
            if old_entry
                .as_ref()
                .is_some_and(|old| old.key == new_entry.key)
            {
                continue;
            }

            if new_entry.enforces_uniqueness {
                self.check_unique(table, index, &new_entry, &primary_key)
                    .await?;
            }
            if let Some(old) = old_entry {
                self.txn.delete(old.key)?;
            }
            self.txn.put(new_entry.key, new_entry.value)?;
        }

        self.txn
            .put(keys::row_key(table, &primary_key), encode_body(table, row))?;
        Ok(())
    }

    /// Reject a write that would put a second row in a unique index slot.
    ///
    /// This read is for the error message, not for the guarantee. A unique
    /// entry's key omits the primary key, so two racing writers produce the same
    /// key and the store's write-write conflict detection settles it regardless
    /// of isolation level. Without this check the loser would see a retryable
    /// conflict instead of being told what it collided with.
    async fn check_unique(
        &self,
        table: &TableDef,
        index: &IndexDef,
        entry: &IndexEntry,
        primary_key: &[Value],
    ) -> Result<()> {
        let Some(existing) = self.txn.get(&entry.key).await? else {
            return Ok(());
        };
        let owner = slate_tuple::decode(&existing, &table.primary_key_types())?;
        if owner != primary_key {
            return Err(KernelError::UniqueViolation {
                table: table.name().to_owned(),
                index: index.name().to_owned(),
            });
        }
        Ok(())
    }

    fn entry_for(&self, table: &TableDef, index: &IndexDef, row: &Row) -> IndexEntry {
        keys::index_entry(
            table,
            index,
            &row.index_values(index),
            &row.primary_key_values(table),
        )
    }

    /// Commit every buffered write atomically.
    ///
    /// Returns the sequence the writes landed at, or `None` if there were none.
    /// Pass it to a later read as [`Freshness::AtLeast`](crate::Freshness) to
    /// read your own writes back from a replica.
    pub async fn commit(self) -> Result<Option<ReadToken>> {
        Ok(self.txn.commit().await?.map(ReadToken::new))
    }

    /// Discard every buffered write.
    pub fn rollback(self) {
        self.txn.rollback();
    }
}

/// A read-only view of a [`RecordStore`].
///
/// This is what a read replica serves. It has no write methods at all rather
/// than write methods that fail, so routing a read to a replica cannot
/// accidentally become an attempt to write to one.
pub struct RecordSnapshot<'a> {
    snapshot: Box<dyn KvSnapshot + Send + 'a>,
    catalog: &'a Catalog,
    security: &'a SecurityCatalog,
    statistics: &'a Statistics,
}

impl core::fmt::Debug for RecordSnapshot<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecordSnapshot").finish_non_exhaustive()
    }
}

impl<'a> RecordSnapshot<'a> {
    /// Wrap a raw snapshot as a secured read view.
    ///
    /// For a router that picks the underlying store per read; see
    /// [`crate::pool::ReplicaPool`].
    #[must_use]
    pub fn over(
        snapshot: Box<dyn KvSnapshot + Send + 'a>,
        catalog: &'a Catalog,
        security: &'a SecurityCatalog,
        statistics: &'a Statistics,
    ) -> Self {
        Self {
            snapshot,
            catalog,
            security,
            statistics,
        }
    }

    /// The catalog this view resolves tables against.
    #[must_use]
    pub const fn catalog(&self) -> &'a Catalog {
        self.catalog
    }

    fn reads(&self) -> SecuredReads<'_> {
        SecuredReads {
            snapshot: self.snapshot.as_ref(),
            security: self.security,
            statistics: self.statistics,
        }
    }

    /// Read one row by primary key, subject to the caller's policy.
    pub async fn get(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        self.reads().get(context, table, primary_key).await
    }

    /// Run `query`, with the caller's security filter folded in.
    pub async fn execute<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        query: &Query,
    ) -> Result<QueryCursor<'q>> {
        self.reads().execute(context, table, query).await
    }

    /// Every row matching `filter`, in `order`.
    pub async fn query<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<QueryCursor<'q>> {
        self.execute(context, table, &Query::all().filter(filter).order(order))
            .await
    }

    /// [`RecordSnapshot::query`], reading only the columns named. See
    /// [`RecordTransaction::query_projected`].
    pub async fn query_projected<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
        projection: &Projection,
    ) -> Result<QueryCursor<'q>> {
        let mut query = Query::all().filter(filter).order(order);
        query.projection = projection.clone();
        self.execute(context, table, &query).await
    }

    /// The plan `query` would run under, without running it.
    pub fn explain(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<Explanation> {
        let plan = self.reads().plan(context, table, query)?;
        Ok(Explanation::of(table, &plan, query))
    }

    /// Compute `aggregates` over the rows `query` selects.
    ///
    /// The projection is narrowed to exactly the columns the aggregates read,
    /// so an index holding them answers without reading any row —
    /// `COUNT(*)` reads no columns at all.
    pub async fn aggregate(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>> {
        self.reads()
            .aggregate(context, table, query, aggregates)
            .await
    }

    /// Rows matching `query`, counted.
    pub async fn count(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<u64> {
        let values = self
            .aggregate(context, table, query, &[Aggregate::Count])
            .await?;
        Ok(match values.first() {
            Some(Value::U64(n)) => *n,
            _ => 0,
        })
    }

    /// Compute `aggregates` per distinct combination of `group`, ordered by
    /// the grouping columns.
    pub async fn group_by(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>> {
        self.reads()
            .group_by(context, table, query, group, aggregates)
            .await
    }
}
