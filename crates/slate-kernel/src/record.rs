//! The record store: primary key operations with atomic index maintenance.
//!
//! Every write puts the row and all of its index entries into one transaction,
//! so an index can never lag the table it describes. There is no background
//! index builder to fall behind and no repair path to get wrong: either the
//! whole write lands or none of it does.

use crate::error::{KernelError, Result};
use crate::keys::{self, IndexEntry};
use crate::store::{KeyRange, KvIterator, KvStore, KvTransaction, ScanOrder};
use slate_schema::{Catalog, IndexDef, Row, TableDef, decode_row, encode_body};
use slate_tuple::Value;

/// A typed record store over a key-value backend.
#[derive(Debug)]
pub struct RecordStore<S> {
    store: S,
    catalog: Catalog,
}

impl<S: KvStore> RecordStore<S> {
    /// Create a record store over `store`, serving the tables in `catalog`.
    pub const fn new(store: S, catalog: Catalog) -> Self {
        Self { store, catalog }
    }

    /// The catalog this store serves.
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
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

    /// The underlying key-value transaction.
    ///
    /// Exposed so the executor can scan without going back through the store.
    #[must_use]
    pub fn raw(&self) -> &(dyn KvTransaction + Send + 'a) {
        self.txn.as_ref()
    }

    /// Read one row by primary key.
    pub async fn get(&self, table: &TableDef, primary_key: &[Value]) -> Result<Option<Row>> {
        let key = keys::row_key(table, primary_key);
        let Some(body) = self.txn.get(&key).await? else {
            return Ok(None);
        };
        Ok(Some(decode_row(table, primary_key, &body)?))
    }

    /// Insert a row, failing if its primary key is already taken.
    pub async fn insert(&self, table: &TableDef, row: &Row) -> Result<()> {
        row.validate(table)?;
        let primary_key = row.primary_key_values(table);
        if self.get(table, &primary_key).await?.is_some() {
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
    pub async fn update(&self, table: &TableDef, row: &Row) -> Result<()> {
        row.validate(table)?;
        let primary_key = row.primary_key_values(table);
        let Some(existing) = self.get(table, &primary_key).await? else {
            return Err(KernelError::RowNotFound {
                table: table.name().to_owned(),
            });
        };
        self.write_row(table, row, Some(existing)).await
    }

    /// Insert the row, or replace it if its primary key is already present.
    pub async fn upsert(&self, table: &TableDef, row: &Row) -> Result<()> {
        row.validate(table)?;
        let primary_key = row.primary_key_values(table);
        let existing = self.get(table, &primary_key).await?;
        self.write_row(table, row, existing).await
    }

    /// Delete a row and every index entry that pointed at it.
    ///
    /// Returns whether a row was there to delete.
    pub async fn delete(&self, table: &TableDef, primary_key: &[Value]) -> Result<bool> {
        let Some(existing) = self.get(table, primary_key).await? else {
            return Ok(false);
        };
        for index in table.indexes() {
            let entry = self.entry_for(table, index, &existing);
            self.txn.delete(entry.key)?;
        }
        self.txn.delete(keys::row_key(table, primary_key))?;
        Ok(true)
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

    /// Scan rows of `table` over `range`, which must be a sub-range of the
    /// table's own key prefix.
    pub async fn scan_rows(
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

    /// Scan `index` over `range`, yielding the indexed values and the primary
    /// key of each entry.
    pub async fn scan_index(
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
