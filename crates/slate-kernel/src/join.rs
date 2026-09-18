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
//! A hash join is two scans and no probes; a nested loop is one scan and a
//! probe per outer row. Which wins depends entirely on what a scanned row costs
//! against what a probe costs, and measuring that against object storage moved
//! the answer a long way.
//!
//! A scan with readahead returns roughly eight thousand rows per request. A
//! point read costs about three. So a probe is worth it only when it saves
//! scanning something like twenty-four thousand rows — which means the loop
//! wins when the *rows fetched* are a vanishingly small fraction of the inner
//! table, not merely a small one.
//!
//! This used to say a loop wins when the outer side is under a hundredth of the
//! inner, which was three orders of magnitude out. Measured: four hundred point
//! reads cost 1,221 requests and 3.73 s, while scanning the whole two-hundred
//! thousand row table cost 25 requests and 0.37 s. Loading one parent and its
//! ten children still picks a loop — but only once the inner table is in the
//! hundreds of millions, not the hundreds of thousands.
//!
//! # Outer joins, and which algorithm can serve them
//!
//! All four join types are here. Three of them constrain nothing, but a right
//! or full outer join has to return inner rows that matched *nothing*, and
//! that is not a fact any single probe can establish — it is the absence of a
//! match over the entire other side.
//!
//! A hash join already holds one side in memory, so it flags the buckets that
//! were probed and hands out the rest once the streaming side runs out. A
//! nested loop cannot: it only ever sees the inner rows some outer row asked
//! for, and reading the rest to find out what it missed would be a hash join
//! with extra steps. So the planner will not choose a loop for a right or full
//! join, and refuses one that is forced.
//!
//! Which side a hash join builds is therefore a pure memory decision again —
//! an unmatched built row is drained at the end rather than needing to have
//! been the streaming side.
//!
//! # A condition spanning both sides
//!
//! The join is keyed on equality, and a filter naming one side belongs in that
//! side's [`Query`], where the planner can turn it into scan bounds. What is
//! left is a condition only a formed pair can answer — `left.a < right.b`.
//!
//! That needs an ordinal space belonging to neither table, which is
//! [`JoinSchema`]: the left table keeps its ordinals and the right table's are
//! shifted past its width. A cross-side condition is then an ordinary
//! [`Expr`] over that space, evaluated by the same evaluator with the same
//! three-valued logic the security filter depends on — rather than a parallel
//! expression type, which would be a second place for those null semantics to
//! be got wrong.
//!
//! It behaves like SQL's `ON`, not `WHERE`. An outer join's preserved row has
//! a missing side that reads as null, so the condition is unknown there and
//! the pair is not admitted — but the row itself still comes back, unmatched.
//! A left row whose every candidate is rejected is unmatched, not gone.
//!
//! # What is not here
//!
//! Only two tables. A third would be a tree of these, and the planner would
//! then have to choose a join order, which is the part of query optimisation
//! that needs a search.

use crate::error::{KernelError, Result};
use crate::expr::{CmpOp, Columns, Expr};
use crate::plan::Plan;
use crate::query::Query;
use crate::scalar::Scalar;
use crate::stats::{POINT_READ_COST, SCAN_ROW_COST, TableStats};
use slate_schema::{ColumnDef, Ordinal, Row, TableDef};
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
    /// Every left row, with nothing on the right where none matched.
    Left,
    /// Every right row, with nothing on the left where none matched.
    Right,
    /// Every row of both sides.
    Full,
}

impl JoinType {
    /// Whether an unmatched row of `side` still has to be returned.
    #[must_use]
    pub const fn preserves(self, side: Side) -> bool {
        match (self, side) {
            (Self::Inner, _) => false,
            (Self::Full, _) => true,
            (Self::Left, Side::Left) | (Self::Right, Side::Right) => true,
            (Self::Left, Side::Right) | (Self::Right, Side::Left) => false,
        }
    }

    /// Whether a row can come back with no left side at all.
    #[must_use]
    pub const fn may_drop_left(self) -> bool {
        self.preserves(Side::Right)
    }
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
/// direction. Use [`Join::limit`] for the result rather than a side's own
/// window, because limiting a side before joining it changes the answer and
/// ordering one does not order the join.
///
/// This used to say a side's `limit`, `offset` and `sort` were *ignored*, and
/// they are not: [`crate::read::SecuredReads::execute`] ends in
/// `with_window`, the same windowing a single-table read gets. Measured, on a
/// join of 8 authors to 20 books returning 18 rows (`tests/paged_join.rs`):
///
/// - `left.limit(3)` gives 8 rows over authors {0,1,2}, and `left.offset(5)`
///   gives 6 rows over authors {5,6,7} — on **all three** algorithms, which
///   agree exactly on which rows come back.
/// - `left.sort_by(desc(id))` is honoured by the nested loop and by
///   hash-build-right, and silently dropped by hash-build-left, whose left
///   rows come back out of buckets.
///
/// So a side's window is honoured and a side's *sort* is algorithm-dependent,
/// which is why the wire refuses all three on an input. [`Join::after`] is
/// built on the first of those: paging gives the left side the window it
/// already honours. See `docs/paging-a-join.md`.
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
    /// A predicate over the joined row, in the space [`JoinSchema`] defines.
    ///
    /// This is where a condition spanning both sides lives: `left.a <
    /// right.b`, or an equality the join is not keyed on. It is evaluated
    /// after the pair is formed, so unlike a side's own filter it cannot
    /// narrow either scan — a filter that mentions one side only belongs in
    /// that side's [`Query`], where the planner can turn it into scan bounds.
    ///
    /// It behaves like SQL's `ON`, not `WHERE`: an outer join's preserved row
    /// has a missing side that reads as null, so a condition touching that
    /// side is unknown and the pair is not admitted — but the preserved row
    /// itself still comes back, unmatched.
    pub having: Expr,
    /// Force an algorithm instead of costing one. For tests and for a caller
    /// who knows something the statistics do not.
    pub force: Option<JoinAlgorithm>,
    /// Extra values computed per joined row, appended after *every* table's
    /// columns.
    ///
    /// The same arrangement [`Query::compute`] uses on a single table, lifted
    /// one level: the `i`th appears at `JoinSchema::computed(i)`, so a group
    /// key or an aggregate addresses it the ordinary way. What is new is that
    /// the expression is evaluated over the *joined* row, so it may read both
    /// sides at once — `hour(trips.pickup_time)` and `left.a - right.b` are
    /// the same kind of thing here.
    ///
    /// It is on the join rather than on each side's [`Query`] because a side's
    /// own computed column has nowhere to go *once the row is flattened*: the
    /// sides are concatenated by declared table width, so a value appended to
    /// the left row lands where the right table's first column belongs. That
    /// is not hypothetical — grouping such a join silently returned the right
    /// table's first column as the group key. `read::no_side_computes` refuses
    /// it on the grouped path for that reason.
    ///
    /// Only the grouped path, because only it flattens. An ungrouped join
    /// hands back each side's row as it stands, computed values included, and
    /// the wire splits them per input — so there a side's `Query::compute` is
    /// a supported feature and stays one.
    pub compute: Vec<Scalar>,
    /// Resume after this left-table primary key. See [`Join::after`].
    pub after: Option<Vec<Value>>,
    /// Whether this join will be resumed from a cursor. See [`Join::paging`].
    pub paging: bool,
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
            having: Expr::True,
            force: None,
            compute: Vec::new(),
            after: None,
            paging: false,
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

    /// Compute these values per joined row. See [`Join::compute`].
    #[must_use]
    pub fn computing<I: IntoIterator<Item = Scalar>>(mut self, scalars: I) -> Self {
        self.compute = scalars.into_iter().collect();
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

    /// Keep every right row, matched or not.
    #[must_use]
    pub const fn right_outer(mut self) -> Self {
        self.join_type = JoinType::Right;
        self
    }

    /// Keep every row of both sides.
    #[must_use]
    pub const fn full_outer(mut self) -> Self {
        self.join_type = JoinType::Full;
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

    /// Resume after the left row with this primary key — keyset pagination.
    ///
    /// **A page of a join is a page of its driving table.** The cursor is the
    /// *left* input's primary key, [`Join::limit`] counts left rows rather
    /// than joined rows, and every joined row those left rows produce comes
    /// back with them.
    ///
    /// # Why that is the cursor
    ///
    /// Every joined row derives from exactly one left row — that is what
    /// left-deep means — so "every left row is read by exactly one page"
    /// gives "every joined row is returned by exactly one page". The first is
    /// the property [`Query::after`] already holds, on the left table's own
    /// key range, and this reuses it rather than restating it: the left input
    /// is given the cursor, the limit and `paging`, and nothing else happens.
    ///
    /// A page is therefore *which left rows were read*, which is a key range,
    /// and not a position in the output — which matters because a join's
    /// output order is algorithm-dependent and a cursor that depended on it
    /// would page differently depending on what the cost model chose. See
    /// `docs/paging-a-join.md` for the measurements, including the one that
    /// says only two of the three algorithms keep the left side's order.
    ///
    /// # What it refuses
    ///
    /// A right or full outer join, because its preserved right rows belong to
    /// no left row and so are in no left page; hash with the left as the
    /// build side, because a built side is consumed into buckets and the page
    /// boundary is the last left row *read*; an `offset` alongside the cursor;
    /// and a page with no `limit`. Everything [`Query::after`] refuses on the
    /// left input — an index access path, a sort the key does not give —
    /// fires from the left's own planning in the kernel's own words.
    ///
    /// # What it costs
    ///
    /// A page's row count is not bounded by `limit`: ten left rows with a
    /// hundred matches each is a thousand-row page. A read is streamed, so
    /// that is slower rather than undeliverable — unlike a predicate write's
    /// `returning`, which is one message and therefore has a cap.
    #[must_use]
    pub fn after(mut self, left_key: impl Into<Vec<Value>>) -> Self {
        self.after = Some(left_key.into());
        self.paging = true;
        self
    }

    /// Declare that this join will be resumed, without resuming one yet.
    ///
    /// The first page has no cursor to carry and must still raise every
    /// refusal the second one would, or the caller learns on page two that
    /// page one was never resumable. [`Query::paging`] exists for the same
    /// reason and this is the same move one level up.
    #[must_use]
    pub const fn paging(mut self) -> Self {
        self.paging = true;
        self
    }

    /// Add a condition over the joined row. See [`Join::having`].
    ///
    /// Build it through a [`JoinSchema`], which is what maps each table's
    /// ordinals into the joined space:
    ///
    /// ```ignore
    /// let at = JoinSchema::of(&orders, &shipments);
    /// let join = Join::equating(order_id, shipment_order_id).having(Expr::compare(
    ///     at.right(shipped_at),
    ///     CmpOp::Gt,
    ///     Value::I64(0),
    /// ));
    /// ```
    #[must_use]
    pub fn having(mut self, predicate: Expr) -> Self {
        self.having = core::mem::replace(&mut self.having, Expr::True).and(predicate);
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
        // A `having` ordinal past the joined width names no column at all, and
        // would silently read as null — that is, as a condition nobody wrote.
        //
        // Computed slots are deliberately not in this space: `having` decides
        // whether a pair is admitted, and the computed values are produced
        // from the admitted pair, so a `having` reading one would be reading a
        // value that does not exist yet.
        let schema = JoinSchema::of(left, right);
        for column in self.having.columns() {
            if schema.resolve(column).is_none() {
                return Err(KernelError::JoinNotSupported {
                    reason: format!(
                        "the join condition names {column:?}, which is outside the \
                         {} columns of `{}` joined to `{}`",
                        schema.width(),
                        left.name(),
                        right.name()
                    ),
                });
            }
        }
        // Comparing columns of different types would compare the types rather
        // than the values, and answer the same way for every row. See
        // [`Expr::CompareColumns`].
        let column_type = |column: Ordinal| {
            let (side, at) = schema.resolve(column)?;
            let table = match side {
                Side::Left => left,
                Side::Right => right,
            };
            table.column(at).map(ColumnDef::value_type)
        };
        if let Some((a, b)) = self.having.column_type_conflict(&column_type) {
            return Err(KernelError::ComparisonTypeMismatch {
                at: format!("{} joined to {}", left.name(), right.name()),
                left: a,
                right: b,
            });
        }
        validate_compute(
            &schema,
            &self.compute,
            &format!("`{}` joined to `{}`", left.name(), right.name()),
        )?;
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

/// A join's or chain's computed values, over an already-flattened row.
///
/// The one place either produces them. Each is evaluated against the row as it
/// stands with the earlier values already appended, so a later expression can
/// read an earlier one — the rule [`Query::compute`] follows on a single table,
/// and the reason a chain of them needs no nesting.
///
/// A missing side reads as null throughout, so a computed value over an outer
/// join's unmatched row is computed from nulls rather than skipped. That is
/// what the rest of the join already does with an absent side and what SQL says
/// it contains; the alternative — a null slot regardless of the expression —
/// would make `coalesce(right.x, 0)` answer differently here than anywhere
/// else.
pub(crate) fn computed_values(flat: &Row, compute: &[Scalar]) -> Vec<Value> {
    if compute.is_empty() {
        return Vec::new();
    }
    // Grown onto a copy of the flat row's values rather than onto a rebuilt
    // `Row` per expression, for the reason `exec::extend` gives: rebuilding
    // clones every value once per computed column, which is quadratic in their
    // number. Only the tail is returned.
    let mut values = flat.values().to_vec();
    let width = values.len();
    values.reserve(compute.len());
    for scalar in compute {
        let so_far: &[Value] = &values;
        values.push(scalar.evaluate(so_far));
    }
    values.split_off(width)
}

/// Refuse a computed value that names an ordinal it cannot read.
///
/// A join's or a chain's `compute` is evaluated against the flattened row with
/// the values before it already appended, so the `i`th may read every table's
/// column and the `i` computed values ahead of it — and nothing else. Naming
/// anything else reads as `Value::Null`, because `Scalar::Column` falls back to
/// null for an ordinal the row does not have, and a group key that is null on
/// every row is one group rather than an error.
///
/// That is exactly the failure `validate_grouping` was written for, one level
/// down, and it went unnoticed for the same reason: an out-of-range ordinal is
/// indistinguishable from a column that happens to be null. `Join::compute`
/// shipped with no validation at all, and a grouping over
/// `Scalar::Column(Ordinal(15))` on a six-column joined space returned
/// `[Group { key: [Null], values: [U64(24)] }]` — a plausible-looking answer to
/// a question nobody asked.
///
/// Two ordinals are refused here and they are worth telling apart in the
/// message, because the fixes are different: one past everything is a typo, and
/// one naming a *later* computed value is an ordering mistake the caller can
/// fix by reordering the list.
pub(crate) fn validate_compute(schema: &JoinSchema, compute: &[Scalar], at: &str) -> Result<()> {
    let columns = schema.columns();
    for (index, scalar) in compute.iter().enumerate() {
        let available = columns + index;
        for ordinal in scalar.columns() {
            if ordinal.0 < available {
                continue;
            }
            let reason = if ordinal.0 < columns + compute.len() {
                format!(
                    "computed value {index} of {at} names {ordinal:?}, which is computed \
                     value {} — itself or one produced after it. A computed value may read \
                     the ones before it, so move it later in the list",
                    ordinal.0 - columns
                )
            } else {
                format!(
                    "computed value {index} of {at} names {ordinal:?}, which is outside the \
                     {columns} columns of that joined row and its {index} earlier computed \
                     value(s) — it would read as null on every row, which is one group \
                     rather than an error"
                )
            };
            return Err(KernelError::JoinNotSupported { reason });
        }
    }
    Ok(())
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

/// A cross-side condition's expected selectivity.
///
/// Each conjunct is costed against the statistics of the side it belongs to,
/// after being mapped back into that table's own ordinals. A conjunct touching
/// both sides — the case the whole feature exists for — has no single table to
/// ask, so it gets a fixed guess. That is the standard place for an optimiser
/// to be wrong, and here it costs time rather than correctness: the condition
/// is still evaluated on every pair.
pub(crate) const CROSS_SIDE_SELECTIVITY: f64 = 0.3;

/// A chain step's condition, costed against the new table's statistics.
///
/// A conjunct naming only the table being added is costed properly; anything
/// touching an earlier table has no single set of statistics to ask, so it
/// gets the same fixed guess a two-table cross-side conjunct gets.
pub(crate) fn having_selectivity_at(
    schema: &JoinSchema,
    having: &Expr,
    stats: &TableStats,
    position: usize,
) -> f64 {
    let mut selectivity = 1.0;
    for conjunct in having.conjuncts() {
        let columns = conjunct.columns();
        let only_here = !columns.is_empty()
            && columns
                .iter()
                .all(|c| matches!(schema.locate(*c), Some((at, _)) if at == position));
        selectivity *= if only_here {
            let moved =
                conjunct.map_columns(&|c| schema.locate(c).map_or(c, |(_, ordinal)| ordinal));
            stats.predicate_selectivity(&moved)
        } else {
            CROSS_SIDE_SELECTIVITY
        };
    }
    selectivity.clamp(0.0, 1.0)
}

pub(crate) fn having_selectivity(
    schema: &JoinSchema,
    having: &Expr,
    left: &TableStats,
    right: &TableStats,
) -> f64 {
    let mut selectivity = 1.0;
    for conjunct in having.conjuncts() {
        let columns = conjunct.columns();
        let touches_left = columns
            .iter()
            .any(|c| matches!(schema.resolve(*c), Some((Side::Left, _))));
        let touches_right = columns
            .iter()
            .any(|c| matches!(schema.resolve(*c), Some((Side::Right, _))));
        selectivity *= match (touches_left, touches_right) {
            (true, false) => left.predicate_selectivity(conjunct),
            (false, true) => {
                let moved =
                    conjunct.map_columns(&|c| schema.resolve(c).map_or(c, |(_, ordinal)| ordinal));
                right.predicate_selectivity(&moved)
            }
            _ => CROSS_SIDE_SELECTIVITY,
        };
    }
    selectivity.clamp(0.0, 1.0)
}

/// The cost of a hash join over two side plans.
pub(crate) fn hash_cost(left: &Plan, right: &Plan) -> f64 {
    // Both sides are read exactly once. Hashing and probing are CPU, which
    // against a round trip is noise; the row costs are already in each plan.
    left.estimated_cost + right.estimated_cost
}

/// The cost of a nested-loop join: the outer scan, plus a probe per outer row.
///
/// One probe per outer row, at whatever that probe's own plan costs — the
/// probe's cost already counts its requests, so this multiplies a count by a
/// cost and must not scale either.
///
/// It used to route the row count through [`pipelined_read_cost`], which was
/// wrong twice over. Dividing by [`PROBE_CONCURRENCY`] treated overlapping as
/// though it did less work rather than less waiting; and once that helper was
/// corrected to charge `POINT_READ_COST` per read, sharing it here silently
/// multiplied every probe's whole plan cost by three. Probes *are* overlapped,
/// and that is worth real wall-clock time — but wall clock is not the unit
/// here, and a model that mixes the two is how the constants came to be wrong
/// by three orders of magnitude in the first place.
pub(crate) fn nested_loop_cost(left: &Plan, probe: &Plan) -> f64 {
    let probes = left.estimated_rows.max(0.0);
    left.estimated_cost + probes * probe.estimated_cost
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

/// The ordinal space of a joined row.
///
/// A join's sides are separate tables with separate ordinals, so `left.a` and
/// `right.a` are both `Ordinal(0)` and a predicate naming one cannot say
/// which. This gives them one space: each table's columns are shifted past the
/// widths of the tables before it.
///
/// It is a shift rather than a `(side, ordinal)` pair so that a cross-side
/// predicate is an ordinary [`Expr`] — the same type, the same evaluator, the
/// same three-valued logic the security filter depends on. A parallel
/// expression type for joins would be a second place for those null semantics
/// to be got wrong.
///
/// Two tables is the common case and has [`JoinSchema::left`] and
/// [`JoinSchema::right`] for it; a chain of more uses
/// [`JoinSchema::at`](JoinSchema::at) with a table's position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinSchema {
    /// Column count of each table, in order.
    widths: Vec<usize>,
    /// Where each table's columns start in the joined space.
    offsets: Vec<usize>,
    /// Values the join computes, which sit after every table's columns. See
    /// [`Join::compute`].
    computed: usize,
}

impl JoinSchema {
    /// The space for a chain of tables, in the order they are joined.
    #[must_use]
    pub fn over<'t, I: IntoIterator<Item = &'t TableDef>>(tables: I) -> Self {
        let widths: Vec<usize> = tables.into_iter().map(|t| t.columns().len()).collect();
        let mut offsets = Vec::with_capacity(widths.len());
        let mut at = 0;
        for width in &widths {
            offsets.push(at);
            at += width;
        }
        Self {
            widths,
            offsets,
            computed: 0,
        }
    }

    /// The same space with room for `n` computed values after every table.
    #[must_use]
    pub fn computing(mut self, n: usize) -> Self {
        self.computed = n;
        self
    }

    /// Where the `i`th computed value sits. See [`Join::compute`].
    ///
    /// Past every table's columns, which is the one place an ordinal can go
    /// without shifting a column that already has one — the same reason
    /// [`Query::computed`] appends rather than inserts.
    #[must_use]
    pub fn computed(&self, index: usize) -> Ordinal {
        Ordinal(self.columns() + index)
    }

    /// Columns the tables contribute, not counting computed values.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.widths.iter().sum()
    }

    /// The space for a join of two tables.
    #[must_use]
    pub fn of(left: &TableDef, right: &TableDef) -> Self {
        Self::over([left, right])
    }

    /// A column of the table at `position` in the chain, in the joined space.
    ///
    /// A position past the end returns the ordinal unchanged, which is then
    /// out of range and refused by validation rather than silently reading as
    /// some other table's column.
    #[must_use]
    pub fn at(&self, position: usize, column: Ordinal) -> Ordinal {
        Ordinal(
            self.offsets
                .get(position)
                .map_or(column.0, |o| o + column.0),
        )
    }

    /// A left-table column, in the joined space. Unchanged, by construction.
    #[must_use]
    pub fn left(&self, column: Ordinal) -> Ordinal {
        self.at(0, column)
    }

    /// A right-table column, in the joined space.
    #[must_use]
    pub fn right(&self, column: Ordinal) -> Ordinal {
        self.at(1, column)
    }

    /// A column of `side`, in the joined space.
    #[must_use]
    pub fn column(&self, side: Side, column: Ordinal) -> Ordinal {
        match side {
            Side::Left => self.left(column),
            Side::Right => self.right(column),
        }
    }

    /// Which table a joined ordinal belongs to, and its ordinal there.
    #[must_use]
    pub fn locate(&self, column: Ordinal) -> Option<(usize, Ordinal)> {
        for (position, (offset, width)) in self.offsets.iter().zip(&self.widths).enumerate() {
            if column.0 >= *offset && column.0 < offset + width {
                return Some((position, Ordinal(column.0 - offset)));
            }
        }
        None
    }

    /// Which side a joined ordinal belongs to, for a two-table join.
    ///
    /// `None` for an ordinal outside the space, and for one belonging to a
    /// third table — a caller asking about sides has a two-table join in mind.
    #[must_use]
    pub fn resolve(&self, column: Ordinal) -> Option<(Side, Ordinal)> {
        match self.locate(column) {
            Some((0, at)) => Some((Side::Left, at)),
            Some((1, at)) => Some((Side::Right, at)),
            _ => None,
        }
    }

    /// Total ordinals in the joined space: every table's columns, then the
    /// values the join computes.
    #[must_use]
    pub fn width(&self) -> usize {
        self.columns() + self.computed
    }

    /// How many tables this space spans.
    #[must_use]
    pub fn len(&self) -> usize {
        self.widths.len()
    }

    /// Whether this space spans no tables.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.widths.is_empty()
    }

    /// Columns the table at `position` contributes, or zero past the end.
    #[must_use]
    pub fn width_at(&self, position: usize) -> usize {
        self.widths.get(position).copied().unwrap_or(0)
    }

    /// How many tables the space covers.
    #[must_use]
    pub fn tables(&self) -> usize {
        self.widths.len()
    }

    /// The columns of `predicate` that belong to the table at `position`, back
    /// in that table's own ordinals.
    #[must_use]
    pub fn columns_at(&self, predicate: &Expr, position: usize) -> Vec<Ordinal> {
        predicate
            .columns()
            .into_iter()
            .filter_map(|c| match self.locate(c) {
                Some((found, ordinal)) if found == position => Some(ordinal),
                _ => None,
            })
            .collect()
    }
}

/// A [`JoinedRow`] seen through a [`JoinSchema`], for evaluating a predicate
/// that spans both sides.
///
/// A side that is absent — an outer join preserved a row that matched nothing
/// — reads as null throughout, which is what SQL says an outer join's missing
/// side contains. So `left.a < right.b` is unknown for a preserved row, and
/// unknown does not admit: an outer join filtered on the other side's columns
/// keeps only rows that matched, which is the familiar reason such a filter
/// belongs in `ON` rather than `WHERE`.
struct JoinedView<'r> {
    schema: &'r JoinSchema,
    left: Option<&'r Row>,
    right: Option<&'r Row>,
}

impl Columns for JoinedView<'_> {
    fn value(&self, ordinal: Ordinal) -> Option<&Value> {
        match self.schema.resolve(ordinal) {
            Some((Side::Left, at)) => self.left.and_then(|row| row.get(at)),
            Some((Side::Right, at)) => self.right.and_then(|row| row.get(at)),
            None => None,
        }
    }
}

/// One row of a join: a row from each side, either absent when an outer join
/// preserved a row that matched nothing.
///
/// Both sides are optional because a full outer join needs both to be: it
/// returns left rows with no right and right rows with no left. An inner or
/// left join never produces an absent left, but the type does not try to say
/// so — a shape that changes with the join type would be worse than an
/// `Option` a caller can see is always `Some`.
///
/// The two sides are kept apart rather than concatenated into one wide row, so
/// each decodes into its own type and neither has to know the other's width.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JoinedRow {
    /// The left row, if there was one.
    pub left: Option<Row>,
    /// The right row, if there was one.
    pub right: Option<Row>,
    /// What [`Join::compute`] produced for this row, in order. Empty when the
    /// join computes nothing.
    ///
    /// These belong to the *join*, not to either side, which is why they are a
    /// third field rather than appended to one of the rows: a value spanning
    /// both sides is not the left row's and not the right row's, and putting it
    /// on either would make its ordinal depend on which one.
    ///
    /// They are filled by [`JoinCursor::next`], the single funnel every joined
    /// row leaves through, so the grouped and ungrouped paths cannot drift.
    /// Before that they were produced only where the grouped path flattened —
    /// so an ungrouped join with `Join::compute` set returned each side at its
    /// declared width and no computed value at all, silently. There is a test
    /// for that now.
    pub computed: Vec<Value>,
}

impl JoinedRow {
    /// A matched pair.
    #[must_use]
    pub const fn matched(left: Row, right: Row) -> Self {
        Self {
            left: Some(left),
            right: Some(right),
            computed: Vec::new(),
        }
    }

    /// A left row that matched nothing.
    #[must_use]
    pub const fn left_only(left: Row) -> Self {
        Self {
            left: Some(left),
            right: None,
            computed: Vec::new(),
        }
    }

    /// A right row that matched nothing.
    #[must_use]
    pub const fn right_only(right: Row) -> Self {
        Self {
            left: None,
            right: Some(right),
            computed: Vec::new(),
        }
    }

    /// A pair, in the order the caller asked for rather than the order the
    /// executor happened to have them in.
    #[must_use]
    pub fn pair(streamed: Row, stored: Row, streamed_side: Side) -> Self {
        match streamed_side {
            Side::Left => Self::matched(streamed, stored),
            Side::Right => Self::matched(stored, streamed),
        }
    }

    /// One unmatched row, on the side it came from.
    #[must_use]
    pub const fn only(row: Row, side: Side) -> Self {
        match side {
            Side::Left => Self::left_only(row),
            Side::Right => Self::right_only(row),
        }
    }

    /// The pair as one row, in the space [`JoinSchema`] defines.
    ///
    /// A missing side — an outer join's unmatched row — becomes nulls of that
    /// side's width, which is exactly what [`JoinedView`] already reports for
    /// it, so a predicate over the flattened row and the same predicate over
    /// the pair answer identically. That matters: `Join::having` reads the
    /// view and a grouped join reads this, and two notions of "the joined row"
    /// would be two answers.
    ///
    /// Kept off the join's own path, which passes the two sides through
    /// untouched so each decodes into its own type and neither has to know the
    /// other's width. Grouping is the caller that genuinely needs one row,
    /// because a group key spans both sides.
    #[must_use]
    pub fn flatten(&self, schema: &JoinSchema) -> Row {
        let mut values = Vec::with_capacity(schema.width());
        for (position, side) in [&self.left, &self.right].into_iter().enumerate() {
            let width = schema.width_at(position);
            match side {
                Some(row) => {
                    values.extend(row.values().iter().take(width).cloned());
                    // A side read under a projection is as wide as its table;
                    // this guards the case where it is not, rather than
                    // producing a row that silently shifts every later ordinal.
                    values.resize(schema.at(position, Ordinal(width)).0, Value::Null);
                }
                None => values.resize(values.len() + width, Value::Null),
            }
        }
        Row::new(values)
    }

    /// Flattened, with [`Self::computed`] appended in order.
    ///
    /// The shape the grouper consumes: one row in the [`JoinSchema`] space,
    /// the join's computed values in the slots
    /// [`JoinSchema::computed`](JoinSchema::computed) names.
    ///
    /// It appends rather than evaluates, because the values were produced once
    /// already by [`JoinCursor::next`]. Evaluating here too would be a second
    /// place for the null rules to live, and the two would drift the first time
    /// one of them learned something.
    ///
    /// The cursor flattened this row once already, to evaluate the values
    /// against, so a grouped join with a computed column flattens each row
    /// twice. That was written up as "a real regression" and it is not one:
    /// keeping the cursor's row on [`JoinedRow`] and reusing it here measured
    /// *no* difference — a grouped join with one computed column over 60,000
    /// paired rows ran at a median 320.9 and 342.2 ms rebuilding the row
    /// across two runs, and 325.3 and 310.3 ms reusing it, against a
    /// within-variant spread of 290 to 363 ms. Both paths clone every value
    /// once; only the bookkeeping differs, and the bookkeeping is not where the
    /// time goes. The hypothesis is withdrawn and the field it would have
    /// needed is not here.
    #[must_use]
    pub fn flatten_appending(&self, schema: &JoinSchema) -> Row {
        let flat = self.flatten(schema);
        if self.computed.is_empty() {
            return flat;
        }
        let mut values = flat.into_values();
        values.extend(self.computed.iter().cloned());
        Row::new(values)
    }

    /// Whether both sides are present.
    #[must_use]
    pub const fn is_matched(&self) -> bool {
        self.left.is_some() && self.right.is_some()
    }

    /// One side, if present.
    #[must_use]
    pub const fn side(&self, side: Side) -> Option<&Row> {
        match side {
            Side::Left => self.left.as_ref(),
            Side::Right => self.right.as_ref(),
        }
    }
}

impl Side {
    /// The other side.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
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

/// The built side of a hash join: the rows read into memory, keyed by their
/// join values, each flagged once something probes it.
///
/// The flag is what makes a right or full outer join possible. A probe miss
/// tells you a *streamed* row matched nothing straight away; a build row that
/// matched nothing is only knowable once the probe side is exhausted, so it
/// has to be remembered as you go and drained at the end.
struct BuildTable {
    rows: HashMap<Vec<u8>, Vec<Row>>,
    /// Which rows of a probed bucket actually paired, one flag per row of the
    /// bucket, by bucket key. Held apart from the rows so a bucket can be
    /// handed out by reference while this is written.
    ///
    /// Per *row* and not per bucket, which it used to be on the reasoning that
    /// "a probe that matches a bucket matches every row in it, because they all
    /// carry the same join values". That is true of the join keys and false of
    /// `having`, which can admit the pair with one row of a bucket and reject
    /// it with the next — so a bucket flagged wholesale swallowed the rows that
    /// were rejected, and a right or full outer join lost them. Nothing without
    /// a cross-side condition can tell the two apart, which is why the join
    /// oracle now generates one.
    hit: HashMap<Vec<u8>, Vec<bool>>,
    /// Rows dropped for having a null join value. They match nothing by
    /// definition, but an outer join that preserves this side still owes them
    /// to the caller.
    nulls: Vec<Row>,
}

impl BuildTable {
    /// Read `cursor` fully, bucketing by `columns`.
    ///
    /// A row with a null join value goes to `nulls` rather than into a bucket
    /// under a key containing one: it can never match, and a key holding a
    /// null is a bug waiting to be written.
    ///
    /// `keep_unmatched` says whether those rows are owed to the caller at all.
    /// An inner or one-sided join throws them away here rather than carrying
    /// them through the whole probe.
    async fn build(
        cursor: &mut QueryCursor<'_>,
        columns: &[Ordinal],
        limit: usize,
        table: &TableDef,
        keep_unmatched: bool,
    ) -> Result<Self> {
        let mut rows: HashMap<Vec<u8>, Vec<Row>> = HashMap::new();
        let mut nulls = Vec::new();
        let mut held = 0usize;
        while let Some(row) = cursor.next().await? {
            let key = join_values(&row, columns);
            held += 1;
            if held > limit {
                return Err(KernelError::JoinBuildTooLarge {
                    table: table.name().to_owned(),
                    limit,
                });
            }
            match key {
                Some(key) => rows.entry(key).or_default().push(row),
                None if keep_unmatched => nulls.push(row),
                None => {}
            }
        }
        Ok(Self {
            rows,
            hit: HashMap::new(),
            nulls,
        })
    }

    fn get(&self, key: &[u8]) -> Option<&[Row]> {
        self.rows.get(key).map(Vec::as_slice)
    }

    /// Note that the row at `position` of this bucket paired with something.
    ///
    /// The flags are allocated on the bucket's first hit and sized to it, so an
    /// unprobed bucket costs nothing and a probed one costs a byte per row
    /// rather than a hash per pair. The key is copied on that first hit only.
    fn mark(&mut self, key: &[u8], position: usize) {
        let width = self.rows.get(key).map_or(0, Vec::len);
        let flags = match self.hit.get_mut(key) {
            Some(flags) => flags,
            None => self
                .hit
                .entry(key.to_vec())
                .or_insert_with(|| vec![false; width]),
        };
        if let Some(flag) = flags.get_mut(position) {
            *flag = true;
        }
    }

    /// Every built row nothing paired with, once probing is done.
    ///
    /// Row by row rather than bucket by bucket. Buckets agree on the join keys
    /// and so are all-or-nothing as far as the *equality* goes, but `having` is
    /// evaluated per pair and can split one — see [`BuildTable::hit`].
    fn unmatched(&mut self) -> Vec<Row> {
        let mut out = core::mem::take(&mut self.nulls);
        for (key, bucket) in &mut self.rows {
            let hit = self.hit.get(key);
            for (position, row) in core::mem::take(bucket).into_iter().enumerate() {
                if !hit.is_some_and(|flags| flags.get(position).copied().unwrap_or(false)) {
                    out.push(row);
                }
            }
        }
        self.rows.clear();
        out
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
        /// The streamed row being matched, its bucket key, how far through the
        /// bucket we are, and whether anything has actually paired with it.
        /// Held by key rather than by a cloned bucket so a row matching a
        /// thousand others does not copy a thousand rows to return the first.
        current: Option<(Row, Option<Vec<u8>>, usize, bool)>,
        /// Built rows nothing matched, handed out after the probe side runs
        /// out. Empty unless the join preserves the built side.
        draining: Option<std::vec::IntoIter<Row>>,
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
    /// The condition over the joined row, and the space it is written in.
    having: Arc<Expr>,
    schema: Arc<JoinSchema>,
    /// [`Join::compute`], evaluated per row this cursor yields. Shared rather
    /// than cloned because `next` runs per row and the expressions do not
    /// change.
    compute: Arc<[Scalar]>,
    limit: Option<usize>,
    offset: usize,
    skipped: usize,
    yielded: usize,
    /// Whether to track the page boundary at all. See [`Join::after`].
    ///
    /// A guard rather than an `Option` on the two fields below because it is
    /// read once per *left* row on every join, paged or not: tracking costs a
    /// row clone, and an unpaged join should not pay it.
    paging: bool,
    /// The last row read from the left scan, which is the page's boundary.
    ///
    /// The last row *read*, not the last emitted: under an inner join a left
    /// row that matched nothing is consumed and emits nothing, and a page
    /// whose left rows all matched nothing would otherwise return no rows and
    /// no advanced cursor — which is a caller looping forever.
    page_end: Option<Row>,
    /// How many rows the left scan yielded, to tell a full page from a short
    /// one. The left's own `limit` is the page size, so `== limit` means full.
    left_rows: usize,
}

/// Whether a formed pair survives the cross-side condition.
///
/// Split out so both algorithms apply it identically. `Expr::True` short-
/// circuits, so a join with no such condition pays a discriminant check per
/// pair and nothing else.
fn admits_pair(having: &Expr, schema: &JoinSchema, left: &Row, right: &Row) -> bool {
    if matches!(having, Expr::True) {
        return true;
    }
    having.admits_over(&JoinedView {
        schema,
        left: Some(left),
        right: Some(right),
    })
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
                let built = BuildTable::build(
                    &mut building,
                    &build_columns,
                    join.build_limit,
                    build_table,
                    join.join_type.preserves(build),
                )
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
                    draining: None,
                }
            }
        };
        Ok(Self {
            state,
            join_type: join.join_type,
            having: Arc::new(join.having.clone()),
            schema: Arc::new(JoinSchema::of(left_table, right_table)),
            compute: join.compute.clone().into(),
            limit: join.limit,
            offset: join.offset,
            skipped: 0,
            yielded: 0,
            paging: join.paging,
            page_end: None,
            left_rows: 0,
        })
    }

    /// The primary key of the last left row this page read, if any.
    ///
    /// Meaningful once the cursor is exhausted: the nested loop keeps probes
    /// in flight, so before then this is the last left row *pulled*, which
    /// runs ahead of the last one emitted. Both converge when the left scan
    /// ends, which its own `limit` guarantees happens.
    #[must_use]
    pub fn page_end(&self, left: &TableDef) -> Option<Vec<Value>> {
        self.page_end
            .as_ref()
            .map(|row| row.primary_key_values(left).to_vec())
    }

    /// How many rows the left scan yielded.
    ///
    /// The page size is the left's limit, so a caller tells a full page from a
    /// short one by comparing against it — and a short page proves there is
    /// nothing after it, so it gets no cursor.
    #[must_use]
    pub const fn driving_rows(&self) -> usize {
        self.left_rows
    }

    /// The next joined row.
    pub async fn next(&mut self) -> Result<Option<JoinedRow>> {
        if self.limit.is_some_and(|l| self.yielded >= l) {
            return Ok(None);
        }
        while let Some(mut row) = self.next_joined().await? {
            if self.skipped < self.offset {
                self.skipped += 1;
                continue;
            }
            // After the window, not before: a computed value for a row the
            // offset discards is work nobody asked for, and `compute` may hold
            // a regex replace.
            if !self.compute.is_empty() {
                row.computed = computed_values(&row.flatten(&self.schema), &self.compute);
            }
            self.yielded += 1;
            return Ok(Some(row));
        }
        Ok(None)
    }

    async fn next_joined(&mut self) -> Result<Option<JoinedRow>> {
        let join_type = self.join_type;
        let having = Arc::clone(&self.having);
        let schema = Arc::clone(&self.schema);
        let paging = self.paging;
        let page_end = &mut self.page_end;
        let left_rows = &mut self.left_rows;
        match &mut self.state {
            State::Hash {
                probe,
                built,
                build_side,
                probe_columns,
                current,
                draining,
            } => loop {
                let probe_side = build_side.other();

                // Built rows nothing matched, once the probe side is done.
                if let Some(rows) = draining {
                    return Ok(rows.next().map(|row| JoinedRow::only(row, *build_side)));
                }

                // Finish emitting the bucket the last streamed row matched.
                if let Some((row, key, at, paired)) = current {
                    let bucket = key.as_deref().and_then(|k| built.get(k));
                    let mut emit = None;
                    // Where in the bucket the emitted pair came from, so the
                    // built row that paired is the one flagged — not every row
                    // that happened to share its join values.
                    let mut emitted_at = None;
                    while let Some(next) = bucket.and_then(|bucket| bucket.get(*at)) {
                        *at += 1;
                        // Which side was built is a costing decision; which
                        // side is `left` is the caller's, so the pair goes back
                        // the way they asked for it before the condition —
                        // written in the caller's terms — is applied.
                        let candidate = JoinedRow::pair(row.clone(), next.clone(), probe_side);
                        let (Some(l), Some(r)) = (&candidate.left, &candidate.right) else {
                            continue;
                        };
                        if admits_pair(&having, &schema, l, r) {
                            emit = Some(candidate);
                            emitted_at = Some(*at - 1);
                            break;
                        }
                    }
                    if let Some(pair) = emit {
                        *paired = true;
                        if let (Some(key), Some(position)) = (key.as_deref(), emitted_at) {
                            built.mark(key, position);
                        }
                        return Ok(Some(pair));
                    }

                    // The bucket is spent. A row that found candidates but had
                    // them all rejected by the condition counts as unmatched,
                    // which is what makes `having` behave like `ON` rather
                    // than a filter over the finished join.
                    let missed = !*paired;
                    let row = row.clone();
                    *current = None;
                    if missed && join_type.preserves(probe_side) {
                        return Ok(Some(JoinedRow::only(row, probe_side)));
                    }
                    continue;
                }

                match probe.next().await? {
                    Some(row) => {
                        // The probe is the left side exactly when the right
                        // was built — which is the only hash shape a paged
                        // join is allowed to run, so this is the boundary.
                        if paging && *build_side == Side::Right {
                            *left_rows += 1;
                            *page_end = Some(row.clone());
                        }
                        let key = join_values(&row, probe_columns);
                        *current = Some((row, key, 0, false));
                    }
                    None => {
                        // A built row that matched nothing is only knowable
                        // now: unlike a probe miss, it is the absence of an
                        // event over the whole probe side.
                        if join_type.preserves(*build_side) {
                            *draining = Some(built.unmatched().into_iter());
                            continue;
                        }
                        return Ok(None);
                    }
                }
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
                        Some(row) => {
                            // The outer side of a loop is always the left.
                            if paging {
                                *left_rows += 1;
                                *page_end = Some(row.clone());
                            }
                            inflight.push_back(probe.matches(row));
                        }
                        None => *exhausted = true,
                    }
                }
                match inflight.next().await {
                    Some(Err(error)) => return Err(error),
                    Some(Ok((left, matches))) => {
                        let matches: Vec<Row> = matches
                            .into_iter()
                            .filter(|right| admits_pair(&having, &schema, &left, right))
                            .collect();
                        if matches.is_empty() {
                            if join_type.preserves(Side::Left) {
                                return Ok(Some(JoinedRow::left_only(left)));
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
