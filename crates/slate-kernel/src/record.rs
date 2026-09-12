//! The record store: primary key operations with atomic index maintenance.
//!
//! Every write puts the row and all of its index entries into one transaction,
//! so an index can never lag the table it describes. There is no background
//! index builder to fall behind and no repair path to get wrong: either the
//! whole write lands or none of it does.

use crate::error::{KernelError, Result};
use crate::exec::QueryCursor;
use crate::expr::Expr;
use crate::keys::{self, IndexEntry};
use crate::plan::plan;
use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::store::{KeyRange, KvIterator, KvStore, KvTransaction, ScanOrder};
use slate_schema::{Catalog, IndexDef, Row, TableDef, decode_row, encode_body};
use slate_tuple::Value;

/// A typed record store over a key-value backend.
#[derive(Debug)]
pub struct RecordStore<S> {
    store: S,
    catalog: Catalog,
    security: SecurityCatalog,
}

impl<S: KvStore> RecordStore<S> {
    /// Create a record store over `store`, serving `catalog` under `security`.
    ///
    /// An empty [`SecurityCatalog`] denies every non-superuser action, so a
    /// store wired up without rules is closed rather than open.
    pub const fn new(store: S, catalog: Catalog, security: SecurityCatalog) -> Self {
        Self {
            store,
            catalog,
            security,
        }
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

    /// Begin a record-level transaction.
    pub async fn begin(&self) -> Result<RecordTransaction<'_>> {
        Ok(RecordTransaction {
            txn: self.store.begin().await?,
            catalog: &self.catalog,
            security: &self.security,
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
        self.security.authorize(context, table, Action::Read)?;
        let filter = self.security.row_filter(context, table, Action::Read)?;
        Ok(self
            .read_row_unchecked(table, primary_key)
            .await?
            .filter(|row| filter.admits(row)))
    }

    /// Read a row with no authorisation or policy applied.
    ///
    /// Internal: the executor uses it to follow an index entry it has already
    /// earned the right to read, and the write paths use it to see the row they
    /// are about to replace. Every public entry point applies the policy.
    pub(crate) async fn read_row_unchecked(
        &self,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        let key = keys::row_key(table, primary_key);
        let Some(body) = self.txn.get(&key).await? else {
            return Ok(None);
        };
        Ok(Some(decode_row(table, primary_key, &body)?))
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

    /// Plan and run a query, with the caller's security filter folded in.
    ///
    /// This is the only way to read more than one row. The policy is conjoined
    /// onto `filter` *before* planning, so it can narrow the scan as well as
    /// filter it, and it is re-evaluated on every candidate row regardless.
    pub async fn query<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<QueryCursor<'q>> {
        self.security.authorize(context, table, Action::Read)?;
        let secured = filter.and(self.security.row_filter(context, table, Action::Read)?);
        QueryCursor::open(self, table, plan(table, &secured, order)).await
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

    /// Scan rows of `table` over `range`, with no policy applied.
    ///
    /// Internal: the executor calls this with bounds the planner derived from an
    /// already-secured predicate. Callers go through
    /// [`RecordTransaction::query`].
    pub(crate) async fn scan_rows(
        &self,
        table: &'a TableDef,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<RowCursor<'_>> {
        Ok(RowCursor {
            inner: self.txn.scan(range, order).await?,
            table,
        })
    }

    /// Scan `index` over `range`, with no policy applied. Internal, as with
    /// [`RecordTransaction::scan_rows`].
    pub(crate) async fn scan_index(
        &self,
        table: &'a TableDef,
        index: &'a IndexDef,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<IndexCursor<'_>> {
        Ok(IndexCursor {
            inner: self.txn.scan(range, order).await?,
            table,
            index,
        })
    }

    /// Commit every buffered write atomically.
    pub async fn commit(self) -> Result<()> {
        self.txn.commit().await
    }

    /// Discard every buffered write.
    pub fn rollback(self) {
        self.txn.rollback();
    }
}

/// A cursor over decoded rows.
pub struct RowCursor<'a> {
    inner: Box<dyn KvIterator + Send + 'a>,
    table: &'a TableDef,
}

impl core::fmt::Debug for RowCursor<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RowCursor")
            .field("table", &self.table.name())
            .finish_non_exhaustive()
    }
}

impl RowCursor<'_> {
    /// The next row, or `None` at the end of the range.
    pub async fn next(&mut self) -> Result<Option<Row>> {
        let Some(kv) = self.inner.next().await? else {
            return Ok(None);
        };
        let primary_key = keys::decode_row_key(self.table, &kv.key)?;
        Ok(Some(decode_row(self.table, &primary_key, &kv.value)?))
    }

    /// Drain the cursor into a vector.
    pub async fn collect(mut self) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        while let Some(row) = self.next().await? {
            out.push(row);
        }
        Ok(out)
    }
}

/// A cursor over decoded index entries.
pub struct IndexCursor<'a> {
    inner: Box<dyn KvIterator + Send + 'a>,
    table: &'a TableDef,
    index: &'a IndexDef,
}

impl core::fmt::Debug for IndexCursor<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IndexCursor")
            .field("index", &self.index.name())
            .finish_non_exhaustive()
    }
}

impl IndexCursor<'_> {
    /// The next entry as `(indexed values, primary key)`.
    pub async fn next(&mut self) -> Result<Option<(Vec<Value>, Vec<Value>)>> {
        let Some(kv) = self.inner.next().await? else {
            return Ok(None);
        };
        Ok(Some(keys::decode_index_entry(
            self.table, self.index, &kv.key, &kv.value,
        )?))
    }

    /// Drain the cursor into a vector.
    pub async fn collect(mut self) -> Result<Vec<(Vec<Value>, Vec<Value>)>> {
        let mut out = Vec::new();
        while let Some(entry) = self.next().await? {
            out.push(entry);
        }
        Ok(out)
    }
}
