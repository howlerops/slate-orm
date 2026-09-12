//! Reads, expressed over a snapshot rather than a transaction.
//!
//! Everything here needs only [`KvSnapshot`], which is what lets the same code
//! serve a writer's transaction and a read replica. The security checks live
//! here too, so a replica cannot be a way around them: there is no read path
//! that does not go through the secured reads in this module.

use crate::aggregate::{Accumulators, Aggregate, Group};
use crate::error::Result;
use crate::exec::QueryCursor;
use crate::expr::Expr;
use crate::join::{self, Join, JoinAlgorithm, JoinCursor, JoinPlan, JoinType, Side};
use crate::keys;
use crate::plan::{Plan, Projection, plan_full};
use crate::query::Query;
use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::stats::Statistics;
use crate::store::{KeyRange, KvIterator, KvSnapshot, ScanOrder};
use bytes::Bytes;
use slate_schema::{IndexDef, Ordinal, Row, TableDef, decode_row};
use slate_tuple::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

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

/// A row as it is stored: its key already decoded, its body still bytes.
///
/// Kept undecoded so the executor can filter on a few columns before paying to
/// materialise the rest.
#[derive(Debug, Clone)]
pub struct RawRow {
    /// The primary key, decoded from the key.
    pub primary_key: Vec<Value>,
    /// The row body, still encoded.
    pub body: Bytes,
}

/// Read a row's stored body without decoding it.
pub(crate) async fn read_row_body(
    snapshot: &dyn KvSnapshot,
    table: &TableDef,
    primary_key: &[Value],
) -> Result<Option<Bytes>> {
    snapshot.get(&keys::row_key(table, primary_key)).await
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
        let secured = Arc::new(query.filter.clone().and(self.security.row_filter(
            context,
            table,
            Action::Read,
        )?));
        Ok(plan_full(
            table,
            secured,
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

    /// Choose how to join two tables.
    ///
    /// Both sides are planned through [`SecuredReads::plan`], so both are
    /// authorised and both carry their own row filter before anything is
    /// costed. A join is not a privileged read; it is two of them.
    pub(crate) fn plan_join(
        self,
        context: &SecurityContext,
        left_table: &TableDef,
        right_table: &TableDef,
        join: &Join,
    ) -> Result<JoinPlan> {
        join.validate(left_table, right_table)?;

        let left = self.plan(context, left_table, &join.left)?;
        let right = self.plan(context, right_table, &join.right)?;

        let left_stats = self.statistics.table(left_table);
        let right_stats = self.statistics.table(right_table);
        let estimated_rows = join::join_cardinality(
            left.estimated_rows,
            right.estimated_rows,
            join::distinct_over(&left_stats, &join.columns(Side::Left)),
            join::distinct_over(&right_stats, &join.columns(Side::Right)),
        );
        let per_row = estimated_rows * join::JOIN_ROW_COST;

        // A probe is the right side read with the join equality bound to one
        // outer row. Costing it needs a plan, and a plan needs a literal; the
        // literal does not matter, because equality selectivity comes from the
        // column's distinct count rather than from the value.
        let mut probe_query = join.right.clone();
        probe_query.filter = core::mem::replace(&mut probe_query.filter, Expr::True)
            .and(join::probe_shape(&join.on));
        let probe = self.plan(context, right_table, &probe_query)?;

        let hash_cost = join::hash_cost(&left, &right) + per_row;
        let loop_cost = join::nested_loop_cost(
            &left,
            &Plan {
                estimated_cost: join::probe_floor(probe.estimated_cost),
                ..probe.clone()
            },
        ) + per_row;

        // Build the smaller side: the cost is the same either way — both sides
        // are read once — so the choice is about memory, not round trips. A
        // left outer join has no choice, since every left row must be emitted
        // whether it matched or not, and only the streaming side can do that.
        let build =
            if join.join_type == JoinType::Left || left.estimated_rows <= right.estimated_rows {
                Side::Right
            } else {
                Side::Left
            };

        let (algorithm, estimated_cost) = match join.force {
            Some(forced @ JoinAlgorithm::Hash { .. }) => (forced, hash_cost),
            Some(JoinAlgorithm::NestedLoop) => (JoinAlgorithm::NestedLoop, loop_cost),
            // Ties go to the hash join: it reads each side once whatever the
            // estimate turns out to be, where a loop that was estimated at ten
            // outer rows and finds ten thousand costs ten thousand round trips.
            None if loop_cost < hash_cost => (JoinAlgorithm::NestedLoop, loop_cost),
            None => (JoinAlgorithm::Hash { build }, hash_cost),
        };

        Ok(JoinPlan {
            algorithm,
            left,
            right,
            estimated_rows,
            estimated_cost,
        })
    }

    /// Plan and run a join.
    pub(crate) async fn join(
        self,
        context: &SecurityContext,
        left_table: &'a TableDef,
        right_table: &'a TableDef,
        join: &Join,
    ) -> Result<JoinCursor<'a>> {
        let plan = self.plan_join(context, left_table, right_table, join)?;
        JoinCursor::open(self, context, left_table, right_table, join, &plan).await
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
    /// The next row, still encoded.
    pub async fn next_raw(&mut self) -> Result<Option<RawRow>> {
        let Some(kv) = self.inner.next().await? else {
            return Ok(None);
        };
        Ok(Some(RawRow {
            primary_key: keys::decode_row_key(self.table, &kv.key)?,
            body: kv.value,
        }))
    }

    /// The next row, fully decoded.
    pub async fn next(&mut self) -> Result<Option<Row>> {
        match self.next_raw().await? {
            None => Ok(None),
            Some(raw) => Ok(Some(decode_row(self.table, &raw.primary_key, &raw.body)?)),
        }
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
