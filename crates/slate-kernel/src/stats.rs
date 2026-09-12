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
//! | `n` pipelined reads | `ceil(n/16)` | issued together, they land together |
//!
//! `SCAN_ROW_COST` is a block fetch amortised over its rows: 0.01 says a block
//! holds about a hundred. That belief has to be shared with anything measuring
//! against the model — [`crate::latency::LatencyProfile`] states the same
//! number, and when the two disagreed the planner looked wrong where it was
//! not.
//!
//! The ratio decides when an index is worth using: a non-covering index scan
//! beats a table scan while it selects under roughly 6% of the rows the scan
//! would touch. Overlapping the row lookups is what makes that 6% rather than
//! 1% — see [`pipelined_read_cost`].
//!
//! What does not change is that covering an index matters far more here than
//! on local disk. Overlapping a round trip makes it cheaper; not making it at
//! all is still free.
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
use slate_tuple::Value;
use std::collections::BTreeMap;

/// Cost of opening a scan, in round trips.
pub const SCAN_OPEN_COST: f64 = 1.0;
/// Cost of one row pulled from an open scan.
pub const SCAN_ROW_COST: f64 = 0.01;
/// Cost of one point read, issued on its own and waited for.
pub const POINT_READ_COST: f64 = 1.0;

/// What `n` point reads cost when issued `depth` at a time.
///
/// An index scan does not wait for each row lookup in turn — it keeps
/// [`crate::exec::DEFAULT_PREFETCH`] of them in flight, and a nested-loop join
/// does the same with its probes. Charging each one a full round trip makes
/// the model prefer a table scan where an index scan is measurably faster: on
/// the benchmark corpus it picked a 22 ms plan over a 17 ms one.
///
/// The shape is waves, not a discount. Reads issued together land together, so
/// `n` of them at depth `d` cost `ceil(n / d)` round trips — which is why a
/// small limit does not get the full benefit: ten reads and one read both cost
/// one wave, and one wave is a whole round trip that nothing amortises.
///
/// Measured against the latency fixture, where any number of concurrent reads
/// complete in the time of one: 100 reads at depth 16 came to 7 waves and 500
/// came to 32, both matching to within the timer's resolution.
#[must_use]
pub fn pipelined_read_cost(reads: f64, depth: usize) -> f64 {
    if reads <= 0.0 {
        return 0.0;
    }
    let depth = depth.max(1) as f64;
    (reads / depth).ceil() * POINT_READ_COST
}
/// How much of a table a comparison between two of its columns is expected to
/// keep.
///
/// A third, the same as a one-sided range against a literal. Both are guesses;
/// this one cannot be improved by a histogram, because the answer depends on
/// how the two columns vary together and nothing here records that.
pub const COLUMN_RANGE_SELECTIVITY: f64 = 0.33;

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

/// Buckets holding roughly equal numbers of rows, describing how one column's
/// values are spread.
///
/// Without one, a range predicate gets a fixed guess and the planner cannot
/// tell `at < 10` from `at < 500` — on the benchmark corpus that meant
/// scanning 2500 rows in 58 ms to return the ten an index finds in 4.5 ms.
///
/// Equi-depth rather than equi-width: buckets of equal *population*, so a
/// column with a long tail spends its resolution where the rows are. The
/// bounds are quantiles of a sample, so bucket `i` covers roughly
/// `1/buckets` of the table whatever the shape of the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    /// Bucket boundaries, sorted. `buckets + 1` of them: bucket `i` holds
    /// values in `bounds[i] ..= bounds[i + 1]`.
    bounds: Vec<Value>,
}

/// Buckets a histogram is built with.
///
/// Sixty-four puts the resolution at about 1.5% of the table, which is fine
/// against a crossover that sits near 6% and an estimate that is a sample
/// anyway.
pub const HISTOGRAM_BUCKETS: usize = 64;

/// Values sampled per column when building a histogram.
///
/// Bounded because `analyze` reads the whole table and cannot hold all of it.
pub const HISTOGRAM_SAMPLE: usize = 10_000;

impl Histogram {
    /// Build from observed values, which need not be sorted.
    ///
    /// `None` when there is too little to describe: one distinct value is not
    /// a distribution, and a histogram claiming otherwise would be worse than
    /// the fixed guess it replaces.
    #[must_use]
    pub fn from_values(mut values: Vec<Value>) -> Option<Self> {
        values.retain(|v| !v.is_null());
        if values.len() < HISTOGRAM_BUCKETS {
            return None;
        }
        values.sort();
        if values.first() == values.last() {
            return None;
        }
        let mut bounds = Vec::with_capacity(HISTOGRAM_BUCKETS + 1);
        for i in 0..=HISTOGRAM_BUCKETS {
            // Quantile i/buckets, clamped so the last index is in range.
            let at = (i * (values.len() - 1)) / HISTOGRAM_BUCKETS;
            if let Some(value) = values.get(at) {
                bounds.push(value.clone());
            }
        }
        (bounds.len() > 1).then_some(Self { bounds })
    }

    /// The fraction of rows below `value`, in `0.0..=1.0`.
    ///
    /// Resolved to the bucket and no further. Interpolating inside a bucket
    /// would need arithmetic on [`Value`], which is a closed type holding
    /// strings and uuids as well as numbers; the midpoint is honest about what
    /// the histogram actually knows.
    #[must_use]
    pub fn fraction_below(&self, value: &Value) -> f64 {
        let buckets = self.bounds.len().saturating_sub(1);
        if buckets == 0 {
            return 0.5;
        }
        match self.bounds.binary_search(value) {
            // Exactly on a boundary: everything before that bucket.
            Ok(index) => (index as f64 / buckets as f64).clamp(0.0, 1.0),
            Err(0) => 0.0,
            Err(index) if index > buckets => 1.0,
            // Inside bucket `index - 1`; call it half way through.
            Err(index) => ((index as f64 - 0.5) / buckets as f64).clamp(0.0, 1.0),
        }
    }

    /// The bucket boundaries.
    #[must_use]
    pub fn bounds(&self) -> &[Value] {
        &self.bounds
    }
}

/// What is known about one table's contents.
#[derive(Debug, Clone, PartialEq)]
pub struct TableStats {
    /// Rows in the table.
    pub row_count: u64,
    columns: BTreeMap<Ordinal, ColumnStats>,
    /// How each column's values are spread, where that has been measured.
    ///
    /// Held apart from [`ColumnStats`] so that stays small and `Copy`: a
    /// histogram is a hundred values and is read on far fewer paths.
    histograms: BTreeMap<Ordinal, Histogram>,
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
            histograms: BTreeMap::new(),
        }
    }

    /// Statistics for a table of a known size, with default column estimates.
    #[must_use]
    pub const fn with_row_count(row_count: u64) -> Self {
        Self {
            row_count,
            columns: BTreeMap::new(),
            histograms: BTreeMap::new(),
        }
    }

    /// Record what is known about a column.
    #[must_use]
    pub fn with_column(mut self, ordinal: Ordinal, stats: ColumnStats) -> Self {
        self.columns.insert(ordinal, stats);
        self
    }

    /// Record how a column's values are spread.
    #[must_use]
    pub fn with_histogram(mut self, ordinal: Ordinal, histogram: Histogram) -> Self {
        self.histograms.insert(ordinal, histogram);
        self
    }

    /// How `ordinal`'s values are spread, if that has been measured.
    #[must_use]
    pub fn histogram(&self, ordinal: Ordinal) -> Option<&Histogram> {
        self.histograms.get(&ordinal)
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

    /// The fraction a range comparison is expected to keep, knowing nothing
    /// about where the value falls.
    ///
    /// The fallback for a column with no histogram. It cannot tell `at < 10`
    /// from `at < 500`, which is the whole reason [`Histogram`] exists.
    #[must_use]
    pub fn range_selectivity(&self, ordinal: Ordinal, one_sided: bool) -> f64 {
        let stats = self.column(ordinal);
        let base = if one_sided { 0.33 } else { 0.1 };
        (base * (1.0 - stats.null_fraction)).clamp(f64::MIN_POSITIVE, 1.0)
    }

    /// The fraction `bounds` keeps, using the column's histogram if there is
    /// one and [`TableStats::range_selectivity`] if there is not.
    ///
    /// Nulls never satisfy a comparison, so whatever the bounds keep is
    /// scaled by the fraction of rows that are not null.
    #[must_use]
    pub fn bounded_selectivity(&self, ordinal: Ordinal, bounds: &[(CmpOp, &Value)]) -> f64 {
        let Some(histogram) = self.histogram(ordinal) else {
            return self.range_selectivity(ordinal, bounds.len() == 1);
        };
        // Start with everything and narrow by each bound. Two bounds on one
        // column are an interval, and an interval is what is left after
        // cutting from both ends.
        let mut low = 0.0f64;
        let mut high = 1.0f64;
        for (op, value) in bounds {
            if value.is_null() {
                return 0.0;
            }
            let at = histogram.fraction_below(value);
            match op {
                CmpOp::Lt | CmpOp::Le => high = high.min(at),
                CmpOp::Gt | CmpOp::Ge => low = low.max(at),
                CmpOp::Eq => return self.equality_selectivity(ordinal),
                CmpOp::Ne => return 1.0 - self.equality_selectivity(ordinal),
            }
        }
        let not_null = 1.0 - self.column(ordinal).null_fraction;
        ((high - low).max(0.0) * not_null).clamp(f64::MIN_POSITIVE, 1.0)
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
                        self.bounded_selectivity(*column, &[(*op, value)])
                    }
                }
            }
            // Two columns compared with no literal in sight. There is no
            // histogram that would help — that would need a joint
            // distribution — so this is a guess, and an equality between two
            // columns is guessed the way a join's is: one over the coarser
            // of the two distinct counts.
            Expr::CompareColumns { left, op, right } => {
                let coarser = self
                    .column(*left)
                    .distinct
                    .min(self.column(*right).distinct)
                    .max(1) as f64;
                match op {
                    CmpOp::Eq => 1.0 / coarser,
                    CmpOp::Ne => 1.0 - 1.0 / coarser,
                    CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => COLUMN_RANGE_SELECTIVITY,
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
