//! Reads, expressed over a snapshot rather than a transaction.
//!
//! Everything here needs only [`KvSnapshot`], which is what lets the same code
//! serve a writer's transaction and a read replica. The security checks live
//! here too, so a replica cannot be a way around them: there is no read path
//! that does not go through the secured reads in this module.

use crate::error::Result;
use crate::exec::QueryCursor;
use crate::expr::Expr;
use crate::keys;
use crate::plan::plan;
use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::store::{KeyRange, KvIterator, KvSnapshot, ScanOrder};
use slate_schema::{IndexDef, Row, TableDef, decode_row};
use slate_tuple::Value;

/// Read a row with no authorisation or policy applied.
///
/// Internal. The executor uses it to follow an index entry it has already
/// earned the right to read, and the write paths use it to see the row they are
/// about to replace.
pub(crate) async fn read_row_unchecked(
    snapshot: &dyn KvSnapshot,
    table: &TableDef,
    primary_key: &[Value],
) -> Result<Option<Row>> {
    let key = keys::row_key(table, primary_key);
    let Some(body) = snapshot.get(&key).await? else {
        return Ok(None);
    };
    Ok(Some(decode_row(table, primary_key, &body)?))
}

/// The read half of the record layer, bound to one snapshot.
///
/// `Copy` so a caller can hand it out by value and still return cursors that
/// borrow the underlying snapshot for its full lifetime.
#[derive(Clone, Copy)]
pub(crate) struct SecuredReads<'a> {
    pub(crate) snapshot: &'a dyn KvSnapshot,
    pub(crate) security: &'a SecurityCatalog,
}

impl<'a> SecuredReads<'a> {
    /// Read one row by primary key, hiding it if the policy does.
    pub(crate) async fn get(
        self,
        context: &SecurityContext,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        self.security.authorize(context, table, Action::Read)?;
        let filter = self.security.row_filter(context, table, Action::Read)?;
        Ok(read_row_unchecked(self.snapshot, table, primary_key)
            .await?
            .filter(|row| filter.admits(row)))
    }

    /// Plan and run a query with the caller's security filter folded in.
    pub(crate) async fn query(
        self,
        context: &SecurityContext,
        table: &'a TableDef,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<QueryCursor<'a>> {
        self.security.authorize(context, table, Action::Read)?;
        let secured = filter.and(self.security.row_filter(context, table, Action::Read)?);
        QueryCursor::open(self.snapshot, table, plan(table, &secured, order)).await
    }
}

/// Scan rows of `table` over `range`, with no policy applied.
pub(crate) async fn scan_rows<'a>(
    snapshot: &'a dyn KvSnapshot,
    table: &'a TableDef,
    range: KeyRange,
    order: ScanOrder,
) -> Result<RowCursor<'a>> {
    Ok(RowCursor {
        inner: snapshot.scan(range, order).await?,
        table,
    })
}

/// Scan `index` over `range`, with no policy applied.
pub(crate) async fn scan_index<'a>(
    snapshot: &'a dyn KvSnapshot,
    table: &'a TableDef,
    index: &'a IndexDef,
    range: KeyRange,
    order: ScanOrder,
) -> Result<IndexCursor<'a>> {
    Ok(IndexCursor {
        inner: snapshot.scan(range, order).await?,
        table,
        index,
    })
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
