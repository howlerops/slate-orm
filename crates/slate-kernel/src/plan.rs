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

use crate::store::{KeyRange, ScanOrder};
use crate::{expr::Expr, keys};
use core::ops::Bound;
use slate_schema::{IndexDef, IndexId, Ordinal, TableDef};
use slate_tuple::{Direction, Value, encode_value_into, prefix_successor};

use crate::expr::CmpOp;

/// How the executor will reach the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
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
    },
    /// The predicate cannot be satisfied; read nothing.
    Nothing,
}

/// A chosen access path plus the filter still to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// How to reach candidate rows.
    pub access: Access,
    /// The predicate to evaluate on every candidate. See the module docs: this
    /// is the authority, the bounds are only a shortcut.
    pub residual: Expr,
    /// Direction to walk the access path in.
    pub order: ScanOrder,
}

/// What a predicate says about one column.
#[derive(Debug, Default, Clone)]
struct ColumnConstraints {
    /// A value the column must equal, including `IS NULL` as "equals null".
    equals: Option<Value>,
    /// Range comparisons on the column.
    ranges: Vec<(CmpOp, Value)>,
}

/// A candidate access path and how much of the predicate it absorbs.
struct Candidate {
    access: Access,
    equality_columns: usize,
    has_range: bool,
    is_table_scan: bool,
}

impl Candidate {
    /// Higher is better. Equality columns dominate: each one multiplies the
    /// selectivity, whereas a range only trims the ends.
    fn score(&self) -> usize {
        self.equality_columns * 2 + usize::from(self.has_range)
    }
}

/// Choose an access path for `predicate` over `table`.
///
/// `order` is the direction to walk the chosen path; a plan does not sort, so a
/// caller that needs a specific row order should ask for an index that stores
/// it.
#[must_use]
pub fn plan(table: &TableDef, predicate: &Expr, order: ScanOrder) -> Plan {
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
        };
    }

    let constraints = collect_constraints(&conjuncts);

    let mut best: Option<Candidate> = None;
    let mut consider = |candidate: Candidate| {
        let better = match &best {
            None => true,
            Some(current) => match candidate.score().cmp(&current.score()) {
                core::cmp::Ordering::Greater => true,
                // A table scan reaches the row directly; an index scan pays a
                // point lookup per row. Break ties in the table's favour.
                core::cmp::Ordering::Equal => candidate.is_table_scan && !current.is_table_scan,
                core::cmp::Ordering::Less => false,
            },
        };
        if better {
            best = Some(candidate);
        }
    };

    consider(match_primary_key(table, &constraints));
    for index in table.indexes() {
        consider(match_index(table, index, &constraints));
    }

    let access = best.map_or_else(
        || Access::TableScan {
            range: KeyRange::prefix(&keys::table_prefix(table)),
        },
        |c| c.access,
    );

    Plan {
        access,
        residual: predicate.clone(),
        order,
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
) -> (KeyRange, usize, bool) {
    let mut prefix = base;
    let mut equality_columns = 0;

    for (ordinal, direction) in key_columns {
        let Some(value) = constraints_for(constraints, *ordinal).and_then(|c| c.equals.clone())
        else {
            break;
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
            (result, equality_columns, true)
        }
        _ => (prefix_range, equality_columns, false),
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

fn match_primary_key(table: &TableDef, constraints: &[(Ordinal, ColumnConstraints)]) -> Candidate {
    let key_columns: Vec<(Ordinal, Direction)> = table
        .primary_key()
        .iter()
        .map(|o| (*o, Direction::Asc))
        .collect();
    let (range, equality_columns, has_range) =
        match_key(keys::table_prefix(table), &key_columns, constraints);
    Candidate {
        access: Access::TableScan { range },
        equality_columns,
        has_range,
        is_table_scan: true,
    }
}

fn match_index(
    table: &TableDef,
    index: &IndexDef,
    constraints: &[(Ordinal, ColumnConstraints)],
) -> Candidate {
    // On a tenant-scoped table the tenant leads every index key, so it has to be
    // matched before the index's own columns.
    let mut key_columns: Vec<(Ordinal, Direction)> = Vec::new();
    if let Some(tenant) = table.tenant_column() {
        key_columns.push((tenant, Direction::Asc));
    }
    key_columns.extend(index.columns().iter().map(|c| (c.ordinal, c.direction)));

    let base = keys::index_prefix(table, index, None);
    let (range, equality_columns, has_range) = match_key(base, &key_columns, constraints);

    Candidate {
        access: Access::IndexScan {
            index: index.id(),
            range,
        },
        equality_columns,
        has_range,
        is_table_scan: false,
    }
}
