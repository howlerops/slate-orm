//! Reads, expressed over a snapshot rather than a transaction.
//!
//! Everything here needs only [`KvSnapshot`], which is what lets the same code
//! serve a writer's transaction and a read replica. The security checks live
//! here too, so a replica cannot be a way around them: there is no read path
//! that does not go through the secured reads in this module.

use crate::aggregate::{Accumulators, Aggregate, Group, Grouper, Grouping};
use crate::chain::{self, Chain, ChainCursor, ChainPlan, JoinStepPlan};
use crate::error::{KernelError, Result};
use crate::exec::QueryCursor;
use crate::expr::Expr;
use crate::join::{self, Join, JoinAlgorithm, JoinCursor, JoinKey, JoinPlan, JoinSchema, Side};
use crate::keys;
use crate::limits::ExecutionLimits;
use crate::plan::{Plan, Projection, plan_hinted};
use crate::query::Query;
use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::stats::Statistics;
use crate::store::{KeyRange, KvIterator, KvSnapshot, ScanOrder};
use bytes::Bytes;
use slate_schema::{ColumnDef, IndexDef, Ordinal, Row, TableDef, decode_row};
use slate_tuple::Value;
use std::collections::BTreeSet;
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
        hint: query.hint,
        compute: query.compute.clone(),
    }
}

/// The join to run for a grouped read: the same join, with each side reading
/// only the columns the grouping and the join itself actually need.
///
/// The mapping is the only fiddly part, and it is fiddly in a way that shows
/// up as a null rather than an error, so the grouped-join oracle compares
/// against a join materialised with no projection at all. What each side has
/// to produce:
///
/// - the grouping columns and the aggregates' columns, which are in the joined
///   space and are resolved back to a side by [`JoinSchema::resolve`];
/// - the columns `Join::having` reads, for the same reason — the pair is
///   rejected or admitted after both sides are read;
/// - the join keys, which are already in each side's own ordinals and are what
///   the hash build and the probe compare.
///
/// A side's `sort`, `limit` and `offset` are already ignored by the join, and
/// the grouping's own `sort` names a *group*, not a row, so neither adds
/// anything here.
fn narrowed_join(join: &Join, schema: &JoinSchema, grouping: &Grouping) -> Join {
    let mut wanted = grouping.columns();
    wanted.extend(join.having.columns());
    let mut left: BTreeSet<Ordinal> = BTreeSet::new();
    let mut right: BTreeSet<Ordinal> = BTreeSet::new();
    for ordinal in wanted {
        match schema.resolve(ordinal) {
            Some((Side::Left, at)) => {
                left.insert(at);
            }
            Some((Side::Right, at)) => {
                right.insert(at);
            }
            // Outside the joined space. Nothing to read for it, and the row it
            // would have come from does not exist; it reads as null on every
            // path alike.
            None => {}
        }
    }
    for key in &join.on {
        left.insert(key.left);
        right.insert(key.right);
    }
    let mut narrowed = join.clone();
    narrowed.left.projection = Projection::Columns(left.into_iter().collect());
    narrowed.right.projection = Projection::Columns(right.into_iter().collect());

    // And the window goes, for the reason [`narrowed`] gives for dropping a
    // single-table query's: aggregating a windowed subset of an unordered
    // result is not a meaningful request. This path used to clone the join and
    // replace only the two projections, so a join's own `limit` survived and
    // windowed the row stream before the grouper saw it — and a join has no
    // order, so *which* rows the window kept was whichever ones the chosen
    // algorithm happened to produce first.
    //
    // It surfaced as the two hash build sides disagreeing: the same grouped
    // join came back as counts of 4/2/4/2 building the right side and 3/3/3/3
    // building the left, both entirely plausible-looking tables. The single-
    // table path had already decided this case is meaningless and refused it;
    // the join path simply forgot to, which made a deliberate rule look like
    // an accident of which plan won.
    narrowed.limit = None;
    narrowed.offset = 0;
    narrowed
}

/// The chain to run for a grouped read: every step reading only the columns
/// something downstream will take *out of its row*.
///
/// The two-table version, [`narrowed_join`], has one condition and knows it up
/// front. A chain does not: a step's `having` may name any table read before
/// it, and a step's join key names an earlier table in the joined space. So the
/// set of columns a table must produce is gathered from every step, not from
/// the grouping alone. Narrowing to the grouping's columns and stopping there
/// reads away a column a later step still needs, and the symptom is a null
/// rather than an error.
///
/// What has to survive, per table:
///
/// - the grouping's columns, in the joined space;
/// - every step's `having` columns, also joined-space, because the condition is
///   evaluated over the accumulated row after the pair is formed;
/// - every step's join keys — the `left` side in the joined space, the `right`
///   side already in that step's own ordinals.
///
/// A `filter` needs nothing here: the projection says what a row *carries*, and
/// the planner computes what to *decode* from the secured predicate, so a
/// filter on an unprojected column still works. That is late materialisation
/// doing its job, and it is why this list is shorter than it looks.
///
/// This was described in yesterday's note as needing "the transitive closure of
/// every step's references". It does not. Every reference is written in the
/// joined space, which is absolute — needing column 7 does not create a need
/// for some other column — so one pass collects them all. The closure was a
/// worry about a shape the design does not have.
fn narrowed_chain(chain: &Chain, schema: &JoinSchema, grouping: &Grouping) -> Chain {
    let mut wanted: Vec<BTreeSet<Ordinal>> = vec![BTreeSet::new(); schema.len()];
    let mut want = |joined: Ordinal, into: &mut Vec<BTreeSet<Ordinal>>| {
        // Outside the joined space: nothing to read for it, and it reads as
        // null on every path alike. Same rule the two-table version applies.
        if let Some((position, at)) = schema.locate(joined) {
            if let Some(set) = into.get_mut(position) {
                set.insert(at);
            }
        }
    };

    for ordinal in grouping.columns() {
        want(ordinal, &mut wanted);
    }
    for (index, step) in chain.steps.iter().enumerate() {
        // Step `index` adds the table at position `index + 1`; the first table
        // is the chain's own `first`.
        let position = index + 1;
        for ordinal in step.having.columns() {
            want(ordinal, &mut wanted);
        }
        for key in &step.on {
            want(key.left, &mut wanted);
            if let Some(set) = wanted.get_mut(position) {
                set.insert(key.right);
            }
        }
    }

    let mut narrowed = chain.clone();
    if let Some(first) = wanted.first() {
        narrowed.first.projection = Projection::Columns(first.iter().copied().collect());
    }
    for (index, step) in narrowed.steps.iter_mut().enumerate() {
        if let Some(set) = wanted.get(index + 1) {
            step.query.projection = Projection::Columns(set.iter().copied().collect());
        }
    }

    // And the window goes, for the reason `narrowed_join` gives at length:
    // aggregating a windowed subset of an unordered result is not a meaningful
    // request, and leaving it in made two join algorithms disagree about the
    // same grouped join. A chain has no order either.
    narrowed.limit = None;
    narrowed.offset = 0;
    narrowed
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

/// [`read_row_unchecked`], decoding only the columns the query asked for.
///
/// A point get used to decode every column regardless of the projection, which
/// made a row's contents depend on whether the planner reached it by key or by
/// index. The planner oracle caught it: the same query returned five populated
/// columns through a point get and two through an index scan.
pub(crate) async fn read_row_projected(
    snapshot: &dyn KvSnapshot,
    table: &TableDef,
    primary_key: &[Value],
    wanted: &slate_schema::ColumnSet,
) -> Result<Option<Row>> {
    let key = keys::row_key(table, primary_key);
    let Some(body) = snapshot.get(&key).await? else {
        return Ok(None);
    };
    Ok(Some(slate_schema::decode_row_columns(
        table,
        primary_key,
        &body,
        Some(wanted),
    )?))
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
    pub(crate) limits: ExecutionLimits,
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

    /// Check the caller may see a *plan* for every table involved.
    ///
    /// Separate from the [`Action::Read`] each plan already authorises, and
    /// checked on every table a multi-table plan touches rather than on the
    /// first: an explanation reports an estimate per side, so a caller
    /// permitted to explain one table would otherwise read statistics off the
    /// other by joining to it.
    ///
    /// A plan is costed against statistics gathered over the whole table, so
    /// its estimates describe rows the caller's row policy may hide. See
    /// [`Action::Explain`].
    pub(crate) fn authorize_explain(
        self,
        context: &SecurityContext,
        tables: &[&TableDef],
    ) -> Result<()> {
        for table in tables {
            self.security.authorize(context, table, Action::Explain)?;
        }
        Ok(())
    }

    /// Plan `query` with the caller's security filter folded in.
    pub(crate) fn plan(
        self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<Plan> {
        self.security.authorize(context, table, Action::Read)?;
        // A comparison between two columns of different types would order by
        // type rather than by value and answer the same way for every row.
        // Only the new variant can trip this, so no query that planned before
        // stops planning now. See `Expr::CompareColumns`.
        if let Some((a, b)) = query
            .filter
            .column_type_conflict(&|column| table.column(column).map(ColumnDef::value_type))
        {
            return Err(KernelError::ComparisonTypeMismatch {
                at: table.name().to_owned(),
                left: a,
                right: b,
            });
        }
        // Conjoining the policy *before* planning is what lets it narrow the
        // scan; it also means a policy on a column the index lacks correctly
        // prevents an index-only scan rather than being skipped by one.
        let secured = Arc::new(query.filter.clone().and(self.security.row_filter(
            context,
            table,
            Action::Read,
        )?));
        Ok(plan_hinted(
            table,
            secured,
            query.order,
            &query.projection,
            &self.statistics.table(table),
            query.planning_limit(),
            &query.sort,
            query.hint,
            &query.compute,
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
        let mut accumulators = Accumulators::with_limits(aggregates, self.limits);
        while let Some(row) = cursor.next().await? {
            accumulators.push(&row)?;
        }
        Ok(accumulators.finish())
    }

    /// Group the rows `query` selects.
    ///
    /// The rows come from an ordinary secured cursor and the grouping is
    /// [`Grouper`]'s, which is also what a grouped join uses. One
    /// implementation, two sources.
    pub(crate) async fn grouped(
        self,
        context: &SecurityContext,
        table: &'a TableDef,
        query: &Query,
        grouping: &Grouping,
    ) -> Result<Vec<Group>> {
        let columns: Vec<Ordinal> = grouping.group.clone();
        let mut cursor = self
            .execute(
                context,
                table,
                &narrowed(query, &grouping.aggregates, &columns),
            )
            .await?;
        let mut grouper = Grouper::with_limits(grouping, self.limits);
        while let Some(row) = cursor.next().await? {
            grouper.push(&row)?;
        }
        Ok(grouper.finish())
    }

    /// Group the rows a *join* produces.
    ///
    /// Grouping sits above the join rather than beside it: it consumes the
    /// joined row stream, in the ordinal space [`JoinSchema`] defines, which is
    /// the same space `Join::having` is written in. A group key can therefore
    /// span both sides, which is the whole reason this is not two calls.
    ///
    /// Both sides are still planned and secured individually — this is the
    /// join's own cursor, not a privileged read — so a policy on either side
    /// applies to the groups exactly as it applies to the rows.
    pub(crate) async fn grouped_join(
        self,
        context: &SecurityContext,
        left_table: &'a TableDef,
        right_table: &'a TableDef,
        join: &Join,
        grouping: &Grouping,
    ) -> Result<Vec<Group>> {
        let schema = JoinSchema::of(left_table, right_table);
        // Each side reads only what the grouping and the join itself need,
        // which is what lets an index-only scan serve a grouped join the way it
        // already serves a grouped table scan. A side's limit and offset are
        // ignored here as they are ignored everywhere else in a join: they
        // would change the answer rather than page it.
        let narrowed = narrowed_join(join, &schema, grouping);
        let plan = self.plan_join(context, left_table, right_table, &narrowed)?;
        let mut cursor =
            JoinCursor::open(self, context, left_table, right_table, &narrowed, &plan).await?;

        let mut grouper = Grouper::with_limits(grouping, self.limits);
        while let Some(joined) = cursor.next().await? {
            // Flattened rather than grouped through a two-sided view, because
            // the accumulators take a `Row` and a second row-like type
            // threaded through them would be a second place for the null rules
            // to drift. `JoinedRow::flatten` reports a missing side as nulls,
            // which is what the view reports too.
            grouper.push(&joined.flatten(&schema))?;
        }
        Ok(grouper.finish())
    }

    /// Group the rows a *chain* produces.
    ///
    /// The n-way generalisation of [`SecuredReads::grouped_join`], and it sits
    /// above the chain the same way: it consumes the chain's own cursor, in the
    /// space [`JoinSchema::over`] defines, so a group key may name any table in
    /// the chain and every step's security still applies.
    ///
    /// Each step reads only the columns something downstream takes out of its
    /// row — see [`narrowed_chain`], which gathers them from every step rather
    /// than from the grouping alone, because a step's condition may name any
    /// table read before it.
    pub(crate) async fn grouped_chain(
        self,
        context: &SecurityContext,
        tables: &[&'a TableDef],
        chain: &Chain,
        grouping: &Grouping,
    ) -> Result<Vec<Group>> {
        let schema = Arc::new(JoinSchema::over(tables.iter().copied()));
        let narrowed = narrowed_chain(chain, &schema, grouping);
        let plan = self.plan_chain(context, tables, &narrowed, &schema)?;
        let mut cursor =
            chain::run(self, context, tables, &narrowed, &plan, Arc::clone(&schema)).await?;

        let mut grouper = Grouper::with_limits(grouping, self.limits);
        while let Some(row) = cursor.next().await? {
            // Flattened for the same reason the two-table version flattens: the
            // accumulators take a `Row`, and a second row-like type threaded
            // through them would be a second place for the null rules to drift.
            grouper.push(&row.flatten(&schema))?;
        }
        Ok(grouper.finish())
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
        let schema = JoinSchema::of(left_table, right_table);
        let estimated_rows =
            join::join_cardinality(
                left.estimated_rows,
                right.estimated_rows,
                join::distinct_over(&left_stats, &join.columns(Side::Left)),
                join::distinct_over(&right_stats, &join.columns(Side::Right)),
            ) * join::having_selectivity(&schema, &join.having, &left_stats, &right_stats);
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
        // are read once — so the choice is about memory, not round trips. The
        // join type does not constrain it, because an unmatched built row is
        // drained once the probe side runs out rather than needing to have
        // been the streaming side.
        let build = if left.estimated_rows <= right.estimated_rows {
            Side::Right
        } else {
            Side::Left
        };

        // A loop streams the left side and probes the right, so it learns
        // which *left* rows matched nothing and never learns anything about a
        // right row it did not fetch. Preserving the right side would mean
        // reading all of it, which is a hash join with extra steps.
        let loop_possible = !join.join_type.preserves(Side::Right);

        let (algorithm, estimated_cost) = match join.force {
            Some(forced @ JoinAlgorithm::Hash { .. }) => (forced, hash_cost),
            Some(JoinAlgorithm::NestedLoop) if !loop_possible => {
                return Err(KernelError::JoinNotSupported {
                    reason: "a nested loop cannot preserve unmatched right rows; \
                             a right or full outer join needs a hash join"
                        .to_owned(),
                });
            }
            Some(JoinAlgorithm::NestedLoop) => (JoinAlgorithm::NestedLoop, loop_cost),
            // Ties go to the hash join: it reads each side once whatever the
            // estimate turns out to be, where a loop that was estimated at ten
            // outer rows and finds ten thousand costs ten thousand round trips.
            None if loop_possible && loop_cost < hash_cost => {
                (JoinAlgorithm::NestedLoop, loop_cost)
            }
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

    /// Choose how to run a chain of joins.
    ///
    /// Every table is planned through [`SecuredReads::plan`], so every one is
    /// authorised and carries its own row filter. A chain is *n* secured
    /// reads, and the argument that a join cannot see a hidden row is the
    /// same one applied once per step.
    pub(crate) fn plan_chain(
        self,
        context: &SecurityContext,
        tables: &[&TableDef],
        chain: &Chain,
        schema: &JoinSchema,
    ) -> Result<ChainPlan> {
        chain.validate(tables, schema)?;

        let first_table = *chain::table_at(tables, 0)?;
        let first = self.plan(context, first_table, &chain.first)?;
        let mut estimated_rows = first.estimated_rows;
        let mut estimated_cost = first.estimated_cost;
        let mut steps = Vec::with_capacity(chain.steps.len());

        for (index, step) in chain.steps.iter().enumerate() {
            let table = *chain::table_at(tables, index + 1)?;
            let stats = self.statistics.table(table);
            let plan = self.plan(context, table, &step.query)?;

            // The same probe-shape trick as a two-table join: the literal does
            // not matter, only the column's distinct count.
            let own: Vec<JoinKey> = step
                .on
                .iter()
                .map(|key| JoinKey::new(key.left, key.right))
                .collect();
            let mut probe_query = step.query.clone();
            probe_query.filter = core::mem::replace(&mut probe_query.filter, Expr::True)
                .and(join::probe_shape(&own));
            let probe = self.plan(context, table, &probe_query)?;

            // Hash: read the table once, probe with what is already in memory.
            // Loop: one probe per accumulated row.
            let hash_cost = plan.estimated_cost;
            let loop_cost = estimated_rows.max(0.0) * chain::step_probe_floor(probe.estimated_cost);
            // A loop learns nothing about a row of the new table it did not
            // fetch, so it cannot preserve that side. Same limit as before.
            let loop_possible = !step.join_type.preserves(Side::Right);

            let (algorithm, cost) = match step.force {
                Some(forced @ JoinAlgorithm::Hash { .. }) => (forced, hash_cost),
                Some(JoinAlgorithm::NestedLoop) if !loop_possible => {
                    return Err(KernelError::JoinNotSupported {
                        reason: format!(
                            "step {} (`{}`) preserves unmatched rows of that table, \
                             which a nested loop cannot do",
                            index + 1,
                            table.name()
                        ),
                    });
                }
                Some(JoinAlgorithm::NestedLoop) => (JoinAlgorithm::NestedLoop, loop_cost),
                None if loop_possible && loop_cost < hash_cost => {
                    (JoinAlgorithm::NestedLoop, loop_cost)
                }
                // The accumulated side is already in memory, so a hash step
                // builds nothing new: it reads the table and probes it.
                None => (JoinAlgorithm::Hash { build: Side::Right }, hash_cost),
            };

            let distinct =
                join::distinct_over(&stats, &step.on.iter().map(|k| k.right).collect::<Vec<_>>());
            // An accumulated row matches, on average, as many rows of the new
            // table as that table has per distinct key value.
            let fanout = (plan.estimated_rows / distinct.max(1.0)).max(0.0);
            let mut rows = estimated_rows * fanout;
            if step.join_type.preserves(Side::Left) {
                // An outer step returns at least what it started with.
                rows = rows.max(estimated_rows);
            }
            rows *= join::having_selectivity_at(schema, &step.having, &stats, index + 1);

            estimated_rows = rows;
            estimated_cost += cost;
            steps.push(JoinStepPlan {
                plan,
                algorithm,
                estimated_rows: rows,
                estimated_cost: cost,
            });
        }

        Ok(ChainPlan {
            first,
            steps,
            estimated_rows,
            estimated_cost,
        })
    }

    /// Plan and run a chain of joins.
    pub(crate) async fn chain(
        self,
        context: &SecurityContext,
        tables: &[&'a TableDef],
        chain: &Chain,
    ) -> Result<ChainCursor> {
        let schema = Arc::new(JoinSchema::over(tables.iter().copied()));
        let plan = self.plan_chain(context, tables, chain, &schema)?;
        chain::run(self, context, tables, chain, &plan, schema).await
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
        let cursor = QueryCursor::open(
            self.limits,
            self.snapshot,
            table,
            plan,
            query.limit,
            query.offset,
            query.compute.clone(),
        )
        .await?;
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
