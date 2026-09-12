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

use crate::query::SortKey;
use crate::stats::{POINT_READ_COST, SCAN_OPEN_COST, SCAN_ROW_COST, SORT_ROW_COST, TableStats};
use crate::store::{KeyRange, ScanOrder};
use crate::{expr::Expr, keys};
use core::ops::Bound;
use slate_schema::{IndexDef, IndexId, Ordinal, TableDef};
use slate_tuple::{Direction, Value, encode_value_into, prefix_successor};
use std::collections::BTreeSet;

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
    /// The predicate cannot be satisfied; read nothing.
    Nothing,
}

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

/// A chosen access path plus the filter still to apply.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// How to reach candidate rows.
    pub access: Access,
    /// The predicate to evaluate on every candidate. See the module docs: this
    /// is the authority, the bounds are only a shortcut.
    pub residual: Expr,
    /// Direction to walk the access path in.
    pub order: ScanOrder,
    /// Sorting the executor must do, because no access path produced the
    /// requested order. `None` means the rows arrive already ordered.
    pub sort: Option<Vec<SortKey>>,
    /// Rows the planner expects this to return.
    pub estimated_rows: f64,
    /// Estimated cost, in object-storage round trips. See [`crate::stats`].
    pub estimated_cost: f64,
}

/// What a predicate says about one column.
#[derive(Debug, Default, Clone)]
struct ColumnConstraints {
    /// A value the column must equal, including `IS NULL` as "equals null".
    equals: Option<Value>,
    /// Range comparisons on the column.
    ranges: Vec<(CmpOp, Value)>,
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
            // so this is per row touched, not per row returned.
            cost += touched * POINT_READ_COST;
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
    plan_full(table, predicate, order, projection, stats, limit, &[])
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
    predicate: &Expr,
    order: ScanOrder,
    projection: &Projection,
    stats: &TableStats,
    limit: Option<usize>,
    sort: &[SortKey],
) -> Plan {
    let conjuncts = predicate.conjuncts();

    // A comparison against a null literal is Unknown for every row, so the
    // conjunction can never be true and there is nothing to read.
    let unsatisfiable = conjuncts.iter().any(|c| {
        matches!(c, Expr::Compare { value, .. } if value.is_null()) || matches!(c, Expr::False)
    });
    if unsatisfiable {
        return Plan {
            access: Access::Nothing,
            residual: predicate.clone(),
            order,
            sort: None,
            estimated_rows: 0.0,
            estimated_cost: 0.0,
        };
    }

    let constraints = collect_constraints(&conjuncts);

    let total_selectivity = stats.predicate_selectivity(predicate);

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
    let mut needed: BTreeSet<Ordinal> = predicate.columns();
    match projection.columns() {
        None => needed.extend((0..table.columns().len()).map(Ordinal)),
        Some(columns) => needed.extend(columns.iter().copied()),
    }

    consider(match_primary_key(table, &constraints, stats));
    for index in table.indexes() {
        consider(match_index(table, index, &constraints, &needed, stats));
    }

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

    Plan {
        access,
        residual: predicate.clone(),
        order,
        sort: must_sort.then(|| sort.to_vec()),
        estimated_rows,
        estimated_cost,
    }
}

fn collect_constraints(conjuncts: &[&Expr]) -> Vec<(Ordinal, ColumnConstraints)> {
    let mut out: Vec<(Ordinal, ColumnConstraints)> = Vec::new();
    let entry = |ordinal: Ordinal, out: &mut Vec<(Ordinal, ColumnConstraints)>| -> usize {
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
                        CmpOp::Eq => c.equals = Some(value.clone()),
                        CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                            c.ranges.push((*op, value.clone()));
                        }
                        // `<>` selects everything but one point, which is not a
                        // range; it stays a residual filter.
                        CmpOp::Ne => {}
                    }
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
                    c.equals = Some(Value::Null);
                }
            }
            _ => {}
        }
    }
    out
}

fn constraints_for(
    constraints: &[(Ordinal, ColumnConstraints)],
    ordinal: Ordinal,
) -> Option<&ColumnConstraints> {
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
    constraints: &[(Ordinal, ColumnConstraints)],
    stats: &TableStats,
) -> (KeyRange, f64) {
    let mut prefix = base;
    let mut equality_columns = 0;
    let mut selectivity = 1.0f64;

    for (ordinal, direction) in key_columns {
        let Some(value) = constraints_for(constraints, *ordinal).and_then(|c| c.equals.clone())
        else {
            break;
        };
        selectivity *= if value.is_null() {
            stats.column(*ordinal).null_fraction.max(f64::MIN_POSITIVE)
        } else {
            stats.equality_selectivity(*ordinal)
        };
        encode_value_into(&mut prefix, &value, *direction);
        equality_columns += 1;
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
                selectivity *= stats.range_selectivity(*ordinal, ranges.len() == 1);
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
    constraints: &[(Ordinal, ColumnConstraints)],
    stats: &TableStats,
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
        .map(|(ordinal, _)| constraints_for(constraints, *ordinal).and_then(|c| c.equals.clone()))
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
        natural_order: key_columns,
    }
}

/// Whether `index` holds every column in `needed`.
///
/// An index entry carries its own columns and the primary key, so those are
/// what it can answer from.
fn covers(table: &TableDef, index: &IndexDef, needed: &BTreeSet<Ordinal>) -> bool {
    let available: BTreeSet<Ordinal> = index
        .columns()
        .iter()
        .map(|c| c.ordinal)
        .chain(table.primary_key().iter().copied())
        .collect();
    needed.is_subset(&available)
}

fn match_index(
    table: &TableDef,
    index: &IndexDef,
    constraints: &[(Ordinal, ColumnConstraints)],
    needed: &BTreeSet<Ordinal>,
    stats: &TableStats,
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
    // order rather than a partial one.
    let mut natural_order = key_columns.clone();
    natural_order.extend(table.primary_key().iter().map(|o| (*o, Direction::Asc)));

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
