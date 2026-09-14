//! Showing what the planner decided.
//!
//! A cost-based planner that cannot be interrogated is worse than a predictable
//! one: when a query is slow the first question is always which path it took
//! and what the planner believed about the data. This answers both, including
//! the estimates — an estimate that is wildly wrong is usually the actual bug,
//! and it is invisible without printing it.

use crate::expr::Expr;
use crate::join::{Join, JoinAlgorithm, JoinPlan, JoinType, Side};
use crate::plan::{Access, Plan};
use crate::query::Query;
use crate::store::ScanOrder;
use core::fmt;
use slate_schema::{Ordinal, TableDef};

/// How a query will be run.
#[derive(Debug, Clone)]
pub struct Explanation {
    /// The table read.
    pub table: String,
    /// How rows are reached.
    pub access: AccessSummary,
    /// The predicate evaluated on every candidate row.
    pub residual: String,
    /// Direction the access path is walked in.
    pub order: ScanOrder,
    /// Rows the planner expects.
    pub estimated_rows: f64,
    /// Estimated cost in object-storage round trips.
    pub estimated_cost: f64,
    /// The caller's limit, if any.
    pub limit: Option<usize>,
    /// The caller's offset.
    pub offset: usize,
    /// Whether the executor must materialise and sort the whole result.
    ///
    /// The expensive part is not the comparison but the materialisation: a
    /// sorted plan cannot return its first row until it has found its last.
    pub sorts: bool,
    /// The columns this plan decodes: the projection, plus whatever the
    /// residual predicate reads, since those are decoded anyway.
    ///
    /// Here because the access path alone does not always show the difference
    /// between two plans that matter differently. A grouped read narrows its
    /// projection to the group keys and the aggregates' columns, and on a table
    /// with no usable index that changes nothing about *how* rows are reached —
    /// so `ExplainAggregate` and `ExplainJoin` printed byte-identical plans for
    /// two reads that decode different amounts of every row. This is the field
    /// that tells them apart.
    pub decodes: Vec<Ordinal>,
}

/// The access path, in terms a reader recognises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessSummary {
    /// One row, by primary key.
    PointGet,
    /// A range of the table's own keys.
    TableScan,
    /// A range of an index, followed by a read per row.
    IndexScan {
        /// Index name.
        index: String,
    },
    /// A range of an index, answered without reading any row.
    IndexOnlyScan {
        /// Index name.
        index: String,
    },
    /// Several disjoint ranges of one index, followed by a read per row.
    IndexScans {
        /// Index name.
        index: String,
        /// How many ranges. Each is an iterator to open, so the count is what
        /// makes this plan cost more than a single-range scan of the same
        /// index — worth printing rather than summarising away.
        ranges: usize,
    },
    /// Several disjoint ranges of one index, answered without reading any row.
    IndexOnlyScans {
        /// Index name.
        index: String,
        /// How many ranges.
        ranges: usize,
    },
    /// Several rows by primary key, read together.
    PointGets {
        /// How many keys.
        keys: usize,
    },
    /// Provably empty.
    Nothing,
}

impl fmt::Display for AccessSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PointGet => f.write_str("Point Get"),
            Self::TableScan => f.write_str("Table Scan"),
            Self::IndexScan { index } => write!(f, "Index Scan using {index}"),
            Self::IndexOnlyScan { index } => write!(f, "Index Only Scan using {index}"),
            Self::IndexScans { index, ranges } => {
                write!(f, "Index Scan using {index} ({ranges} ranges)")
            }
            Self::IndexOnlyScans { index, ranges } => {
                write!(f, "Index Only Scan using {index} ({ranges} ranges)")
            }
            Self::PointGets { keys } => write!(f, "Point Gets ({keys} keys)"),
            Self::Nothing => f.write_str("Result (nothing)"),
        }
    }
}

impl Explanation {
    /// Describe `plan`.
    #[must_use]
    pub fn of(table: &TableDef, plan: &Plan, query: &Query) -> Self {
        let access = match &plan.access {
            Access::PointGet { .. } => AccessSummary::PointGet,
            Access::TableScan { .. } => AccessSummary::TableScan,
            Access::IndexScan {
                index, covering, ..
            } => {
                let name = table
                    .index(*index)
                    .map_or_else(|| format!("{index:?}"), |i| i.name().to_owned());
                if *covering {
                    AccessSummary::IndexOnlyScan { index: name }
                } else {
                    AccessSummary::IndexScan { index: name }
                }
            }
            Access::IndexScans {
                index,
                ranges,
                covering,
            } => {
                let name = table
                    .index(*index)
                    .map_or_else(|| format!("{index:?}"), |i| i.name().to_owned());
                let ranges = ranges.len();
                if *covering {
                    AccessSummary::IndexOnlyScans {
                        index: name,
                        ranges,
                    }
                } else {
                    AccessSummary::IndexScans {
                        index: name,
                        ranges,
                    }
                }
            }
            Access::PointGets { keys } => AccessSummary::PointGets { keys: keys.len() },
            Access::Nothing => AccessSummary::Nothing,
        };

        Self {
            table: table.name().to_owned(),
            access,
            residual: format!("{:?}", plan.residual),
            order: plan.order,
            estimated_rows: plan.estimated_rows,
            estimated_cost: plan.estimated_cost,
            limit: query.limit,
            offset: query.offset,
            sorts: plan.sort.is_some(),
            decodes: plan.output_columns.ordinals(),
        }
    }

    /// Whether the plan avoids reading rows entirely.
    #[must_use]
    pub const fn is_index_only(&self) -> bool {
        matches!(self.access, AccessSummary::IndexOnlyScan { .. })
    }

    /// Whether the plan streams, rather than buffering everything first.
    #[must_use]
    pub const fn streams(&self) -> bool {
        !self.sorts
    }
}

impl fmt::Display for Explanation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.sorts {
            f.write_str("Sort -> ")?;
        }
        write!(
            f,
            "{} on {}  (rows={:.0} cost={:.2}",
            self.access, self.table, self.estimated_rows, self.estimated_cost
        )?;
        if self.order == ScanOrder::Descending {
            f.write_str(" backwards")?;
        }
        if let Some(limit) = self.limit {
            write!(f, " limit={limit}")?;
        }
        if self.offset > 0 {
            write!(f, " offset={}", self.offset)?;
        }
        // Printed because it is often the only visible difference between a
        // grouped read's plan and the plan of the read it groups: narrowing the
        // projection need not change the access path.
        write!(
            f,
            " decodes=[{}]",
            self.decodes
                .iter()
                .map(|ordinal| ordinal.0.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )?;
        f.write_str(")")
    }
}

/// How a join will be run: the algorithm, and each side's own plan.
///
/// Both sides are shown because a join's cost is almost entirely its sides'.
/// A join that looks expensive is usually a side that is, and printing only
/// the algorithm would hide it.
#[derive(Debug, Clone)]
pub struct JoinExplanation {
    /// How the sides are combined.
    pub algorithm: JoinAlgorithm,
    /// Which rows survive.
    pub join_type: JoinType,
    /// How the left side is read.
    pub left: Explanation,
    /// How the right side is read. For a nested loop this is the shape of one
    /// probe, not a plan run once.
    pub right: Explanation,
    /// Joined rows the planner expects.
    pub estimated_rows: f64,
    /// Estimated cost in object-storage round trips.
    pub estimated_cost: f64,
    /// The caller's limit on the joined result, if any.
    pub limit: Option<usize>,
    /// The caller's offset on the joined result.
    pub offset: usize,
    /// The condition over the joined row, if there is one.
    pub having: Option<String>,
}

impl JoinExplanation {
    /// Describe `plan`.
    #[must_use]
    pub fn of(left: &TableDef, right: &TableDef, plan: &JoinPlan, join: &Join) -> Self {
        Self {
            algorithm: plan.algorithm,
            join_type: join.join_type,
            left: Explanation::of(left, &plan.left, &join.left),
            right: Explanation::of(right, &plan.right, &join.right),
            estimated_rows: plan.estimated_rows,
            estimated_cost: plan.estimated_cost,
            limit: join.limit,
            offset: join.offset,
            having: match &join.having {
                Expr::True => None,
                other => Some(format!("{other:?}")),
            },
        }
    }

    /// Whether the planner chose to probe the inner side per outer row.
    #[must_use]
    pub const fn is_nested_loop(&self) -> bool {
        matches!(self.algorithm, JoinAlgorithm::NestedLoop)
    }

    /// The side read into memory, for a hash join.
    #[must_use]
    pub const fn build_side(&self) -> Option<Side> {
        match self.algorithm {
            JoinAlgorithm::Hash { build } => Some(build),
            JoinAlgorithm::NestedLoop => None,
        }
    }
}

impl fmt::Display for JoinExplanation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.join_type {
            JoinType::Inner => "Inner",
            JoinType::Left => "Left",
            JoinType::Right => "Right",
            JoinType::Full => "Full",
        };
        match self.algorithm {
            JoinAlgorithm::Hash { build } => {
                let side = match build {
                    Side::Left => "left",
                    Side::Right => "right",
                };
                write!(f, "Hash {kind} Join (build {side})")?;
            }
            JoinAlgorithm::NestedLoop => write!(f, "Nested Loop {kind} Join")?,
        }
        write!(
            f,
            "  (rows={:.0} cost={:.2}",
            self.estimated_rows, self.estimated_cost
        )?;
        if let Some(limit) = self.limit {
            write!(f, " limit={limit}")?;
        }
        if self.offset > 0 {
            write!(f, " offset={}", self.offset)?;
        }
        writeln!(f, ")")?;
        if let Some(having) = &self.having {
            writeln!(f, "  on {having}")?;
        }
        writeln!(f, "  -> {}", self.left)?;
        write!(f, "  -> {}", self.right)
    }
}
