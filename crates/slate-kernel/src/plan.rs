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

        let mut cost = SCAN_OPEN_COST + touched * SCAN_ROW_COST;
        if let Access::IndexScan { covering, .. } = self.access
            && !covering
        {
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
    let mut output_columns = predicate_columns.clone();
    match projection.columns() {
        None => output_columns = ColumnSet::all(table.columns().len()),
        Some(columns) => {
            for column in columns {
                if column.0 < width {
                    output_columns.insert(*column);
                } else if let Some(scalar) = compute.get(column.0 - width) {
                    for input in scalar.columns() {
                        output_columns.insert(input);
                    }
                }
            }
            // A column the sort orders by has to be decoded even when the
            // caller did not ask to see it. Leaving it out does not fail: it
            // reads back as null, every row compares equal, and the result
            // comes out in whatever order the scan happened to produce. A
            // wrong order that looks like an order is worse than an error.
            for key in sort {
                if key.column.0 < width {
                    output_columns.insert(key.column);
                } else if let Some(scalar) = compute.get(key.column.0 - width) {
                    for input in scalar.columns() {
                        output_columns.insert(input);
                    }
                }
            }
            // Every computed value is evaluated on every row whether or not
            // anything references it, so its inputs are always needed. The
            // same hole the sort column fell through: an undecoded input reads
            // as null and the computed value is quietly wrong rather than
            // missing.
            for scalar in compute {
                for input in scalar.columns() {
                    output_columns.insert(input);
                }
            }
        }
    }

    if unsatisfiable {
        return Plan {
            access: Access::Nothing,
            residual: predicate,
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
    let needed = match projection.columns() {
        None => Needed::All,
        Some(columns) => {
            let mut set: BTreeSet<Ordinal> = predicate
                .columns()
                .into_iter()
                .flat_map(|c| expanded_inputs(c, width, compute))
                .collect();
            set.extend(columns.iter().copied().filter(|c| c.0 < width));
            // An index cannot answer a query on its own unless it holds what
            // the computed values read, for the same reason.
            for scalar in compute {
                set.extend(scalar.columns());
            }
            // Same reason as `output_columns`: an index that does not hold the
            // sort column cannot answer the query on its own, however well it
            // covers the projection.
            set.extend(sort.iter().map(|key| key.column));
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
    for index in table.indexes() {
        if matches!(hint, Some(AccessHint::TableScan))
            || matches!(hint, Some(AccessHint::Index(wanted)) if wanted != index.id())
        {
            continue;
        }
        considered += 1;
        consider(match_index(
            table,
            index,
            &constraints,
            &needed,
            stats,
            ordered,
        ));
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
    let access = match &access {
        Access::TableScan { range } | Access::IndexScan { range, .. } if range.is_empty() => {
            Access::Nothing
        }
        _ => access,
    };

    Plan {
        access,
        residual: predicate,
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

/// Whether `index` holds every column in `needed`.
///
/// An index entry carries its own columns and the primary key, so those are
/// what it can answer from.
fn covers(table: &TableDef, index: &IndexDef, needed: &Needed) -> bool {
    // Scanning two short slices beats building a set per index per query; both
    // are a handful of entries and this runs on every plan.
    let holds = |ordinal: Ordinal| {
        index.columns().iter().any(|c| c.ordinal == ordinal)
            || table.primary_key().contains(&ordinal)
    };
    match needed {
        Needed::All => (0..table.columns().len()).map(Ordinal).all(holds),
        Needed::Some(columns) => columns.iter().copied().all(holds),
    }
}

fn match_index(
    table: &TableDef,
    index: &IndexDef,
    constraints: &[(Ordinal, ColumnConstraints<'_>)],
    needed: &Needed,
    stats: &TableStats,
    ordered: bool,
) -> Candidate {
    // On a tenant-scoped table the tenant leads every index key, so it has to be
    // matched before the index's own columns.
    let mut key_columns: Vec<(Ordinal, Direction)> = Vec::new();
    if let Some(tenant) = table.tenant_column() {
        key_columns.push((tenant, Direction::Asc));
    }
    key_columns.extend(index.columns().iter().map(|c| (c.ordinal, c.direction)));

    let base = keys::index_prefix(table, index, None);
    let (range, bound_selectivity) = match_key(base, &key_columns, constraints, stats);

    // Index entries end with the primary key, so an index scan is ordered by
    // its own columns and then by the key — which is what makes it a total
    // order rather than a partial one. Only worth computing if anything asked
    // for an order.
    let natural_order = if ordered {
        let mut order = key_columns.clone();
        order.extend(table.primary_key().iter().map(|o| (*o, Direction::Asc)));
        order
    } else {
        Vec::new()
    };

    Candidate {
        access: Access::IndexScan {
            index: index.id(),
            range,
            covering: covers(table, index, needed),
        },
        bound_selectivity,
        natural_order,
    }
}
