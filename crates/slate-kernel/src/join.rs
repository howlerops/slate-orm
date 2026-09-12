//! Joining two tables.
//!
//! # A join is two secured reads, not one privileged one
//!
//! Each side of a join is planned and executed through the same secured read
//! path a single-table query uses ([`crate::read::SecuredReads`]): each is
//! authorised against the caller's roles, and each carries its own row-level
//! filter conjoined *before* planning. Nothing here reads a row directly.
//!
//! That is the whole security argument, and it is structural rather than
//! careful: a join cannot see a row either side's policy hides, because it
//! never asks storage for rows — it asks two cursors that have already
//! applied their policies. A hidden row on the inner side makes an outer row
//! *unmatched*, exactly as a genuinely absent row would, so a join is not an
//! existence oracle either.
//!
//! # Choosing an algorithm
//!
//! Under this cost model a scanned row costs a hundredth of a round trip and a
//! probe costs at least one, so a hash join — two scans, no probes — is the
//! general answer. A nested loop only wins when the outer side is smaller than
//! roughly a hundredth of the inner one.
//!
//! That is not a marginal case: it is what an ORM does all day. Load one user,
//! then their orders. One outer row against ten thousand inner ones is
//! precisely where the loop is right, and the planner picks it there.
//!
//! # What is not here
//!
//! The join condition is equality between columns, and any other filter belongs
//! to one side or the other. A predicate spanning both sides — `left.a <
//! right.b` — has nowhere to live yet, because a row of a join has no single
//! ordinal space. Right and full outer joins are also absent; a right join is a
//! left join with the sides swapped, and writing that out is the caller's job
//! for now.

use crate::error::{KernelError, Result};
use crate::expr::{CmpOp, Expr};
use crate::plan::Plan;
use crate::query::Query;
use crate::stats::{POINT_READ_COST, SCAN_ROW_COST, TableStats};
use slate_schema::{Ordinal, Row, TableDef};
use slate_tuple::{Direction, Value, encode_value_into};

/// How many inner probes a nested-loop join keeps in flight.
///
/// A probe is a round trip. Issuing them one at a time makes the join's
/// latency the sum of its probes, which is the same mistake a serial index
/// scan makes; see [`crate::exec::DEFAULT_PREFETCH`], whose reasoning and
/// trade-off this shares.
pub const PROBE_CONCURRENCY: usize = 16;

/// Rows a hash join will hold in memory before refusing to continue.
///
/// A hash join materialises one side. Left unbounded that is a way to turn a
/// mistyped join key into an out-of-memory kill, so there is a limit and it
/// reports rather than dies. Raise it with [`Join::build_limit`] when the
/// memory is genuinely there.
pub const DEFAULT_BUILD_LIMIT: usize = 100_000;

/// Which side of a join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The left table.
    Left,
    /// The right table.
    Right,
}

/// Which rows survive the join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JoinType {
    /// Only rows that matched on both sides.
    #[default]
    Inner,
    /// Every left row, with nulls where the right side had no match.
    Left,
}

/// One equality of the join condition: `left.column = right.column`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinKey {
    /// Column on the left table.
    pub left: Ordinal,
    /// Column on the right table.
    pub right: Ordinal,
}

impl JoinKey {
    /// `left.a = right.b`.
    #[must_use]
    pub const fn new(left: Ordinal, right: Ordinal) -> Self {
        Self { left, right }
    }
}

/// A request to join two tables.
///
/// Each side carries its own [`Query`]: its filter, its projection, its scan
/// direction. A side's `limit`, `offset` and `sort` are ignored — limiting a
/// side before joining it changes the answer, and ordering one does not order
/// the join. Use [`Join::limit`] for the result.
#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    /// What to read from the left table.
    pub left: Query,
    /// What to read from the right table.
    pub right: Query,
    /// The equality columns. Empty is refused: a cross join is not something
    /// anyone asks for by accident, and this would be the accident.
    pub on: Vec<JoinKey>,
    /// Which rows survive.
    pub join_type: JoinType,
    /// Maximum joined rows to return.
    pub limit: Option<usize>,
    /// Joined rows to discard first.
    pub offset: usize,
    /// Rows a hash build side may hold. See [`DEFAULT_BUILD_LIMIT`].
    pub build_limit: usize,
    /// Force an algorithm instead of costing one. For tests and for a caller
    /// who knows something the statistics do not.
    pub force: Option<JoinAlgorithm>,
}

impl Join {
    /// Join on `keys`, reading everything from both sides.
    #[must_use]
    pub fn on<I: IntoIterator<Item = JoinKey>>(keys: I) -> Self {
        Self {
            left: Query::all(),
            right: Query::all(),
            on: keys.into_iter().collect(),
            join_type: JoinType::Inner,
            limit: None,
            offset: 0,
            build_limit: DEFAULT_BUILD_LIMIT,
            force: None,
        }
    }

    /// Join on one pair of columns.
    #[must_use]
    pub fn equating(left: Ordinal, right: Ordinal) -> Self {
        Self::on([JoinKey::new(left, right)])
    }

    /// What to read from the left table.
    #[must_use]
    pub fn left(mut self, query: Query) -> Self {
        self.left = query;
        self
    }

    /// What to read from the right table.
    #[must_use]
    pub fn right(mut self, query: Query) -> Self {
        self.right = query;
        self
    }

    /// Keep every left row, matched or not.
    #[must_use]
    pub const fn left_outer(mut self) -> Self {
        self.join_type = JoinType::Left;
        self
    }

    /// Return at most `limit` joined rows.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Discard the first `offset` joined rows.
    #[must_use]
    pub const fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Rows a hash build side may hold.
    #[must_use]
    pub const fn build_limit(mut self, rows: usize) -> Self {
        self.build_limit = rows;
        self
    }

    /// Run this algorithm rather than the cheapest one.
    #[must_use]
    pub const fn using(mut self, algorithm: JoinAlgorithm) -> Self {
        self.force = Some(algorithm);
        self
    }

    /// Reject a request the executor could not run.
    pub(crate) fn validate(&self, left: &TableDef, right: &TableDef) -> Result<()> {
        if self.on.is_empty() {
            return Err(KernelError::JoinNotSupported {
                reason: "a join needs at least one equality; a cross join is not offered"
                    .to_owned(),
            });
        }
        for key in &self.on {
            if left.column(key.left).is_none() {
                return Err(KernelError::JoinNotSupported {
                    reason: format!("table `{}` has no column {:?}", left.name(), key.left),
                });
            }
            if right.column(key.right).is_none() {
                return Err(KernelError::JoinNotSupported {
                    reason: format!("table `{}` has no column {:?}", right.name(), key.right),
                });
            }
        }
        Ok(())
    }

    /// The join's columns on one side.
    pub(crate) fn columns(&self, side: Side) -> Vec<Ordinal> {
        self.on
            .iter()
            .map(|key| match side {
                Side::Left => key.left,
                Side::Right => key.right,
            })
            .collect()
    }
}

/// How the executor will combine the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinAlgorithm {
    /// Read one side into a hash table, then stream the other past it.
    ///
    /// Two scans and no probes, which under this cost model is almost always
    /// the cheaper shape. Costs memory proportional to the built side.
    Hash {
        /// The side read into memory. The other side streams.
        build: Side,
    },
    /// Stream the left side, and look the right side up per row.
    ///
    /// One probe per outer row, each a round trip, overlapped
    /// [`PROBE_CONCURRENCY`] at a time. Worth it only when the outer side is
    /// small — which, for an ORM loading one record's children, it is.
    NestedLoop,
}

/// A costed plan for a join, and the plans for each of its sides.
#[derive(Debug, Clone)]
pub struct JoinPlan {
    /// How the sides are combined.
    pub algorithm: JoinAlgorithm,
    /// How the left side is read.
    pub left: Plan,
    /// How the right side is read, for a hash join.
    ///
    /// A nested loop re-plans the right side per outer row with the join
    /// equality bound to that row's value, so this is the shape of the probe
    /// rather than a plan that will be run as-is.
    pub right: Plan,
    /// Joined rows the planner expects.
    pub estimated_rows: f64,
    /// Estimated cost in object-storage round trips.
    pub estimated_cost: f64,
}

/// Estimate how many rows joining two sides produces.
///
/// The textbook estimate: matches divide by the larger of the two join
/// columns' distinct counts. It assumes the values are drawn from overlapping
/// domains, which for a foreign key they are, and badly overstates a join
/// between unrelated columns — which is a query nobody means to write.
pub(crate) fn join_cardinality(
    left_rows: f64,
    right_rows: f64,
    left_distinct: f64,
    right_distinct: f64,
) -> f64 {
    let divisor = left_distinct.max(right_distinct).max(1.0);
    (left_rows * right_rows / divisor).max(0.0)
}

/// Distinct values across the join columns of one side, as a count.
pub(crate) fn distinct_over(stats: &TableStats, columns: &[Ordinal]) -> f64 {
    // Independence again: the product of each column's distinct count, capped
    // at the table's size because there cannot be more combinations than rows.
    let product: f64 = columns
        .iter()
        .map(|c| stats.column(*c).distinct.max(1) as f64)
        .product();
    product.min(stats.row_count.max(1) as f64).max(1.0)
}

/// The cost of a hash join over two side plans.
pub(crate) fn hash_cost(left: &Plan, right: &Plan) -> f64 {
    // Both sides are read exactly once. Hashing and probing are CPU, which
    // against a round trip is noise; the row costs are already in each plan.
    left.estimated_cost + right.estimated_cost
}

/// The cost of a nested-loop join: the outer scan, plus a probe per outer row.
pub(crate) fn nested_loop_cost(left: &Plan, probe: &Plan) -> f64 {
    left.estimated_cost + left.estimated_rows.max(0.0) * probe.estimated_cost
}

/// Bind the join equalities to a row's values, for planning or running a probe.
///
/// Returns `None` when any join value is null: SQL equality against null is
/// unknown, never true, so such a row matches nothing and there is no probe
/// worth issuing.
pub(crate) fn probe_filter(keys: &[JoinKey], left_row: &Row) -> Option<Expr> {
    let mut conjuncts = Vec::with_capacity(keys.len());
    for key in keys {
        let value = left_row.get(key.left)?;
        if value.is_null() {
            return None;
        }
        conjuncts.push(Expr::Compare {
            column: key.right,
            op: CmpOp::Eq,
            value: value.clone(),
        });
    }
    Some(Expr::all(conjuncts))
}

/// A placeholder equality on each right-side join column, for costing a probe
/// before any outer row exists.
///
/// The literal is irrelevant: equality selectivity comes from the column's
/// distinct count, not from the value being compared.
pub(crate) fn probe_shape(keys: &[JoinKey]) -> Expr {
    Expr::all(keys.iter().map(|key| Expr::Compare {
        column: key.right,
        op: CmpOp::Eq,
        value: Value::I64(0),
    }))
}

/// Cost per joined row the executor pays regardless of algorithm: building the
/// output. CPU only, so far below a round trip.
pub(crate) const JOIN_ROW_COST: f64 = SCAN_ROW_COST / 10.0;

/// A guard against a probe plan the cost model thinks is free.
///
/// A probe always opens something, so it can never be cheaper than the point
/// read it is standing in for. Without this a nested loop could be costed at
/// zero per row and win everywhere.
pub(crate) fn probe_floor(cost: f64) -> f64 {
    cost.max(POINT_READ_COST)
}

/// One row of a join: a row from each side, the right absent when a left outer
/// join found no match.
///
/// The two sides are kept apart rather than concatenated into one wide row.
/// Concatenating would need an ordinal space belonging to neither table, and
/// every caller would then have to know the left table's width to read a right
/// column. Keeping them separate also lets each side decode into its own type.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinedRow {
    /// The left row. Always present.
    pub left: Row,
    /// The right row, if one matched.
    pub right: Option<Row>,
}

impl JoinedRow {
    /// A matched pair.
    #[must_use]
    pub const fn matched(left: Row, right: Row) -> Self {
        Self {
            left,
            right: Some(right),
        }
    }

    /// A left row with no match.
    #[must_use]
    pub const fn unmatched(left: Row) -> Self {
        Self { left, right: None }
    }

    /// Whether the right side matched.
    #[must_use]
    pub const fn is_matched(&self) -> bool {
        self.right.is_some()
    }
}

/// The join values of a row, encoded, or `None` if any is null.
///
/// Encoded rather than compared as values, for two reasons. It is the equality
/// the storage layer itself uses, so two rows join exactly when they would
/// collide in an index — which is the definition a join ought to have. And
/// [`Value`] is not hashable, because it can hold a float; the encoding
/// settles what `-0.0` and `NaN` mean here by deferring to the codec rather
/// than by inventing an answer.
///
/// `None` when any join value is null: null never equals null, so such a row
/// belongs in no bucket and matches no probe. Returning `None` rather than a
/// key that contains a null is what keeps that from being an easy mistake to
/// make later.
pub(crate) fn join_values(row: &Row, columns: &[Ordinal]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(columns.len() * 8);
    for column in columns {
        let value = row.get(*column)?;
        if value.is_null() {
            return None;
        }
        encode_value_into(&mut out, value, Direction::Asc);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

use crate::exec::QueryCursor;
use crate::read::SecuredReads;
use crate::security::SecurityContext;
use futures::future::BoxFuture;
use futures::stream::{FuturesOrdered, StreamExt as _};
use std::collections::HashMap;
use std::sync::Arc;

/// Everything a nested loop needs to probe the inner side, owned so each probe
/// future can hold it without borrowing the cursor.
struct Probe<'a> {
    reads: SecuredReads<'a>,
    context: Arc<SecurityContext>,
    table: &'a TableDef,
    /// The right side's own query, before the join equality is bound onto it.
    base: Arc<Query>,
    keys: Arc<Vec<JoinKey>>,
}

/// An outer row and the inner rows it matched, still in flight.
type PendingProbe<'a> = BoxFuture<'a, Result<(Row, Vec<Row>)>>;

impl<'a> Probe<'a> {
    /// The inner rows matching `left_row`, as a future that owns what it needs.
    fn matches(&self, left_row: Row) -> PendingProbe<'a> {
        let reads = self.reads;
        let context = Arc::clone(&self.context);
        let table = self.table;
        let base = Arc::clone(&self.base);
        let keys = Arc::clone(&self.keys);
        Box::pin(async move {
            // A null join value equals nothing, so there is no probe to issue
            // and no match to find.
            let Some(bound) = probe_filter(&keys, &left_row) else {
                return Ok((left_row, Vec::new()));
            };
            let mut query = (*base).clone();
            query.filter = core::mem::replace(&mut query.filter, Expr::True).and(bound);
            // The probe is a whole secured read: authorised, policy-filtered,
            // planned. There is no shortcut past it just because a join asked.
            let rows = reads
                .execute(&context, table, &query)
                .await?
                .collect()
                .await?;
            Ok((left_row, rows))
        })
    }
}

/// One probe side of a hash join: the rows read into memory, keyed by their
/// join values.
struct BuildTable {
    rows: HashMap<Vec<u8>, Vec<Row>>,
}

impl BuildTable {
    /// Read `cursor` fully, bucketing by `columns`.
    ///
    /// Rows with a null join value are dropped here rather than stored under a
    /// null key: they can never match, and a key holding a null is a bug
    /// waiting to be written.
    async fn build(
        cursor: &mut QueryCursor<'_>,
        columns: &[Ordinal],
        limit: usize,
        table: &TableDef,
    ) -> Result<Self> {
        let mut rows: HashMap<Vec<u8>, Vec<Row>> = HashMap::new();
        let mut held = 0usize;
        while let Some(row) = cursor.next().await? {
            let Some(key) = join_values(&row, columns) else {
                continue;
            };
            held += 1;
            if held > limit {
                return Err(KernelError::JoinBuildTooLarge {
                    table: table.name().to_owned(),
                    limit,
                });
            }
            rows.entry(key).or_default().push(row);
        }
        Ok(Self { rows })
    }

    fn get(&self, key: &[u8]) -> Option<&[Row]> {
        self.rows.get(key).map(Vec::as_slice)
    }
}

/// What the cursor is in the middle of doing.
enum State<'a> {
    Hash {
        probe: QueryCursor<'a>,
        built: BuildTable,
        /// Which side is in memory. The other is `probe`.
        build_side: Side,
        /// Join columns on the streaming side.
        probe_columns: Vec<Ordinal>,
        /// The streamed row being matched, its bucket key, and how far through
        /// the bucket we are. Held by key rather than by a cloned bucket so a
        /// row matching a thousand others does not copy a thousand rows to
        /// return the first.
        current: Option<(Row, Option<Vec<u8>>, usize)>,
    },
    NestedLoop {
        outer: QueryCursor<'a>,
        probe: Probe<'a>,
        inflight: FuturesOrdered<PendingProbe<'a>>,
        exhausted: bool,
        /// Matches of the outer row currently being emitted.
        current: Option<(Row, std::vec::IntoIter<Row>)>,
    },
}

/// A cursor over the rows a join produces.
///
/// The join does not promise an order. Neither does SQL without an `ORDER BY`,
/// and here the order depends on which side the planner chose to build — which
/// is a costing decision that should be free to change.
pub struct JoinCursor<'a> {
    state: State<'a>,
    join_type: JoinType,
    limit: Option<usize>,
    offset: usize,
    skipped: usize,
    yielded: usize,
}

impl core::fmt::Debug for JoinCursor<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("JoinCursor")
            .field("yielded", &self.yielded)
            .finish_non_exhaustive()
    }
}

impl<'a> JoinCursor<'a> {
    /// Run `plan` for `join` over the two tables.
    pub(crate) async fn open(
        reads: SecuredReads<'a>,
        context: &SecurityContext,
        left_table: &'a TableDef,
        right_table: &'a TableDef,
        join: &Join,
        plan: &JoinPlan,
    ) -> Result<Self> {
        let state = match plan.algorithm {
            JoinAlgorithm::NestedLoop => {
                let outer = reads.execute(context, left_table, &join.left).await?;
                State::NestedLoop {
                    outer,
                    probe: Probe {
                        reads,
                        context: Arc::new(context.clone()),
                        table: right_table,
                        base: Arc::new(join.right.clone()),
                        keys: Arc::new(join.on.clone()),
                    },
                    inflight: FuturesOrdered::new(),
                    exhausted: false,
                    current: None,
                }
            }
            JoinAlgorithm::Hash { build } => {
                let (build_table, build_query, build_columns) = match build {
                    Side::Left => (left_table, &join.left, join.columns(Side::Left)),
                    Side::Right => (right_table, &join.right, join.columns(Side::Right)),
                };
                let mut building = reads.execute(context, build_table, build_query).await?;
                let built =
                    BuildTable::build(&mut building, &build_columns, join.build_limit, build_table)
                        .await?;
                let (probe_table, probe_query, probe_columns) = match build {
                    Side::Left => (right_table, &join.right, join.columns(Side::Right)),
                    Side::Right => (left_table, &join.left, join.columns(Side::Left)),
                };
                State::Hash {
                    probe: reads.execute(context, probe_table, probe_query).await?,
                    built,
                    build_side: build,
                    probe_columns,
                    current: None,
                }
            }
        };
        Ok(Self {
            state,
            join_type: join.join_type,
            limit: join.limit,
            offset: join.offset,
            skipped: 0,
            yielded: 0,
        })
    }

    /// The next joined row.
    pub async fn next(&mut self) -> Result<Option<JoinedRow>> {
        if self.limit.is_some_and(|l| self.yielded >= l) {
            return Ok(None);
        }
        while let Some(row) = self.next_joined().await? {
            if self.skipped < self.offset {
                self.skipped += 1;
                continue;
            }
            self.yielded += 1;
            return Ok(Some(row));
        }
        Ok(None)
    }

    async fn next_joined(&mut self) -> Result<Option<JoinedRow>> {
        let join_type = self.join_type;
        match &mut self.state {
            State::Hash {
                probe,
                built,
                build_side,
                probe_columns,
                current,
            } => loop {
                // Finish emitting the bucket the last streamed row matched.
                if let Some((row, key, at)) = current {
                    let matched = key.as_deref().and_then(|k| built.get(k));
                    if let Some(next) = matched.and_then(|bucket| bucket.get(*at)) {
                        *at += 1;
                        let next = next.clone();
                        return Ok(Some(match build_side {
                            // The streamed row is the left one when the right
                            // side was built, and the other way round when it
                            // was not. Which side was built is a costing
                            // decision; which side is `left` is the caller's.
                            Side::Right => JoinedRow::matched(row.clone(), next),
                            Side::Left => JoinedRow::matched(next, row.clone()),
                        }));
                    }
                    let unmatched = *at == 0;
                    let row = row.clone();
                    *current = None;
                    // Only a left outer join emits a row that found nothing,
                    // and only when the left side is the one streaming.
                    if unmatched && join_type == JoinType::Left && *build_side == Side::Right {
                        return Ok(Some(JoinedRow::unmatched(row)));
                    }
                    continue;
                }
                let Some(row) = probe.next().await? else {
                    return Ok(None);
                };
                let key = join_values(&row, probe_columns);
                *current = Some((row, key, 0));
            },
            State::NestedLoop {
                outer,
                probe,
                inflight,
                exhausted,
                current,
            } => loop {
                if let Some((left, matches)) = current {
                    if let Some(right) = matches.next() {
                        return Ok(Some(JoinedRow::matched(left.clone(), right)));
                    }
                    *current = None;
                    continue;
                }
                // Keep probes in flight. Each is a round trip; issued one at a
                // time the join's latency would be their sum.
                while !*exhausted && inflight.len() < PROBE_CONCURRENCY {
                    match outer.next().await? {
                        Some(row) => inflight.push_back(probe.matches(row)),
                        None => *exhausted = true,
                    }
                }
                match inflight.next().await {
                    Some(Err(error)) => return Err(error),
                    Some(Ok((left, matches))) => {
                        if matches.is_empty() {
                            if join_type == JoinType::Left {
                                return Ok(Some(JoinedRow::unmatched(left)));
                            }
                            continue;
                        }
                        *current = Some((left, matches.into_iter()));
                    }
                    None => return Ok(None),
                }
            },
        }
    }

    /// Drain the cursor into a vector.
    pub async fn collect(mut self) -> Result<Vec<JoinedRow>> {
        let mut out = Vec::new();
        while let Some(row) = self.next().await? {
            out.push(row);
        }
        Ok(out)
    }

    /// Count the joined rows.
    pub async fn count(mut self) -> Result<usize> {
        let mut n = 0;
        while self.next().await?.is_some() {
            n += 1;
        }
        Ok(n)
    }
}
