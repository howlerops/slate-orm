//! Reads, expressed over a snapshot rather than a transaction.
//!
//! Everything here needs only [`KvSnapshot`], which is what lets the same code
//! serve a writer's transaction and a read replica. The security checks live
//! here too, so a replica cannot be a way around them: there is no read path
//! that does not go through the secured reads in this module.

use crate::aggregate::{Accumulators, Aggregate, Group};
use crate::error::Result;
use crate::exec::QueryCursor;
use crate::keys;
use crate::plan::{Plan, Projection, plan_full};
use crate::query::Query;
use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::stats::Statistics;
use crate::store::{KeyRange, KvIterator, KvSnapshot, ScanOrder};
use slate_schema::{IndexDef, Ordinal, Row, TableDef, decode_row};
use slate_tuple::Value;
use std::collections::BTreeMap;

/// Restrict a query to the columns an aggregation actually reads.
///
/// This is where the win is: `COUNT(*)` needs no columns, so any usable index
/// can answer it without touching a row. A limit or offset is dropped, since
/// aggregating a windowed subset of an unordered result is not a meaningful
/// request.
fn narrowed(query: &Query, aggregates: &[Aggregate], group: &[Ordinal]) -> Query {
    let mut columns = Aggregate::columns(aggregates);
    columns.extend(group.iter().copied());
    Query {
        filter: query.filter.clone(),
        order: query.order,
        projection: Projection::Columns(columns.into_iter().collect()),
        sort: Vec::new(),
        limit: None,
        offset: 0,
    }
}

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
    pub(crate) statistics: &'a Statistics,
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

    /// Plan `query` with the caller's security filter folded in.
    pub(crate) fn plan(
        self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<Plan> {
        self.security.authorize(context, table, Action::Read)?;
        // Conjoining the policy *before* planning is what lets it narrow the
        // scan; it also means a policy on a column the index lacks correctly
        // prevents an index-only scan rather than being skipped by one.
        let secured =
            query
                .filter
                .clone()
                .and(self.security.row_filter(context, table, Action::Read)?);
        Ok(plan_full(
            table,
            &secured,
            query.order,
            &query.projection,
            &self.statistics.table(table),
            query.planning_limit(),
            &query.sort,
        ))
    }

    /// Compute `aggregates` over the rows `query` selects.
    pub(crate) async fn aggregate(
        self,
        context: &SecurityContext,
        table: &'a TableDef,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>> {
        let mut cursor = self
            .execute(context, table, &narrowed(query, aggregates, &[]))
            .await?;
        let mut accumulators = Accumulators::new(aggregates);
        while let Some(row) = cursor.next().await? {
            accumulators.push(&row)?;
        }
        Ok(accumulators.finish())
    }

    /// Compute `aggregates` per distinct combination of `group`.
    pub(crate) async fn group_by(
        self,
        context: &SecurityContext,
        table: &'a TableDef,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>> {
        let mut cursor = self
            .execute(context, table, &narrowed(query, aggregates, group))
            .await?;

        // Grouped in a map rather than by sorting first: the input is not
        // ordered by the grouping columns in general, and requiring that would
        // mean sorting every row to save a hash lookup per row.
        let mut groups: BTreeMap<Vec<Value>, Accumulators> = BTreeMap::new();
        while let Some(row) = cursor.next().await? {
            let key: Vec<Value> = group
                .iter()
                .map(|c| row.get(*c).cloned().unwrap_or(Value::Null))
                .collect();
            groups
                .entry(key)
                .or_insert_with(|| Accumulators::new(aggregates))
                .push(&row)?;
        }

        // `BTreeMap` gives group order for free, and a deterministic result is
        // worth more than the constant factor a hash map would save.
        Ok(groups
            .into_iter()
            .map(|(key, accumulators)| Group {
                key,
                values: accumulators.finish(),
            })
            .collect())
    }

    /// Plan and run a query with the caller's security filter folded in.
    pub(crate) async fn execute(
        self,
        context: &SecurityContext,
        table: &'a TableDef,
        query: &Query,
    ) -> Result<QueryCursor<'a>> {
        let plan = self.plan(context, table, query)?;
        let cursor = QueryCursor::open(self.snapshot, table, plan).await?;
        Ok(cursor.with_window(query.limit, query.offset))
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
