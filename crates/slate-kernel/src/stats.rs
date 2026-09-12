//! Table statistics, and the cost model they feed.
//!
//! Without these the planner picks whichever index matches the most equality
//! terms, which on the benchmark corpus chose a plan 30× slower than ignoring
//! the index entirely. Structure alone cannot tell you that: an index is worth
//! using only when it selects few enough rows to be worth a point read each,
//! and "few enough" is a fact about the data.
//!
//! # The cost model
//!
//! Three constants, in units of one object-storage round trip:
//!
//! | | cost | why |
//! |---|---|---|
//! | open a scan | 1.0 | one round trip |
//! | one row from a scan | 0.01 | a block fetch amortised over its rows, plus decode |
//! | one point read | 1.0 | a round trip that amortises over nothing |
//!
//! The ratio is what matters, and it has a blunt consequence: a
//! non-covering index scan only beats a table scan when it selects under
//! roughly 1% of the rows the scan would touch. That is not a quirk of the
//! numbers, it is what storage where every lookup is a network round trip
//! actually implies — and it is why covering an index matters so much more here
//! than it would on local disk.
//!
//! # Estimates
//!
//! Selectivities multiply, which assumes the columns are independent. They
//! frequently are not, and this is the standard place for an optimiser to be
//! wrong. It is a deliberate trade: correlated-column statistics are a large
//! subsystem, and the residual predicate means a bad estimate costs time rather
//! than correctness.

use crate::expr::{CmpOp, Expr};
use slate_schema::{Ordinal, TableDef, TableId};
use std::collections::BTreeMap;

/// Cost of opening a scan, in round trips.
pub const SCAN_OPEN_COST: f64 = 1.0;
/// Cost of one row pulled from an open scan.
pub const SCAN_ROW_COST: f64 = 0.01;
/// Cost of one point read.
pub const POINT_READ_COST: f64 = 1.0;
/// Cost of one comparison level when sorting a row: CPU only, no I/O, so
/// several orders of magnitude below a round trip.
pub const SORT_ROW_COST: f64 = 0.000_02;

/// What is known about one column's contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnStats {
    /// Distinct values. One means every row shares a value; `row_count` means
    /// the column is unique.
    pub distinct: u64,
    /// Fraction of rows where the column is null, in `0.0..=1.0`.
    pub null_fraction: f64,
}

impl Default for ColumnStats {
    fn default() -> Self {
        // Stands in for an un-analysed column: selective enough to be worth
        // indexing, not so selective that the planner bets everything on it.
        Self {
            distinct: 100,
            null_fraction: 0.1,
        }
    }
}

/// What is known about one table's contents.
#[derive(Debug, Clone, PartialEq)]
pub struct TableStats {
    /// Rows in the table.
    pub row_count: u64,
    columns: BTreeMap<Ordinal, ColumnStats>,
}

impl Default for TableStats {
    fn default() -> Self {
        Self::assumed()
    }
}

impl TableStats {
    /// The estimates used before anything has been analysed.
    ///
    /// A thousand rows, a hundred distinct values per column. Wrong for any
    /// particular table, and better than assuming the structure of a predicate
    /// tells you how many rows it selects.
    #[must_use]
    pub const fn assumed() -> Self {
        Self {
            row_count: 1_000,
            columns: BTreeMap::new(),
        }
    }

    /// Statistics for a table of a known size, with default column estimates.
    #[must_use]
    pub const fn with_row_count(row_count: u64) -> Self {
        Self {
            row_count,
            columns: BTreeMap::new(),
        }
    }

    /// Record what is known about a column.
    #[must_use]
    pub fn with_column(mut self, ordinal: Ordinal, stats: ColumnStats) -> Self {
        self.columns.insert(ordinal, stats);
        self
    }

    /// What is known about `ordinal`, or the default.
    #[must_use]
    pub fn column(&self, ordinal: Ordinal) -> ColumnStats {
        self.columns.get(&ordinal).copied().unwrap_or_default()
    }

    /// The fraction of rows an equality on `ordinal` is expected to keep.
    #[must_use]
    pub fn equality_selectivity(&self, ordinal: Ordinal) -> f64 {
        let stats = self.column(ordinal);
        let distinct = stats.distinct.max(1) as f64;
        // Nulls never match an equality, so they are not among the candidates.
        ((1.0 - stats.null_fraction) / distinct).clamp(f64::MIN_POSITIVE, 1.0)
    }

    /// The fraction a range comparison is expected to keep.
    ///
    /// A fixed guess. Estimating it properly needs a histogram, which is the
    /// next thing to add here and is not needed to stop the planner making the
    /// mistake this module exists to prevent.
    #[must_use]
    pub fn range_selectivity(&self, ordinal: Ordinal, one_sided: bool) -> f64 {
        let stats = self.column(ordinal);
        let base = if one_sided { 0.33 } else { 0.1 };
        (base * (1.0 - stats.null_fraction)).clamp(f64::MIN_POSITIVE, 1.0)
    }

    /// The fraction of rows a whole predicate is expected to keep.
    #[must_use]
    pub fn predicate_selectivity(&self, predicate: &Expr) -> f64 {
        match predicate {
            Expr::True => 1.0,
            Expr::False => 0.0,
            Expr::Compare { column, op, value } => {
                if value.is_null() {
                    // Comparing with null is unknown for every row.
                    return 0.0;
                }
                match op {
                    CmpOp::Eq => self.equality_selectivity(*column),
                    CmpOp::Ne => 1.0 - self.equality_selectivity(*column),
                    CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                        self.range_selectivity(*column, true)
                    }
                }
            }
            Expr::IsNull { column, negated } => {
                let fraction = self.column(*column).null_fraction;
                if *negated { 1.0 - fraction } else { fraction }
            }
            Expr::In { column, values } => {
                let each = self.equality_selectivity(*column);
                (each * values.len() as f64).clamp(0.0, 1.0)
            }
            // Independence: the standard assumption, and the standard way to be
            // wrong. See the module docs.
            Expr::And(parts) => parts
                .iter()
                .map(|p| self.predicate_selectivity(p))
                .product::<f64>()
                .clamp(0.0, 1.0),
            Expr::Or(parts) => {
                let none_match: f64 = parts
                    .iter()
                    .map(|p| 1.0 - self.predicate_selectivity(p))
                    .product();
                (1.0 - none_match).clamp(0.0, 1.0)
            }
            Expr::Not(inner) => (1.0 - self.predicate_selectivity(inner)).clamp(0.0, 1.0),
        }
    }
}

/// Statistics for every table a store serves.
#[derive(Debug, Clone, Default)]
pub struct Statistics {
    tables: BTreeMap<TableId, TableStats>,
}

impl Statistics {
    /// Empty: every table falls back to [`TableStats::assumed`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
        }
    }

    /// Record statistics for a table.
    pub fn set(&mut self, table: TableId, stats: TableStats) {
        self.tables.insert(table, stats);
    }

    /// Record statistics for a table, chaining.
    #[must_use]
    pub fn with(mut self, table: TableId, stats: TableStats) -> Self {
        self.set(table, stats);
        self
    }

    /// What is known about `table`, or the defaults.
    #[must_use]
    pub fn table(&self, table: &TableDef) -> TableStats {
        self.tables
            .get(&table.id())
            .cloned()
            .unwrap_or_else(TableStats::assumed)
    }

    /// Whether anything has been recorded for `table`.
    #[must_use]
    pub fn has(&self, table: TableId) -> bool {
        self.tables.contains_key(&table)
    }
}
