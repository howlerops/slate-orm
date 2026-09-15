//! Choosing how to read a table, and turning predicates into scan bounds.
//!
//! The planner is heuristic on purpose. A cost model needs statistics, and
//! statistics need a whole subsystem; matching a predicate's equality terms
//! against an index's leading columns gets most of the benefit for none of that.
//!
//! # Bounds narrow, the residual decides
//!
//! Scan bounds are an optimisation. Every conjunct of the predicate stays in
//! [`Plan::residual`] and is re-checked against each row, even when the bounds
//! already imply it. That costs a little work per row and buys two things: a
//! bug in bound derivation can only make a scan slower, never wrong; and a
//! mandatory security predicate is enforced by evaluation, not by the planner
//! having correctly turned it into a range. Dropping conjuncts that the bounds
//! provably enforce is a later optimisation, and one that has to be argued for
//! rather than assumed.
//!
//! # One exception: an index the bounds do not describe
//!
//! A *partial* index breaks the rule above, and is the one place in the planner
//! where a mistake loses rows rather than time. Its entries are only the rows
//! its own predicate admits, so choosing it does not narrow a scan — it changes
//! which rows exist to be scanned, and no amount of residual filtering puts a
//! missing entry back. [`implies`] is therefore the gate: an index carrying a
//! predicate is a candidate only when the query's predicate can be *shown* to
//! land inside it, and "cannot show it" means the index is not used. That
//! asymmetry is deliberate — a missed index costs a scan, a wrong one costs
//! correctness.
//!
//! # Where partial and expression indexes are declared
//!
//! A partial index is declared on [`IndexDef`], where it belongs, and the
//! record store maintains it: `IndexDef` lives in `slate-schema` and cannot
//! name an [`Expr`], but it does not have to — it holds an
//! `Arc<dyn slate_schema::Predicate>`, the same seam a `CHECK` uses, and the
//! kernel implements that trait for `Expr`. The write path only needs to *run*
//! the predicate, which the trait gives it. The planner needs to *read* it, and
//! gets it back with a downcast.
//!
//! That downcast is the price of one declaration site rather than two. Stating
//! a partial index's predicate twice — once for the writer to run, once for the
//! planner to reason about — would put a silent wrong-answer bug one edit away
//! from existing. An index whose predicate is not an `Expr` is one the planner
//! never chooses, which is the safe direction.
//!
//! An **expression** index is declared the same way, through a second seam:
//! [`slate_schema::Computed`] produces a value where `Predicate` produces a
//! verdict, and the kernel implements it for
//! [`Scalar`](crate::scalar::Scalar). The write path computes the key from the
//! row, the planner downcasts back to a `Scalar` to match it against what a
//! query computes, and an index whose expression is not a `Scalar` is
//! maintained but never chosen — the same asymmetry, the same safe direction.
//!
//! # A computed value is a value an index can hold
//!
//! Both statistics and covering turn on treating the value an expression index
//! keys on as an ordinary value that happens to live in the entry rather than
//! in the row.
//!
//! - **Statistics.** [`TableStats`] records them against the index, because the
//!   value has no ordinal of its own. A query that computes the same expression
//!   gives it one, and `planning_stats` copies the statistics onto that ordinal
//!   for the length of the plan, so every estimate reads them the way it reads
//!   a column's.
//! - **Covering.** [`covers`] asks what each *value* the query needs would have
//!   to be read from. A column has to be in the entry; a computed value is in
//!   the entry when this is the index that computes it, and otherwise needs
//!   whatever feeds it to be. The executor then takes that one value out of the
//!   entry rather than evaluating it from columns the entry never carried.

use crate::error::KernelError;
use crate::exec::DEFAULT_PREFETCH;
use crate::query::{AccessHint, SortKey};
use crate::stats::{
    POINT_READ_COST, SCAN_OPEN_COST, SCAN_ROW_COST, SORT_ROW_COST, TableStats, pipelined_read_cost,
};
use crate::store::{KeyRange, ScanOrder};
use crate::{expr::Expr, keys};
use core::ops::Bound;
use slate_schema::{ColumnSet, IndexDef, IndexId, Ordinal, TableDef};
use slate_tuple::{Direction, Value, encode_prefix_into, encode_value_into, prefix_successor};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::expr::CmpOp;

/// How the executor will reach the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// Fetch exactly one row by its primary key.
    ///
    /// Distinct from a one-row `TableScan` because it is a point read rather
    /// than opening an iterator, and a point read is the cheapest thing the
    /// storage layer does.
    PointGet {
        /// The full primary key, in key order.
        key: Vec<Value>,
    },
    /// Read rows directly from the table's key range.
    TableScan {
        /// The range to scan. Always within the table's own prefix.
        range: KeyRange,
    },
    /// Walk an index, then fetch each row by the primary key it points at.
    IndexScan {
        /// Which index.
        index: IndexId,
        /// The range to scan. Always within the index's own prefix.
        range: KeyRange,
        /// Whether the index entry alone answers the query.
        ///
        /// When true the row lookup is skipped entirely, which is the
        /// difference between one read per matching row and none. See
        /// [`Projection`].
        covering: bool,
    },
    /// Walk several disjoint ranges of one index, one after another.
    ///
    /// What `IN` over an *indexed non-key* column becomes, the way
    /// [`Access::PointGets`] is what `IN` over the key becomes. `kind IN ('a',
    /// 'q')` names two values that need not sit next to each other, so the one
    /// range holding both also holds every `kind` between them — which on a
    /// column worth indexing is most of the index. Two ranges hold neither.
    ///
    /// # Order
    ///
    /// The ranges are disjoint and held in ascending *key* order — key, not
    /// value, because a descending index column stores its values reversed and
    /// the ranges are sorted by the bytes rather than by the literals. Each one
    /// pins the leading column to a different value, so walking them in order
    /// yields exactly what a single scan of the whole index would have yielded
    /// with the other values left out. That is what lets an `ORDER BY` on the
    /// index's own columns still stream, rather than being sorted.
    ///
    /// A descending scan walks the list backwards, each range descending. The
    /// executor does the reversing, because it is the executor that knows which
    /// way it is walking; the plan states the ranges once, ascending.
    IndexScans {
        /// Which index.
        index: IndexId,
        /// Disjoint ranges within the index's own prefix, in ascending key
        /// order. Never empty, and never a single range — one range is an
        /// [`Access::IndexScan`], and having two spellings of the same plan
        /// would mean two things to test and two things to cost.
        ranges: Vec<KeyRange>,
        /// Whether the index entries alone answer the query. As
        /// [`Access::IndexScan::covering`].
        covering: bool,
    },
    /// Fetch several rows by primary key, all at once.
    ///
    /// What `IN` over a key becomes. Distinct from a scan because the rows are
    /// read directly and concurrently — loading ten records by id is one wave
    /// of round trips, where scanning for them is the whole table.
    PointGets {
        /// The keys to read, in key order.
        keys: Vec<Vec<Value>>,
    },
    /// The predicate cannot be satisfied; read nothing.
    Nothing,
}

/// How many keys an `IN` may become before it stays a filter.
///
/// Each one is a key held in the plan and a read issued by the executor. Past
/// some size the reads cost more than scanning the table would, and the cost
/// model says so on its own — but it should not have to build a hundred
/// thousand keys to find that out.
pub const MAX_POINT_GETS: usize = 1024;

/// How many ranges an `IN` over an index may become before it stays one range.
///
/// The same bound as [`MAX_POINT_GETS`] and for the same reason: the cost model
/// already rejects a large set on its own — each range is a scan to open, so
/// `k` ranges start `k` requests down against the hundred and twenty-six a
/// whole table scan costs at a million rows — but it should not have to encode
/// a hundred thousand key pairs to find that out.
pub const MAX_INDEX_RANGES: usize = 1024;

/// Which columns a query needs.
///
/// Naming fewer columns is not only less data to carry: if an index holds all
/// of them, the row itself never has to be read. That turns the dominant cost of
/// an index scan — one point read per matching row — into nothing at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Projection {
    /// Every column of the table.
    #[default]
    All,
    /// Only these columns.
    ///
    /// Columns outside the list read back as [`Value::Null`], which is
    /// indistinguishable from a stored null — so a projected row is for a caller
    /// that knows what it asked for, not for round-tripping back into storage.
    ///
    /// Two sets of columns come back regardless: the primary key, which is
    /// decoded from the row's key before its body is touched, and whatever the
    /// predicate reads, which has to be decoded to evaluate it. Both are free by
    /// the time the row is returned, so withholding them would cost work rather
    /// than save it.
    ///
    /// A computed value's inputs are the exception, and deliberately so: they
    /// are read on the way to producing it and are then put back to null.
    /// Returning them would be free here too, and it would make a row's
    /// contents depend on its plan — an index keyed on `lower(title)` holds the
    /// computed value and not `title`, so a scan of that index cannot produce
    /// the column, and no other path may either.
    Columns(Vec<Ordinal>),
}

impl Projection {
    /// Nothing but the predicate's own columns, for counting.
    #[must_use]
    pub const fn none() -> Self {
        Self::Columns(Vec::new())
    }

    /// The columns this projection needs, or `None` for all of them.
    #[must_use]
    pub fn columns(&self) -> Option<&[Ordinal]> {
        match self {
            Self::All => None,
            Self::Columns(columns) => Some(columns),
        }
    }
}

/// How selective the residual must be before it is worth decoding a row in two
/// passes.
///
/// Splitting the decode costs one extra walk over every row and saves a full
/// decode of every rejected one. A walk is roughly a third of a decode, so the
/// split pays once the residual rejects about a third; half leaves margin for
/// an estimate that is wrong in the usual direction.
pub const FILTER_FIRST_SELECTIVITY: f64 = 0.5;

/// A chosen access path plus the filter still to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// How to reach candidate rows.
    pub access: Access,
    /// The predicate to evaluate on every candidate. See the module docs: this
    /// is the authority, the bounds are only a shortcut.
    ///
    /// Shared rather than owned: it is the query's own predicate, deep-cloning
    /// it per plan copied every string literal in the tree for no reason.
    pub residual: Arc<Expr>,
    /// Direction to walk the access path in.
    pub order: ScanOrder,
    /// Sorting the executor must do, because no access path produced the
    /// requested order. `None` means the rows arrive already ordered.
    pub sort: Option<Vec<SortKey>>,
    /// Columns the residual predicate reads.
    ///
    /// A scan decodes these first and evaluates the filter before touching
    /// anything else, so a row that will be rejected never pays for the columns
    /// only its caller wanted.
    pub predicate_columns: ColumnSet,
    /// Columns the caller ends up seeing: the projection, plus the predicate's,
    /// since those are decoded anyway.
    pub output_columns: ColumnSet,
    /// Whether to decode the predicate's columns first and the rest only for
    /// rows that survive.
    ///
    /// Only worth it when the residual rejects enough rows to cover the extra
    /// pass; see [`FILTER_FIRST_SELECTIVITY`].
    pub filter_first: bool,
    /// Rows the planner expects this to return.
    pub estimated_rows: f64,
    /// Estimated cost, in object-storage round trips. See [`crate::stats`].
    pub estimated_cost: f64,
}

impl Plan {
    /// Narrow this plan to the rows strictly after `key` in its own scan order.
    ///
    /// This is keyset pagination, and it is a *narrowing of a plan* rather than
    /// an input to planning because that is what it is: the query is the same
    /// query, asked again from further along. Expressing it here also keeps
    /// [`plan_hinted`]'s signature — already nine arguments — from growing a
    /// tenth for something that does not affect which access path is chosen.
    ///
    /// # Why this is not `OFFSET`
    ///
    /// `OFFSET n` reads and throws away `n` rows. The executor says so in a
    /// comment — *"an offset still has to find the rows it discards; there is no
    /// cheaper way to know which ones they are"* — and that is true when all you
    /// have is a count. Given the *key* the last page ended on there is a
    /// cheaper way, because the key range itself can start after it. Page five
    /// hundred then costs what page one costs.
    ///
    /// The correctness difference matters more than the cost. `OFFSET` counts
    /// rows, so a row inserted or deleted ahead of the cursor between two pages
    /// shifts every later page by one: a row is silently skipped or served
    /// twice, and nothing anywhere reports it. A key does not move when its
    /// neighbours change.
    ///
    /// # Errors
    /// If the access path does not walk the table's primary key — an index scan
    /// yields rows in the index's order, where a primary key says nothing about
    /// where the page ended. Filtering on it anyway would silently drop rows,
    /// which is the failure this exists to remove rather than relocate. Callers
    /// reaching this through [`Query::after`](crate::Query::after) do not see
    /// it, because a cursor pins the access path to the primary key.
    pub fn resume_after(mut self, table: &TableDef, key: &[Value]) -> crate::Result<Self> {
        if key.len() != table.primary_key().len() {
            return Err(KernelError::InvalidCursor {
                table: table.name().to_owned(),
                reason: format!(
                    "a cursor is a whole primary key: `{}` has {} key column{}, and this one has {}",
                    table.name(),
                    table.primary_key().len(),
                    if table.primary_key().len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    key.len(),
                ),
            });
        }
        let boundary = keys::row_key(table, key);

        self.access = match self.access {
            Access::TableScan { range } => Access::TableScan {
                // Excluded, so the row the last page *ended* on is not also the
                // row the next page starts with. An inclusive bound would serve
                // every boundary row twice, which is the bug people write when
                // they reach for this by hand.
                range: range.intersect(match self.order {
                    ScanOrder::Ascending => {
                        KeyRange::new(Bound::Excluded(boundary), Bound::Unbounded)
                    }
                    // Descending walks the range from its top, so "after" is
                    // below: the cursor becomes the exclusive *end*.
                    ScanOrder::Descending => {
                        KeyRange::new(Bound::Unbounded, Bound::Excluded(boundary))
                    }
                }),
            },
            // One row, and the question is only whether it is on this side of
            // the cursor. Cheaper to answer here than to open a range holding
            // at most one key.
            Access::PointGet { key: one } => {
                let at = keys::row_key(table, &one);
                let past = match self.order {
                    ScanOrder::Ascending => at > boundary,
                    ScanOrder::Descending => at < boundary,
                };
                if past {
                    Access::PointGet { key: one }
                } else {
                    Access::Nothing
                }
            }
            // Nothing is still nothing, and narrowing it is not an error: a
            // caller paging through an empty result should reach the end, not a
            // refusal.
            Access::Nothing => Access::Nothing,
            Access::IndexScan { index, .. } | Access::IndexScans { index, .. } => {
                return Err(KernelError::InvalidCursor {
                    table: table.name().to_owned(),
                    reason: format!(
                        "this plan walks index {} and yields rows in that index's order, where a \
                         primary key does not say where the page ended. Drop the ORDER BY, or \
                         page by offset",
                        index.0
                    ),
                });
            }
            Access::PointGets { .. } => {
                return Err(KernelError::InvalidCursor {
                    table: table.name().to_owned(),
                    reason: "this plan reads a set of keys named by an `IN`, which is already \
                             bounded and is not a range to resume inside"
                        .to_owned(),
                });
            }
        };

        // An empty range reads nothing, and its estimates have to say so — the
        // same correction `plan_hinted` makes, for the same reason: a plan that
        // reads no rows but is costed as if it read the table makes `EXPLAIN`
        // lie about the page the caller is actually on.
        if matches!(&self.access, Access::TableScan { range } if range.is_empty()) {
            self.access = Access::Nothing;
        }
        if matches!(self.access, Access::Nothing) {
            self.estimated_rows = 0.0;
            self.estimated_cost = 0.0;
        }
        Ok(self)
    }
}

/// What a predicate says about one column.
///
/// Borrows its literals from the predicate. Cloning them copied every string in
/// the query on every plan, to read them once and throw them away.
#[derive(Debug, Default, Clone)]
struct ColumnConstraints<'a> {
    /// A value the column must equal, including `IS NULL` as "equals null".
    equals: Option<&'a Value>,
    /// Range comparisons on the column.
    ranges: Vec<(CmpOp, &'a Value)>,
    /// Values from an `IN`, any one of which the column may equal.
    any_of: Option<&'a [Value]>,
    /// A literal prefix every matching value must start with, from a `LIKE`
    /// anchored at the front. Owned because it is derived from the pattern
    /// rather than borrowed out of it — an escape makes them differ.
    starts_with: Option<Value>,
}

/// How deep the executor will pipeline, given what the caller asked for.
///
/// A window narrows it: fetching sixteen rows to return ten spends six round
/// trips on rows that are discarded, so the cursor caps the prefetch at the
/// window and the cost model has to agree, or it credits the plan with
/// concurrency the executor will not use.
fn prefetch_depth(limit: Option<usize>) -> usize {
    match limit {
        Some(limit) if limit > 0 => limit.min(DEFAULT_PREFETCH),
        _ => DEFAULT_PREFETCH,
    }
}

/// The real columns behind an ordinal, which for a computed one is whatever
/// feeds it.
///
/// A computed value lives past the table's own columns, so a filter or a sort
/// key naming one refers to nothing the decoder knows about. What has to be
/// read is its inputs.
fn expanded_inputs(
    column: Ordinal,
    width: usize,
    compute: &[crate::scalar::Scalar],
) -> Vec<Ordinal> {
    if column.0 < width {
        return vec![column];
    }
    compute
        .get(column.0 - width)
        .map(|scalar| scalar.columns().into_iter().collect())
        .unwrap_or_default()
}

/// A candidate access path and what it is expected to cost.
struct Candidate {
    access: Access,
    /// Fraction of the table the access path's bounds admit, before the
    /// residual filters any further.
    bound_selectivity: f64,
    /// The order rows come out in, as `(column, stored direction)`.
    natural_order: Vec<(Ordinal, Direction)>,
}

impl Candidate {
    /// Whether walking this path in `order` already produces `sort`.
    ///
    /// A requested order is satisfied when it is a prefix of what the path
    /// produces. Columns pinned to a single value by the bounds are skipped:
    /// ordering by a column that can only hold one value is a no-op, and
    /// missing that would sort a great many results that were already in order.
    fn satisfies(&self, sort: &[SortKey], order: ScanOrder, pinned: &BTreeSet<Ordinal>) -> bool {
        let mut produced = self
            .natural_order
            .iter()
            .filter(|(column, _)| !pinned.contains(column));
        for key in sort.iter().filter(|k| !pinned.contains(&k.column)) {
            let Some((column, stored)) = produced.next() else {
                return false;
            };
            if *column != key.column || !key.matches_storage(*stored, order) {
                return false;
            }
        }
        true
    }
}

impl Candidate {
    /// Estimated cost in round trips, and rows returned.
    ///
    /// `total_selectivity` is the whole predicate's; the ratio between it and
    /// the bounds' is how much filtering still happens after reading, which is
    /// what decides how many rows a limit makes the path touch.
    fn estimate(
        &self,
        stats: &TableStats,
        total_selectivity: f64,
        limit: Option<usize>,
        must_sort: bool,
    ) -> (f64, f64) {
        if matches!(self.access, Access::Nothing) {
            return (0.0, 0.0);
        }
        let rows = stats.row_count as f64;
        if let Access::PointGet { .. } = self.access {
            return (1.0f64.min(rows), POINT_READ_COST);
        }
        if let Access::PointGets { keys } = &self.access {
            // Every key is read whether the residual keeps it or not, so the
            // cost is the whole set; the rows returned are what survives.
            let asked = keys.len() as f64;
            let kept = (rows * total_selectivity).clamp(0.0, asked);
            let returned = limit.map_or(kept, |limit| kept.min(limit as f64));
            let reads = pipelined_read_cost(asked, prefetch_depth(limit));
            return (returned, reads);
        }

        let admitted = (rows * self.bound_selectivity).max(1.0);
        let matched = (rows * total_selectivity).max(0.0);
        // A sort has to see every matching row before it can return the first,
        // so a limit stops being a reason to read less.
        let mut returned = matched;
        if let Some(limit) = limit
            && !must_sort
        {
            returned = returned.min(limit as f64);
        }

        // How much of what the bounds admit still has to be looked at to
        // produce `returned` rows. With no limit this is everything admitted.
        let residual_selectivity = if self.bound_selectivity > 0.0 {
            (total_selectivity / self.bound_selectivity).clamp(f64::MIN_POSITIVE, 1.0)
        } else {
            1.0
        };
        let touched = (returned / residual_selectivity).clamp(1.0, admitted);

        // Every range is an iterator of its own, and opening one is a request.
        // They are walked one after another rather than overlapped, so this is
        // also `k` round trips of latency — the cost unit counts requests, and
        // on that axis the two happen to agree.
        let (opens, fetches_rows) = match &self.access {
            Access::IndexScan { covering, .. } => (1.0, !covering),
            Access::IndexScans {
                ranges, covering, ..
            } => (ranges.len() as f64, !covering),
            _ => (1.0, false),
        };
        let mut cost = opens * SCAN_OPEN_COST + touched * SCAN_ROW_COST;
        if fetches_rows {
            // The row has to be read before the residual can even be evaluated,
            // so this is per row touched, not per row returned. Issued in
            // waves, because the executor overlaps them — and at the same
            // depth the executor will actually use, which a limit narrows.
            cost += pipelined_read_cost(touched, prefetch_depth(limit));
        }
        if must_sort && matched > 1.0 {
            cost += matched * matched.log2() * SORT_ROW_COST;
        }
        let returned = limit.map_or(returned, |limit| returned.min(limit as f64));
        (returned, cost)
    }
}

/// Choose an access path for `predicate` over `table`.
///
/// `order` is the direction to walk the chosen path; a plan does not sort, so a
/// caller that needs a specific row order should ask for an index that stores
/// it.
#[must_use]
pub fn plan(table: &TableDef, predicate: &Expr, order: ScanOrder) -> Plan {
    plan_projected(table, predicate, order, &Projection::All)
}

/// Choose an access path, given the columns the caller actually needs.
///
/// A narrower projection can make an index-only scan possible; see
/// [`Projection`].
#[must_use]
pub fn plan_projected(
    table: &TableDef,
    predicate: &Expr,
    order: ScanOrder,
    projection: &Projection,
) -> Plan {
    plan_with(
        table,
        predicate,
        order,
        projection,
        &TableStats::assumed(),
        None,
    )
}

/// Choose an access path using known statistics and a row limit.
///
/// The statistics decide whether an index is worth its point reads; the limit
/// decides how much of the chosen path will actually be walked. Both change the
/// answer, so both belong in the decision rather than being discovered at
/// execution time.
#[must_use]
pub fn plan_with(
    table: &TableDef,
    predicate: &Expr,
    order: ScanOrder,
    projection: &Projection,
    stats: &TableStats,
    limit: Option<usize>,
) -> Plan {
    plan_full(
        table,
        Arc::new(predicate.clone()),
        order,
        projection,
        stats,
        limit,
        &[],
    )
}

/// What a query needs to be able to read.
///
/// `All` is kept as a case rather than expanded into a set: it is the common
/// one, and materialising every column's ordinal per query to then ask whether
/// an index covers them is work with a known answer.
enum Needed {
    All,
    Some(BTreeSet<Ordinal>),
}

/// Choose an access path, given everything the query asks for.
///
/// `sort` is what makes an ordered index worth more than its point reads: a
/// path that already produces the requested order streams, and one that does
/// not has to materialise every matching row before returning the first.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn plan_full(
    table: &TableDef,
    predicate: Arc<Expr>,
    order: ScanOrder,
    projection: &Projection,
    stats: &TableStats,
    limit: Option<usize>,
    sort: &[SortKey],
) -> Plan {
    plan_hinted(
        table,
        predicate,
        order,
        projection,
        stats,
        limit,
        sort,
        None,
        &[],
    )
}

/// [`plan_full`], with an access path the caller insists on.
///
/// A hint narrows which candidates are considered; everything else — bounds,
/// residual, costing — is unchanged, so a hinted plan is a real plan and not a
/// second code path that could disagree with the first.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn plan_hinted(
    table: &TableDef,
    predicate: Arc<Expr>,
    order: ScanOrder,
    projection: &Projection,
    stats: &TableStats,
    limit: Option<usize>,
    sort: &[SortKey],
    hint: Option<AccessHint>,
    compute: &[crate::scalar::Scalar],
) -> Plan {
    let conjuncts = predicate.conjuncts();

    // A comparison against a null literal is Unknown for every row, so the
    // conjunction can never be true and there is nothing to read.
    let unsatisfiable = conjuncts.iter().any(|c| {
        matches!(c, Expr::Compare { value, .. } if value.is_null()) || matches!(c, Expr::False)
    });
    // A computed value lives past the table's own columns, and reading one
    // means reading its inputs. Ordinals outside the table are dropped and
    // replaced by whatever feeds them, so the rest of the planner never sees a
    // column that does not exist.
    let width = table.columns().len();
    let predicate_columns: ColumnSet = predicate
        .columns()
        .into_iter()
        .flat_map(|c| expanded_inputs(c, width, compute))
        .collect();
    // Columns of the answer, which is not the same set as the columns that
    // have to be read to produce it. A computed value's inputs are read and
    // are *not* part of the answer: `SELECT id, lower(title)` asked for `id`,
    // and handing back `title` as well would make a row's contents depend on
    // whether the plan happened to need the column — the same shape of bug the
    // planner oracle found in the covering scan. The executor adds the inputs
    // to what it decodes and takes them out of the row again; see
    // `exec::QueryCursor`.
    //
    // It has to be this way round now that an expression index can cover a
    // query. Such an index holds `lower(title)` and not `title`, so a plan that
    // reads it *cannot* return `title`; a table scan of the same query must
    // therefore not return it either, or the two paths answer differently.
    let mut output_columns = predicate
        .columns()
        .into_iter()
        .filter(|c| c.0 < width)
        .collect::<ColumnSet>();
    match projection.columns() {
        None => output_columns = ColumnSet::all(table.columns().len()),
        Some(columns) => {
            for column in columns.iter().filter(|c| c.0 < width) {
                output_columns.insert(*column);
            }
            // A column the sort orders by has to be decoded even when the
            // caller did not ask to see it. Leaving it out does not fail: it
            // reads back as null, every row compares equal, and the result
            // comes out in whatever order the scan happened to produce. A
            // wrong order that looks like an order is worse than an error.
            for key in sort.iter().filter(|k| k.column.0 < width) {
                output_columns.insert(key.column);
            }
        }
    }

    if unsatisfiable {
        return Plan {
            access: Access::Nothing,
            residual: Arc::new(predicate.prepared()),
            order,
            sort: None,
            predicate_columns,
            output_columns,
            filter_first: false,
            estimated_rows: 0.0,
            estimated_cost: 0.0,
        };
    }

    let constraints = collect_constraints(&conjuncts);

    // From here on, a computed value the table has an analysed expression index
    // for is an ordinary column with ordinary statistics. See `planning_stats`.
    let stats = planning_stats(table, stats, compute, width);
    let stats = stats.as_ref();

    let total_selectivity = stats.predicate_selectivity(&predicate);

    // Columns the bounds pin to a single value are already constant across the
    // result, so ordering by them is free.
    let pinned: BTreeSet<Ordinal> = constraints
        .iter()
        .filter(|(_, c)| c.equals.is_some())
        .map(|(ordinal, _)| *ordinal)
        .collect();

    let mut best: Option<(Candidate, bool, f64, f64)> = None;
    let mut consider = |candidate: Candidate| {
        let must_sort = !sort.is_empty() && !candidate.satisfies(sort, order, &pinned);
        let (rows, cost) = candidate.estimate(stats, total_selectivity, limit, must_sort);
        let better = match &best {
            None => true,
            // Ties go to whichever is already chosen, so the order candidates
            // are generated in decides them: the primary key first, then
            // indexes in declaration order. Deterministic beats arbitrary.
            Some((_, _, _, current)) => cost < *current,
        };
        if better {
            best = Some((candidate, must_sort, rows, cost));
        }
    };

    // What the query needs to see: the projected columns plus whatever the
    // predicate reads, since the predicate still has to be evaluated.
    // Computed ordinals are left as they are rather than replaced by their
    // inputs, because whether an index can answer one depends on the index:
    // an expression index *is* that value and hands it over out of the entry,
    // and every other index has to hold whatever feeds it. Expanding here
    // would settle that question before knowing which index was being asked.
    let needed = match projection.columns() {
        None => Needed::All,
        Some(columns) => {
            let mut set: BTreeSet<Ordinal> = predicate.columns().into_iter().collect();
            set.extend(columns.iter().copied());
            // Same reason as `output_columns`: an index that does not hold the
            // sort column cannot answer the query on its own, however well it
            // covers the projection.
            set.extend(sort.iter().map(|key| key.column));
            // Every computed value is evaluated on every row whether or not
            // anything references it, so every one of them is needed however
            // narrow the projection.
            set.extend((0..compute.len()).map(|i| Ordinal(width + i)));
            Needed::Some(set)
        }
    };

    let ordered = !sort.is_empty();
    // A hint restricts the candidates rather than replacing the choice: the
    // one that survives is still costed, bounded and given a residual the
    // ordinary way.
    let mut considered = 0usize;
    if !matches!(hint, Some(AccessHint::Index(_))) {
        considered += 1;
        consider(match_primary_key(table, &constraints, stats, ordered));
        if let Some(candidate) = match_point_gets(table, &constraints, stats) {
            consider(candidate);
        }
    }
    let context = MatchContext {
        table,
        predicate: &predicate,
        constraints: &constraints,
        needed: &needed,
        stats,
        ordered,
        compute,
        width,
    };
    for index in table.indexes() {
        if matches!(hint, Some(AccessHint::TableScan))
            || matches!(hint, Some(AccessHint::Index(wanted)) if wanted != index.id())
        {
            continue;
        }
        // An index that cannot answer this query is not a candidate at all —
        // and that includes one the caller hinted at. A hint is advice about
        // which of several correct plans to take, never permission to read a
        // partial index that does not hold the rows asked for.
        let Some(candidate) = match_index(&context, index) else {
            continue;
        };
        considered += 1;
        consider(candidate);
    }
    // An index hint naming an index that is gone leaves nothing to consider.
    // Falling back beats refusing: a hint is advice.
    if considered == 0 {
        consider(match_primary_key(table, &constraints, stats, ordered));
    }

    // How much of what the access path admits the residual still rejects. Only
    // worth splitting the decode in two when that is a real fraction.
    let residual_selectivity = best.as_ref().map_or(1.0, |(candidate, ..)| {
        if candidate.bound_selectivity > 0.0 {
            (total_selectivity / candidate.bound_selectivity).clamp(0.0, 1.0)
        } else {
            1.0
        }
    });
    let filter_first = residual_selectivity < FILTER_FIRST_SELECTIVITY;

    let (access, must_sort, estimated_rows, estimated_cost) = best.map_or_else(
        || {
            (
                Access::TableScan {
                    range: KeyRange::prefix(&keys::table_prefix(table)),
                },
                !sort.is_empty(),
                stats.row_count as f64,
                SCAN_OPEN_COST + stats.row_count as f64 * SCAN_ROW_COST,
            )
        },
        |(candidate, must_sort, rows, cost)| (candidate.access, must_sort, rows, cost),
    );

    // Contradictory bounds are an empty range, not a scan of one. `id > 28 AND
    // id < 0` is legal and selects nothing, and the bounds derived from it run
    // backwards — which a store is entitled to treat as a programming error
    // (`BTreeMap::range` panics on it). Saying `Nothing` is both the correct
    // answer and the cheapest way to reach it.
    // Contradictory bounds are an empty range, and a plan over an empty range
    // reads nothing — so its estimates have to say so too. Leaving them at what
    // the range would have cost made `EXPLAIN` report a plan that reads no rows
    // as costing twelve round trips and returning eighty-eight thousand, which
    // the plan snapshot caught on its first run.
    let (access, estimated_rows, estimated_cost) = match &access {
        Access::TableScan { range } | Access::IndexScan { range, .. } if range.is_empty() => {
            (Access::Nothing, 0.0, 0.0)
        }
        // A multi-range scan drops its empty ranges as it is built, so an empty
        // list here means every value in the `IN` contradicted another bound.
        Access::IndexScans { ranges, .. } if ranges.iter().all(KeyRange::is_empty) => {
            (Access::Nothing, 0.0, 0.0)
        }
        _ => (access, estimated_rows, estimated_cost),
    };

    Plan {
        access,
        // Prepared once here rather than at each cursor: every consumer of a
        // plan evaluates this per candidate row, and the cost of arranging an
        // `IN` list for lookup should be paid once per plan, not per scan.
        residual: Arc::new(predicate.prepared()),
        order,
        sort: must_sort.then(|| sort.to_vec()),
        predicate_columns,
        output_columns,
        filter_first,
        estimated_rows,
        estimated_cost,
    }
}

fn collect_constraints<'a>(conjuncts: &[&'a Expr]) -> Vec<(Ordinal, ColumnConstraints<'a>)> {
    let mut out: Vec<(Ordinal, ColumnConstraints<'a>)> = Vec::new();
    let entry = |ordinal: Ordinal, out: &mut Vec<(Ordinal, ColumnConstraints<'a>)>| -> usize {
        match out.iter().position(|(o, _)| *o == ordinal) {
            Some(i) => i,
            None => {
                out.push((ordinal, ColumnConstraints::default()));
                out.len() - 1
            }
        }
    };

    for conjunct in conjuncts {
        match conjunct {
            Expr::Compare { column, op, value } if !value.is_null() => {
                let i = entry(*column, &mut out);
                if let Some((_, c)) = out.get_mut(i) {
                    match op {
                        CmpOp::Eq => c.equals = Some(value),
                        CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                            c.ranges.push((*op, value));
                        }
                        // `<>` selects everything but one point, which is not a
                        // range; it stays a residual filter.
                        CmpOp::Ne => {}
                    }
                }
            }
            // A `LIKE` anchored at the front confines its matches to a range
            // of the keyspace: every value starting with `abc` sorts between
            // `abc` and the first thing above it. An unanchored pattern says
            // nothing about where its matches are and stays a residual.
            // Only a case-sensitive pattern: `ILIKE 'abc%'` also matches
            // `ABC…`, which does not sort next to `abc…`, so there is no one
            // range that holds every match.
            Expr::Like {
                column,
                pattern,
                negated: false,
                insensitive: false,
            } => {
                if let Some(prefix) = crate::expr::like_prefix(pattern) {
                    let i = entry(*column, &mut out);
                    if let Some((_, c)) = out.get_mut(i) {
                        c.starts_with.get_or_insert(Value::Str(prefix));
                    }
                }
            }
            // An `IN` pins the column to one of several values. It is not a
            // range — the values need not be adjacent — so it becomes several
            // reads rather than one wider one.
            Expr::In { column, values } if !values.is_empty() => {
                let i = entry(*column, &mut out);
                if let Some((_, c)) = out.get_mut(i) {
                    // Two `IN`s on one column would have to be intersected;
                    // the first is kept and the rest stay residual, which is
                    // narrower than nothing and always correct.
                    c.any_of.get_or_insert(values.as_slice());
                }
            }
            // `IS NULL` is a prefix like any other value, because null has its
            // own encoding. `IS NOT NULL` is not, so it stays residual.
            Expr::IsNull {
                column,
                negated: false,
            } => {
                let i = entry(*column, &mut out);
                if let Some((_, c)) = out.get_mut(i) {
                    c.equals = Some(&Value::Null);
                }
            }
            _ => {}
        }
    }
    out
}

fn constraints_for<'c, 'a>(
    constraints: &'c [(Ordinal, ColumnConstraints<'a>)],
    ordinal: Ordinal,
) -> Option<&'c ColumnConstraints<'a>> {
    constraints
        .iter()
        .find(|(o, _)| *o == ordinal)
        .map(|(_, c)| c)
}

/// Match the predicate against a key's columns, longest equality prefix first.
///
/// Returns the encoded key prefix and whether a range bound was applied. Shared
/// by the primary key and by every secondary index, since both are just ordered
/// column lists over the same encoding.
fn match_key(
    base: Vec<u8>,
    key_columns: &[(Ordinal, Direction)],
    constraints: &[(Ordinal, ColumnConstraints<'_>)],
    stats: &TableStats,
) -> (KeyRange, f64) {
    let mut prefix = base;
    let mut equality_columns = 0;
    let mut selectivity = 1.0f64;

    for (ordinal, direction) in key_columns {
        let Some(value) = constraints_for(constraints, *ordinal).and_then(|c| c.equals) else {
            break;
        };
        selectivity *= if value.is_null() {
            stats.column(*ordinal).null_fraction.max(f64::MIN_POSITIVE)
        } else {
            stats.equality_selectivity(*ordinal)
        };
        encode_value_into(&mut prefix, value, *direction);
        equality_columns += 1;
    }

    // A `LIKE` anchored at the front behaves exactly like a range on the
    // column after the equality prefix: every value starting with `abc`
    // encodes to something beginning with the encoding of `abc` minus its
    // terminator, so the matches are one contiguous span of the keyspace.
    if let Some((ordinal, direction)) = key_columns.get(equality_columns)
        && let Some(constraint) = constraints_for(constraints, *ordinal)
        && constraint.equals.is_none()
        && constraint.ranges.is_empty()
        && let Some(start) = constraint.starts_with.as_ref()
    {
        let mut bounded = prefix.clone();
        if encode_prefix_into(&mut bounded, start, *direction) {
            if let Value::Str(text) = start {
                selectivity *= stats.prefix_selectivity(*ordinal, text);
            }
            return (KeyRange::prefix(&bounded), selectivity);
        }
    }

    // At most one range bound, on the column right after the equality prefix:
    // a range on a later column is not contiguous in the key.
    let next = key_columns.get(equality_columns);
    let range_term = next.and_then(|(ordinal, direction)| {
        constraints_for(constraints, *ordinal)
            .filter(|c| c.equals.is_none())
            .map(|c| (c.ranges.clone(), *direction))
    });

    let prefix_range = KeyRange::prefix(&prefix);
    match range_term {
        Some((ranges, direction)) if !ranges.is_empty() => {
            let mut result = prefix_range;
            for (op, value) in &ranges {
                result = result.intersect(bound_for(&prefix, *op, value, direction));
            }
            if let Some((ordinal, _)) = key_columns.get(equality_columns) {
                // Through the histogram where there is one, so the bounds a
                // scan gets and the rows the planner expects from them come
                // from the same belief about the data.
                selectivity *= stats.bounded_selectivity(*ordinal, &ranges);
            }
            (result, selectivity)
        }
        _ => (prefix_range, selectivity),
    }
}

/// The key range implied by `column <op> value`, given the key prefix that
/// precedes the column.
fn bound_for(prefix: &[u8], op: CmpOp, value: &Value, direction: Direction) -> KeyRange {
    // A descending column stores values in reverse, so the byte-space bound is
    // the mirror of the value-space one.
    let op = match direction {
        Direction::Asc => op,
        Direction::Desc => op.mirrored(),
    };

    let mut boundary = prefix.to_vec();
    encode_value_into(&mut boundary, value, direction);

    // `boundary` begins with a keyspace discriminator byte, so it can never be
    // all `0xFF` and its successor always exists. The fallback keeps the
    // function total.
    let after_boundary = prefix_successor(&boundary);

    match op {
        CmpOp::Lt => KeyRange::new(Bound::Unbounded, Bound::Excluded(boundary)),
        CmpOp::Le => KeyRange::new(
            Bound::Unbounded,
            after_boundary.map_or(Bound::Unbounded, Bound::Excluded),
        ),
        CmpOp::Gt => after_boundary.map_or_else(
            || empty_range(&boundary),
            |start| KeyRange::new(Bound::Included(start), Bound::Unbounded),
        ),
        CmpOp::Ge => KeyRange::new(Bound::Included(boundary), Bound::Unbounded),
        // Neither of these narrows a range; they are handled as residuals.
        CmpOp::Eq | CmpOp::Ne => KeyRange::all(),
    }
}

fn empty_range(at: &[u8]) -> KeyRange {
    KeyRange::new(Bound::Included(at.to_vec()), Bound::Excluded(at.to_vec()))
}

fn match_primary_key(
    table: &TableDef,
    constraints: &[(Ordinal, ColumnConstraints<'_>)],
    stats: &TableStats,
    ordered: bool,
) -> Candidate {
    let key_columns: Vec<(Ordinal, Direction)> = table
        .primary_key()
        .iter()
        .map(|o| (*o, Direction::Asc))
        .collect();

    // Every key column pinned to a value is a single row, so read it directly
    // rather than opening a scan over a range that holds exactly one key.
    let full_key: Option<Vec<Value>> = key_columns
        .iter()
        .map(|(ordinal, _)| constraints_for(constraints, *ordinal).and_then(|c| c.equals.cloned()))
        .collect();
    if let Some(key) = full_key
        && !key.is_empty()
    {
        return Candidate {
            access: Access::PointGet { key },
            bound_selectivity: 1.0 / (stats.row_count.max(1) as f64),
            // One row is ordered by anything.
            natural_order: Vec::new(),
        };
    }

    let (range, bound_selectivity) =
        match_key(keys::table_prefix(table), &key_columns, constraints, stats);
    Candidate {
        access: Access::TableScan { range },
        bound_selectivity,
        natural_order: if ordered { key_columns } else { Vec::new() },
    }
}

/// Reading several rows by key, when an `IN` pins the whole key.
///
/// A candidate of its own rather than something `match_primary_key` returns
/// instead of a scan. A set of reads is not always cheaper than the scan it
/// replaces — twenty keys are, four hundred are not — so both have to be on
/// the table for the cost model to choose between them. Returning only this
/// one hid the better plan and let an unrelated index win by default.
fn match_point_gets(
    table: &TableDef,
    constraints: &[(Ordinal, ColumnConstraints<'_>)],
    stats: &TableStats,
) -> Option<Candidate> {
    let key_columns: Vec<(Ordinal, Direction)> = table
        .primary_key()
        .iter()
        .map(|o| (*o, Direction::Asc))
        .collect();
    let keys = point_get_set(&key_columns, constraints)?;
    let count = keys.len() as f64;
    Some(Candidate {
        access: Access::PointGets { keys },
        bound_selectivity: (count / stats.row_count.max(1) as f64).clamp(0.0, 1.0),
        // Read in key order, but a caller wanting an order should say so; a
        // set of reads is not an access path anything can be ordered by.
        natural_order: Vec::new(),
    })
}

/// The full primary keys an `IN` pins down, if it pins them all.
///
/// Every key column must be fixed: all but one by an equality, and exactly one
/// by an `IN`. Two `IN`s would be a cross product, which grows faster than it
/// is worth and is left to the scan.
///
/// The keys come back sorted, so the reads go out in key order and a caller
/// walking the result sees the same order a scan would have given.
fn point_get_set(
    key_columns: &[(Ordinal, Direction)],
    constraints: &[(Ordinal, ColumnConstraints<'_>)],
) -> Option<Vec<Vec<Value>>> {
    if key_columns.is_empty() {
        return None;
    }
    let mut fixed: Vec<Option<Value>> = Vec::with_capacity(key_columns.len());
    let mut varying: Option<(usize, &[Value])> = None;
    for (position, (ordinal, _)) in key_columns.iter().enumerate() {
        let constraint = constraints_for(constraints, *ordinal)?;
        if let Some(value) = constraint.equals {
            fixed.push(Some(value.clone()));
            continue;
        }
        // A second `IN` on the key: not handled, so this is not a point-get
        // set and the scan paths take it.
        if varying.is_some() {
            return None;
        }
        varying = Some((position, constraint.any_of?));
        fixed.push(None);
    }

    let (position, values) = varying?;
    if values.is_empty() || values.len() > MAX_POINT_GETS {
        return None;
    }
    let mut keys: Vec<Vec<Value>> = values
        .iter()
        .map(|value| {
            let mut key = fixed.clone();
            if let Some(slot) = key.get_mut(position) {
                *slot = Some(value.clone());
            }
            key.into_iter().flatten().collect()
        })
        .collect();
    // Every key must have come out whole. A short one would be a prefix, and
    // reading a prefix as a key is how you read the wrong row.
    if keys.iter().any(|key| key.len() != key_columns.len()) {
        return None;
    }
    // Sorted and deduplicated: `IN (1, 1, 2)` is two rows, not three, and a
    // duplicate key would return the same row twice.
    keys.sort();
    keys.dedup();
    Some(keys)
}

/// Which of the query's computed values `index` keys on, if any.
///
/// The one place that answers "is this expression index the one this query is
/// asking about", called by the planner to decide whether the index is a
/// candidate at all and by the executor to decide which computed value to take
/// out of an entry rather than evaluate. Two answers to that question would be
/// a silent wrong answer one edit away — the executor would fill in a value the
/// planner matched somewhere else — which is why it is a function and not a
/// rule written down twice.
///
/// `None` for an ordinary index, for an expression index whose expression is
/// not a [`Scalar`](crate::scalar::Scalar), and for one whose expression the
/// query does not compute.
#[must_use]
pub(crate) fn expression_position(
    index: &IndexDef,
    compute: &[crate::scalar::Scalar],
) -> Option<usize> {
    let expression = crate::record::index_expression(index)?;
    compute.iter().position(|scalar| scalar == expression)
}

/// The statistics to plan with, with every expression index's own statistics
/// copied onto the ordinal this query gives its value.
///
/// [`TableStats`] records an expression index's statistics against the index,
/// because the value is not a column and has no ordinal of its own. A query
/// that computes the same expression *does* give it one — `width + position` —
/// and from there on it is an ordinary column as far as every estimate is
/// concerned. Aliasing it here means `predicate_selectivity`, `match_key`,
/// `bounded_selectivity` and the rest read it without knowing it came from an
/// index.
///
/// The alternative was to thread a resolver — "is this ordinal computed, and if
/// so by which index" — through six signatures, every one of which would then
/// have a way to forget. This clones the statistics once per query, and only
/// when the query actually computes an analysed expression; every other query
/// borrows.
fn planning_stats<'a>(
    table: &TableDef,
    stats: &'a TableStats,
    compute: &[crate::scalar::Scalar],
    width: usize,
) -> std::borrow::Cow<'a, TableStats> {
    if compute.is_empty() {
        return std::borrow::Cow::Borrowed(stats);
    }
    // Two indexes on the same expression describe the same value and would
    // write the same numbers at the same ordinal, so the first wins and the
    // rest are skipped rather than fought over.
    let mut aliased: Option<TableStats> = None;
    let mut done: BTreeSet<Ordinal> = BTreeSet::new();
    for index in table.indexes() {
        let Some(position) = expression_position(index, compute) else {
            continue;
        };
        let ordinal = Ordinal(width + position);
        if !done.insert(ordinal) {
            continue;
        }
        let measured = stats.expression(index.id());
        let histogram = stats.expression_histogram(index.id());
        if measured.is_none() && histogram.is_none() {
            // Never analysed. Copying the default over would be the same
            // numbers `TableStats::column` already returns, for the price of
            // cloning the whole table's statistics.
            continue;
        }
        let mut next = aliased.take().unwrap_or_else(|| stats.clone());
        if let Some(measured) = measured {
            next = next.with_column(ordinal, measured);
        }
        if let Some(histogram) = histogram {
            next = next.with_histogram(ordinal, histogram.clone());
        }
        aliased = Some(next);
    }
    aliased.map_or(std::borrow::Cow::Borrowed(stats), std::borrow::Cow::Owned)
}

/// Whether `index` holds every value in `needed`.
///
/// An index entry carries its own columns and the primary key, so those are
/// what it can answer from — plus, for an expression index, the one computed
/// value it keys on, which is in the entry and needs no row at all.
fn covers(cx: &MatchContext<'_>, index: &IndexDef) -> bool {
    // Scanning two short slices beats building a set per index per query; both
    // are a handful of entries and this runs on every plan.
    let holds = |ordinal: Ordinal| {
        index.columns().iter().any(|c| c.ordinal == ordinal)
            || cx.table.primary_key().contains(&ordinal)
    };
    // The computed position this index's entries already hold the value for.
    // Every *other* computed value is still evaluated from the row, so what it
    // reads has to be reachable without one.
    let from_entry = expression_position(index, cx.compute);

    // One forward pass over the compute list, settling for each position
    // whether a scan of this index can produce it without reading the row.
    //
    // A position is available when the index keys on it — the executor takes
    // that value straight out of the entry — or when everything it reads is
    // already available. The second half is what makes this a *pass* rather
    // than a lookup: a scalar may read an earlier computed value, and
    // `SELECT id WHERE lower(title) = 'x' COMPUTING lower(title), length(#0)`
    // is answerable from an entry keyed on `lower(title)` even though nothing
    // in it holds `title`. Refusing that was the conservative choice this
    // replaces; it cost a row read per row on a query an entry could answer
    // outright.
    //
    // The direction of a mistake here has not changed and is worth restating:
    // a covering scan wrongly claimed returns nulls where the row had values,
    // which is a wrong answer rather than a slow one. That is why this runs
    // over the same `compute` slice the executor's `extend` walks, in the same
    // order, and why the planner oracle generates chained compute lists over
    // an expression index.
    //
    // Iterative rather than recursive so a compute list where each element
    // reads the two before it costs `n` steps and not `2^n`.
    let mut available: Vec<bool> = Vec::with_capacity(cx.compute.len());
    for (position, scalar) in cx.compute.iter().enumerate() {
        let supplied = Some(position) == from_entry
            || scalar.columns().into_iter().all(|input| {
                if input.0 < cx.width {
                    return holds(input);
                }
                match available.get(input.0 - cx.width) {
                    // An earlier computed value, already settled.
                    Some(ready) => *ready,
                    // Either a forward reference or an ordinal naming no
                    // computed value at all. The second reads nothing and is
                    // null on every path alike, so it is held; the first reads
                    // a value that does not exist yet and is *also* null on
                    // every path, but proving that means modelling evaluation
                    // order in the planner, so it is refused instead. A refusal
                    // costs a row read.
                    None => input.0 - cx.width >= cx.compute.len(),
                }
            });
        available.push(supplied);
    }

    let holds_value = |ordinal: Ordinal| {
        if ordinal.0 < cx.width {
            return holds(ordinal);
        }
        // An ordinal past the table naming no computed value reads nothing and
        // expands to nothing, which is trivially held — the executor makes it
        // null on every path alike.
        available.get(ordinal.0 - cx.width).copied().unwrap_or(true)
    };
    match cx.needed {
        // Everything, so an expression index has nothing to add: it would have
        // to hold every column of the table before the computed value mattered.
        Needed::All => (0..cx.table.columns().len()).map(Ordinal).all(holds),
        Needed::Some(columns) => columns.iter().copied().all(holds_value),
    }
}

/// Whether every row `query` admits is a row `index` admits.
///
/// The question a partial index turns on, and the only place in the planner
/// where "not sure" and "no" have to mean the same thing. A `true` here lets
/// the planner read an index that does not hold the whole table; if that
/// judgement is wrong the query silently returns fewer rows than it should, and
/// no residual can notice. So this answers `true` only for implications it can
/// demonstrate, and `false` for everything else including plenty that are in
/// fact true. A missed implication costs a table scan.
///
/// # What it can show
///
/// - the same term on both sides, syntactically;
/// - a conjunction, when any one conjunct suffices — so `a = 1 AND b = 2`
///   implies `a = 1`, and a security policy's own term counts as much as the
///   caller's;
/// - a disjunction, when *every* branch lands inside the index, which is what
///   makes `WHERE (a = 1 OR a = 2)` usable against `WHERE a > 0`;
/// - a bound against a looser bound in the same direction: `x > 10` implies
///   `x > 3`, `x >= 4` implies `x > 3`, but `x >= 3` does not imply `x > 3`;
/// - an equality or an `IN` against any bound every one of its values meets;
/// - `IS NOT NULL` from any comparison, pattern match or `IN` on the column,
///   because all of them are *unknown* rather than true on a null. This is the
///   commonest partial index there is — `WHERE deleted_at IS NULL` is the
///   other one — so it is worth the special case.
///
/// # What it cannot
///
/// Anything needing two conjuncts at once: `x >= 5 AND x <= 5` does not imply
/// `x = 5` here. Anything arithmetic: nothing knows that `x > 3` on an integer
/// is `x >= 4`. And through `NOT`, only what three-valued logic gives for
/// nothing — that `NOT (x IS NULL)` is `x IS NOT NULL`, and that a negated
/// comparison is still never true of a null.
///
/// Comparisons between literals use [`Value`]'s own order, which is exactly the
/// order [`Expr::evaluate`](crate::Expr::evaluate) compares with. That is what
/// makes the reasoning sound rather than approximately sound: the planner and
/// the evaluator are not two opinions about what `<` means.
#[must_use]
pub fn implies(query: &Expr, index: &Expr) -> bool {
    // Each conjunct of the index's predicate has to be established separately;
    // together they are the whole of it.
    index
        .conjuncts()
        .iter()
        .all(|goal| implies_one(query, goal))
}

/// Whether `query` establishes the single term `goal`.
fn implies_one(query: &Expr, goal: &Expr) -> bool {
    if query == goal || matches!(goal, Expr::True) {
        return true;
    }
    // A disjunctive goal is met by landing inside any one of its branches.
    if let Expr::Or(branches) = goal
        && branches.iter().any(|branch| implies_one(query, branch))
    {
        return true;
    }
    match query {
        // A query that admits nothing is inside every set of rows, vacuously.
        Expr::False => true,
        // `conjuncts` flattens nested `And`s, so this sees every term at once
        // rather than recursing pairwise.
        Expr::And(_) => query
            .conjuncts()
            .iter()
            .any(|conjunct| implies_one(conjunct, goal)),
        // Every branch must land inside, because a row admitted by any one of
        // them is admitted by the whole. An empty `Or` admits nothing, but
        // saying so here would be reasoning about an expression nobody builds.
        Expr::Or(branches) => {
            !branches.is_empty() && branches.iter().all(|branch| implies_one(branch, goal))
        }
        _ => leaf_implies(query, goal),
    }
}

/// One leaf term against one leaf goal.
fn leaf_implies(query: &Expr, goal: &Expr) -> bool {
    // `NOT (x IS NULL)` and `x IS NOT NULL` are the same predicate — `IS NULL`
    // is the one test that is never unknown, so negating it is exact — and both
    // spellings are written. Missing one of them would make the planner's
    // answer depend on how the schema's author phrased it.
    if let Some(column) = not_null_goal(goal) {
        return excludes_nulls(query, column);
    }
    match goal {
        Expr::Compare {
            column,
            op,
            value: goal_value,
        } if !goal_value.is_null() => match query {
            Expr::Compare {
                column: on,
                op: held,
                value,
            } if on == column && !value.is_null() => {
                comparison_implies(*held, value, *op, goal_value)
            }
            // Every value in the set has to meet the goal; one that does not is
            // a row the index would not hold.
            Expr::In { column: on, values } if on == column => {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| !value.is_null() && satisfies(value, *op, goal_value))
            }
            _ => false,
        },
        _ => false,
    }
}

/// The column a goal of "this column has a value" is about, in either spelling.
fn not_null_goal(goal: &Expr) -> Option<Ordinal> {
    match goal {
        Expr::IsNull {
            column,
            negated: true,
        } => Some(*column),
        Expr::Not(inner) => match inner.as_ref() {
            Expr::IsNull {
                column,
                negated: false,
            } => Some(*column),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `x <held> value` being true forces `x <goal> goal_value` to be true.
fn comparison_implies(held: CmpOp, value: &Value, goal: CmpOp, goal_value: &Value) -> bool {
    use CmpOp::{Eq, Ge, Gt, Le, Lt, Ne};
    match held {
        // Pinned to one value: the goal either holds at it or does not.
        Eq => satisfies(value, goal, goal_value),
        // `x <> v` bounds nothing.
        Ne => false,
        Lt | Le | Gt | Ge => match (held, goal) {
            // `x < v` and `v <= g` give `x < v <= g`.
            (Lt, Lt | Le) => value <= goal_value,
            // `x <= v` needs a strictly smaller `v` for a strict goal.
            (Le, Lt) => value < goal_value,
            (Le, Le) => value <= goal_value,
            (Gt, Gt | Ge) => value >= goal_value,
            (Ge, Gt) => value > goal_value,
            (Ge, Ge) => value >= goal_value,
            // A bound establishes `x <> g` when it excludes `g` outright.
            (Lt, Ne) => goal_value >= value,
            (Le, Ne) => goal_value > value,
            (Gt, Ne) => goal_value <= value,
            (Ge, Ne) => goal_value < value,
            _ => false,
        },
    }
}

/// Whether the literal `value` satisfies `<op> goal_value`.
///
/// Written out rather than borrowed from the evaluator, which takes a row: two
/// statements of one rule, the way `oracle.rs` restates the sort comparator.
fn satisfies(value: &Value, op: CmpOp, goal_value: &Value) -> bool {
    use core::cmp::Ordering::{Equal, Greater, Less};
    let ordering = value.cmp(goal_value);
    match op {
        CmpOp::Eq => ordering == Equal,
        CmpOp::Ne => ordering != Equal,
        CmpOp::Lt => ordering == Less,
        CmpOp::Le => matches!(ordering, Less | Equal),
        CmpOp::Gt => ordering == Greater,
        CmpOp::Ge => matches!(ordering, Greater | Equal),
    }
}

/// Whether a row admitted by `query` must have a value in `column`.
///
/// Three-valued logic does the work: a comparison, a pattern match or an `IN`
/// against a null is *unknown*, and unknown does not admit. So any of them
/// being true is a statement that the column holds something — which is exactly
/// what a `WHERE column IS NOT NULL` index holds.
fn excludes_nulls(query: &Expr, column: Ordinal) -> bool {
    match query {
        Expr::IsNull {
            column: on,
            negated: true,
        } => *on == column,
        // `NOT e` is true only where `e` is false, and none of these are ever
        // false on a null — they are unknown, and `NOT unknown` is unknown.
        Expr::Not(inner) => match inner.as_ref() {
            Expr::IsNull {
                column: on,
                negated: false,
            } => *on == column,
            inner => unknown_on_null(inner, column),
        },
        _ => unknown_on_null(query, column),
    }
}

/// Whether `query` is unknown for every row whose `column` is null.
fn unknown_on_null(query: &Expr, column: Ordinal) -> bool {
    match query {
        Expr::Compare {
            column: on, value, ..
        } => *on == column && !value.is_null(),
        // An `IN` is true only of a value equal to one of the candidates, and
        // null is equal to nothing — not even to a null candidate, which makes
        // the test unknown rather than true.
        Expr::In { column: on, .. } => *on == column,
        Expr::Like { column: on, .. } | Expr::Matches { column: on, .. } => *on == column,
        Expr::CompareColumns { left, right, .. } => *left == column || *right == column,
        _ => false,
    }
}

/// Everything matching one index against one query needs.
///
/// A struct rather than nine arguments: the list grew every time the planner
/// learned something new, and a call site of nine positional values is one
/// transposition away from planning a different query than it asked for.
struct MatchContext<'a> {
    table: &'a TableDef,
    /// The whole predicate, security filter included. Needed as a tree rather
    /// than as constraints because that is what a partial index's predicate has
    /// to be implied by — and a policy term is as good an implication as a
    /// caller's own `WHERE`.
    predicate: &'a Expr,
    constraints: &'a [(Ordinal, ColumnConstraints<'a>)],
    needed: &'a Needed,
    stats: &'a TableStats,
    ordered: bool,
    compute: &'a [crate::scalar::Scalar],
    /// Columns the table itself has. Anything at or past this is computed.
    width: usize,
}

fn match_index(cx: &MatchContext<'_>, index: &IndexDef) -> Option<Candidate> {
    // A partial index holds entries only for the rows its predicate admits, so
    // using it for a query that reaches outside them does not return the wrong
    // *columns*, it returns the wrong *rows* — silently, and with no residual
    // able to put them back. This is the gate; see the module docs.
    let partial = match index.predicate() {
        None => None,
        Some(_) => match crate::record::index_predicate(index) {
            Some(predicate) if implies(cx.predicate, predicate) => Some(predicate),
            // Either the query was not shown to land inside the index, or the
            // predicate is not an `Expr` and so nothing can be shown about it
            // at all. Both mean the same thing here: the entries this index is
            // missing are missing whichever it is.
            _ => return None,
        },
    };

    // On a tenant-scoped table the tenant leads every index key, so it has to be
    // matched before the index's own columns.
    let mut key_columns: Vec<(Ordinal, Direction)> = Vec::new();
    if let Some(tenant) = cx.table.tenant_column() {
        key_columns.push((tenant, Direction::Asc));
    }
    let computed = index.expression();
    match computed {
        None => key_columns.extend(index.columns().iter().map(|c| (c.ordinal, c.direction))),
        Some(declared) => {
            // An expression index keys on a value the row does not contain, so
            // the only predicate it can serve is one over that same value —
            // which in this layer means the query computed it and named the
            // ordinal the computed value was appended at. A query that does not
            // compute it has nothing the index's keys could be matched against,
            // so the index is not a candidate rather than a full scan.
            //
            // An index whose expression is not a `Scalar` is in the same
            // position as a partial index whose predicate is not an `Expr`:
            // maintained, and never matched.
            let expression = crate::record::index_expression(index)?;
            let position = cx.compute.iter().position(|scalar| scalar == expression)?;
            key_columns.push((Ordinal(cx.width + position), declared.direction()));
        }
    }

    let base = keys::index_prefix(cx.table, index, None);
    let (range, mut bound_selectivity) =
        match_key(base.clone(), &key_columns, cx.constraints, cx.stats);
    // A partial index is smaller than the table by exactly the share of rows
    // its predicate keeps, and a scan of it can only ever touch what is in it.
    // Independence again — see `stats` — but in the one direction that matters
    // here, since the query implies the predicate and so the two overlap
    // completely rather than by chance.
    if let Some(predicate) = partial {
        bound_selectivity *= cx.stats.predicate_selectivity(predicate);
    }

    // Index entries end with the primary key, so an index scan is ordered by
    // its own columns and then by the key — which is what makes it a total
    // order rather than a partial one. Only worth computing if anything asked
    // for an order.
    let natural_order = if cx.ordered {
        let mut order = key_columns.clone();
        order.extend(cx.table.primary_key().iter().map(|o| (*o, Direction::Asc)));
        order
    } else {
        Vec::new()
    };

    // An expression index used to be excluded here, because the executor
    // evaluated every scalar from the row's own columns and a row rebuilt from
    // an entry has the source column null — so it would have computed
    // `lower(null)` and answered differently from every other access path. The
    // executor now takes that one value out of the entry it is already holding
    // (`exec::QueryCursor`), so the entry answers for it and `covers` treats it
    // like any other value the index holds.
    let covering = covers(cx, index);

    // An `IN` on the first column the equality prefix does not pin splits the
    // one range into several narrow ones. Worth it precisely when the values
    // are spread out, which is the case a single hull range serves worst.
    if let Some((ranges, selectivity)) = in_ranges(&base, &key_columns, cx.constraints, cx.stats) {
        let selectivity = partial.map_or(selectivity, |predicate| {
            selectivity * cx.stats.predicate_selectivity(predicate)
        });
        // One range is an ordinary index scan, and a narrower one than the
        // prefix match above produced — the `IN` pinned a column the equality
        // terms did not.
        let access = match <[KeyRange; 1]>::try_from(ranges) {
            Ok([range]) => Access::IndexScan {
                index: index.id(),
                range,
                covering,
            },
            Err(ranges) => Access::IndexScans {
                index: index.id(),
                ranges,
                covering,
            },
        };
        return Some(Candidate {
            access,
            bound_selectivity: selectivity,
            natural_order,
        });
    }

    Some(Candidate {
        access: Access::IndexScan {
            index: index.id(),
            range,
            covering,
        },
        bound_selectivity,
        natural_order,
    })
}

/// The disjoint ranges an `IN` splits an index scan into, and the share of the
/// table they admit between them.
///
/// `None` when the predicate gives nothing to split on, which leaves the
/// ordinary single-range scan in place.
///
/// The split has to be on the first key column the equality terms do not
/// already pin. An `IN` on a later column does not narrow anything: with
/// `(kind, size)` and only `size IN (1, 9)`, every range would have to span
/// every `kind`, which is the whole index twice over. Reaching those values
/// without the leading column means skipping through the index rather than
/// scanning ranges of it, which is a different access path and not this one.
fn in_ranges<'a>(
    base: &[u8],
    key_columns: &[(Ordinal, Direction)],
    constraints: &[(Ordinal, ColumnConstraints<'a>)],
    stats: &TableStats,
) -> Option<(Vec<KeyRange>, f64)> {
    let position = key_columns.iter().position(|(ordinal, _)| {
        constraints_for(constraints, *ordinal)
            .and_then(|c| c.equals)
            .is_none()
    })?;
    let (ordinal, _) = key_columns.get(position)?;
    let values = constraints_for(constraints, *ordinal)?.any_of?;
    // Before anything is sorted or encoded: a set this large cannot win, and
    // deciding that after sorting a hundred thousand values would be paying the
    // planning cost to discover the plan is not worth planning.
    if values.len() > MAX_INDEX_RANGES {
        return None;
    }

    // Nulls are dropped rather than searched for. `x IN (NULL, 1)` is true only
    // where `x = 1`: a null candidate makes the test *unknown* for every other
    // row, and unknown admits nothing. A range for the null would return rows
    // the residual then throws away.
    let mut distinct: Vec<&'a Value> = values.iter().filter(|value| !value.is_null()).collect();
    // Sorted and deduplicated for the same reason a point-get set is: `IN (1,
    // 1)` is one range, and two copies of a range return every row in it twice.
    distinct.sort();
    distinct.dedup();
    if distinct.is_empty() || distinct.len() > MAX_INDEX_RANGES {
        return None;
    }

    let mut ranges = Vec::with_capacity(distinct.len());
    let mut selectivity = 0.0f64;
    let constraint = constraints_for(constraints, *ordinal)?;
    for value in distinct {
        // The column's *other* bounds still apply, and pinning it as an
        // equality hides them: `match_key` takes a range term only from the
        // first column no equality pins, so `kind IN ('a', 'q') AND kind > 'm'`
        // would otherwise scan a range for `'a'` that the residual is certain
        // to reject in full.
        if !meets_other_bounds(constraint, value) {
            continue;
        }
        // Pin the column and re-derive the bounds exactly as an equality there
        // would have: same prefix, same range term on the column after it, same
        // histogram. Rebuilding that here instead would be a second bound
        // derivation to keep in step with the first.
        let mut pinned: Vec<(Ordinal, ColumnConstraints<'a>)> = constraints.to_vec();
        if let Some((_, constraint)) = pinned.iter_mut().find(|(o, _)| o == ordinal) {
            constraint.equals = Some(value);
            constraint.any_of = None;
        }
        let (range, share) = match_key(base.to_vec(), key_columns, &pinned, stats);
        // A value contradicting another bound — `size IN (1, 9) AND size > 5` —
        // contributes no range rather than a backwards one.
        if range.is_empty() {
            continue;
        }
        ranges.push(range);
        selectivity += share;
    }
    if ranges.is_empty() {
        return None;
    }
    // Ascending *key* order, which for a descending column is descending value
    // order. Sorting the encoded bounds rather than the literals is what makes
    // that fall out rather than needing a case.
    ranges.sort_by(|a, b| start_bytes(a).cmp(&start_bytes(b)));
    Some((ranges, selectivity.clamp(f64::MIN_POSITIVE, 1.0)))
}

/// Whether one value of an `IN` also meets the column's other bounds.
///
/// Only the bounds the planner already understands — the residual still decides
/// on the row, as everywhere else. A `false` here drops a range that would have
/// been scanned for nothing; it can never drop a row, because the value it
/// rejects is one no admitted row holds.
fn meets_other_bounds(constraint: &ColumnConstraints<'_>, value: &Value) -> bool {
    if !constraint
        .ranges
        .iter()
        .all(|(op, bound)| satisfies(value, *op, bound))
    {
        return false;
    }
    match (&constraint.starts_with, value) {
        (Some(Value::Str(prefix)), Value::Str(text)) => text.starts_with(prefix.as_str()),
        // A prefix against a value that is not a string matches nothing, the
        // same way `LIKE` on a non-string is unknown rather than false.
        (Some(_), _) => false,
        (None, _) => true,
    }
}

/// Where a range starts, for ordering ranges against each other. `None` sorts
/// first, which is where an unbounded start belongs.
fn start_bytes(range: &KeyRange) -> Option<&[u8]> {
    match &range.start {
        Bound::Unbounded => None,
        Bound::Included(key) | Bound::Excluded(key) => Some(key.as_slice()),
    }
}
