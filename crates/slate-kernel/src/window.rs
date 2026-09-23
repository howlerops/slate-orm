//! Values computed over a partition, one per input row.
//!
//! # Why this is not an aggregate
//!
//! [`Grouper`](crate::aggregate::Grouper) holds one entry per distinct key and
//! folds each row into that entry's accumulators — the row itself is dropped
//! the moment it has been counted. That is the right shape for `GROUP BY`,
//! where ten thousand rows become four, and it is exactly the wrong shape
//! here: a window function answers "what is this row's rank among its peers",
//! which has one answer *per input row*. The cardinalities are opposites, so
//! this could not be a new [`Aggregate`] variant however the enum was widened.
//!
//! What it can share is the arithmetic. `SUM(x) OVER (…)` and `SUM(x)` add the
//! same numbers, and [`Accumulators`] already carries the exact-integer,
//! decimal-aware running total that took a bug to get right. So a window
//! aggregate is [`WindowFunction::Over`] wrapping an ordinary [`Aggregate`],
//! and nothing about how values are added is written twice.
//!
//! # The frame, which is where SQL surprises people
//!
//! `SUM(x) OVER (PARTITION BY p)` is the partition's total, repeated on every
//! row. `SUM(x) OVER (PARTITION BY p ORDER BY t)` is a *running* total — and
//! this is not a variant spelling, it is SQL's default frame changing from
//! `RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING` to `RANGE
//! BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW` the moment an `ORDER BY`
//! appears. Both are implemented here and both follow the standard, because
//! the alternative is a number that looks right and is not.
//!
//! `RANGE` also decides what happens on a tie: rows equal under the window's
//! `ORDER BY` are *peers*, and every peer sees the same running value — the
//! one that includes all of them. Implementing this as `ROWS` instead (each
//! row seeing only itself and what came before) is a one-character difference
//! in the loop and gives different answers on any column with duplicates,
//! which is most of them. See `a_running_total_gives_peers_the_same_value`.
//!
//! Explicit frames (`ROWS BETWEEN 3 PRECEDING AND CURRENT ROW`) are not
//! offered. They are a real feature and a larger one: an arbitrary frame turns
//! a single forward pass into a sliding window with its own eviction, and
//! several aggregates cannot be un-accumulated at all (`MIN` over a shrinking
//! frame needs a monotonic deque, not a subtraction). Left out rather than
//! approximated.
//!
//! # What it costs
//!
//! Every admitted row is held. A window needs its whole partition before it
//! can answer for any row in it, partitions arrive interleaved in scan order,
//! and nothing in the keyspace groups them — so there is no streaming form of
//! this operator short of a partitioned sort spilling to storage. That is why
//! [`ExecutionLimits::max_window_rows`] exists and why it is checked while
//! collecting rather than after.
//!
//! Rows are never moved. Each distinct `(partition, order)` specification
//! sorts a `Vec<usize>` of indices and walks that, writing results back by
//! index, so two windows with different partitions cost two permutations and
//! zero row copies — and the query's own `ORDER BY`, which runs afterwards,
//! sees rows in their original arrival order rather than in whichever window's
//! order happened to be applied last.

use slate_schema::{Ordinal, Row};
use slate_tuple::Value;

use crate::aggregate::{Accumulators, Aggregate};
use crate::error::{KernelError, Result};
use crate::exec::compare_rows;
use crate::limits::ExecutionLimits;
use crate::query::SortKey;

/// What a window computes for each row of its partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFunction {
    /// The row's position in its partition, from 1. Ties are broken by the
    /// order the rows arrived in, which is the only tiebreak available and is
    /// why [`Window`] refuses this without an `ORDER BY`.
    RowNumber,
    /// The row's rank, where peers share a rank and the next rank skips the
    /// gap: `1, 1, 3`.
    Rank,
    /// The row's rank with no gaps: `1, 1, 2`.
    DenseRank,
    /// An ordinary aggregate over the frame. Whole-partition without an
    /// `ORDER BY`, running through the current row's peers with one.
    Over(Aggregate),
    /// The value `offset` rows earlier in the partition's order, or null at
    /// the start.
    Lag {
        /// The column to read.
        column: Ordinal,
        /// How far back. Zero is refused — it is the current row, spelled
        /// obscurely.
        offset: usize,
    },
    /// The value `offset` rows later in the partition's order, or null at the
    /// end.
    Lead {
        /// The column to read.
        column: Ordinal,
        /// How far forward. Zero is refused, as for [`WindowFunction::Lag`].
        offset: usize,
    },
}

impl WindowFunction {
    /// The column this reads, for the projection.
    ///
    /// `None` for the ranking functions and `COUNT(*)`, which read the row's
    /// position rather than any of its values.
    #[must_use]
    pub const fn column(self) -> Option<Ordinal> {
        match self {
            Self::RowNumber | Self::Rank | Self::DenseRank => None,
            Self::Over(aggregate) => aggregate.column(),
            Self::Lag { column, .. } | Self::Lead { column, .. } => Some(column),
        }
    }

    /// Whether the partition has to be ordered for this to mean anything.
    const fn needs_order(self) -> bool {
        match self {
            Self::RowNumber | Self::Rank | Self::DenseRank => true,
            Self::Lag { .. } | Self::Lead { .. } => true,
            // A whole-partition aggregate is well defined unordered; adding an
            // order turns it into a running one, which is a different and also
            // valid answer.
            Self::Over(_) => false,
        }
    }

    /// The name a refusal uses, so the caller sees what it wrote.
    const fn name(self) -> &'static str {
        match self {
            Self::RowNumber => "ROW_NUMBER",
            Self::Rank => "RANK",
            Self::DenseRank => "DENSE_RANK",
            Self::Over(_) => "a window aggregate",
            Self::Lag { .. } => "LAG",
            Self::Lead { .. } => "LEAD",
        }
    }
}

/// One `OVER (…)` clause and the function computed under it.
///
/// Built through [`Window::new`], which refuses the specifications that have
/// no meaning rather than answering them arbitrarily.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// What to compute.
    pub function: WindowFunction,
    /// `PARTITION BY`. Empty means the whole result is one partition, which is
    /// what SQL means by omitting the clause.
    pub partition: Vec<Ordinal>,
    /// The window's own `ORDER BY`, which is not the query's. It decides peer
    /// groups and, for an aggregate, turns the frame into a running one.
    pub order: Vec<SortKey>,
}

impl Window {
    /// A window, or the reason this one cannot be answered.
    ///
    /// Two shapes are refused rather than given a defensible-looking answer:
    ///
    /// - A ranking function, `LAG` or `LEAD` with no `ORDER BY`. `RANK` and
    ///   `DENSE_RANK` would be 1 for every row, which is not an answer anybody
    ///   wants; `ROW_NUMBER`, `LAG` and `LEAD` would number, or step through,
    ///   an order the query never asked for. Postgres permits all of these;
    ///   the argument for refusing is that the permissive reading is never
    ///   what the author meant, and a query that silently numbers rows by
    ///   whichever index the planner picked is the kind of wrong answer this
    ///   repository refuses elsewhere.
    /// - `COUNT(DISTINCT …)` with an `ORDER BY`. The running form needs the
    ///   distinct set as it stood at each peer group, and the only way to keep
    ///   those is one copy of the set per group — quadratic in the partition,
    ///   which is what [`ExecutionLimits`] exists to keep out. Without an
    ///   `ORDER BY` it is one set for the whole partition and is allowed.
    /// - `LAG`/`LEAD` at offset zero, which is the current row's value written
    ///   the long way round and is much more likely to be a bug than an
    ///   intention.
    pub fn new(
        function: WindowFunction,
        partition: Vec<Ordinal>,
        order: Vec<SortKey>,
    ) -> Result<Self> {
        if order.is_empty() && function.needs_order() {
            return Err(KernelError::WindowNeedsOrder {
                function: function.name(),
            });
        }
        if !order.is_empty()
            && matches!(function, WindowFunction::Over(Aggregate::CountDistinct(_)))
        {
            return Err(KernelError::RunningDistinctCount);
        }
        if matches!(
            function,
            WindowFunction::Lag { offset: 0, .. } | WindowFunction::Lead { offset: 0, .. }
        ) {
            return Err(KernelError::WindowOffsetZero {
                function: function.name(),
            });
        }
        Ok(Self {
            function,
            partition,
            order,
        })
    }

    /// Every column this window reads, so the planner decodes them.
    ///
    /// The partition and order columns are in here as well as the function's
    /// argument: a window partitioned by a column the projection left out
    /// would see that column as null on every row, put the whole result in one
    /// partition, and answer — wrongly, and without saying so. The same
    /// argument the sort keys are pinned for.
    pub fn collect_columns(&self, out: &mut impl Extend<Ordinal>) {
        out.extend(self.function.column());
        out.extend(self.partition.iter().copied());
        out.extend(self.order.iter().map(|key| key.column));
    }
}

/// Append each window's value to every row.
///
/// `rows` arrive with the table's columns and the query's computed values
/// already on them, and leave one value wider per window, in the order the
/// windows were declared — so window `i` is at
/// `table.columns().len() + compute.len() + i`, which is what
/// [`Query::windowed`](crate::Query::windowed) reports.
pub(crate) fn evaluate(
    windows: &[Window],
    rows: Vec<Row>,
    limits: ExecutionLimits,
) -> Result<Vec<Row>> {
    if windows.is_empty() {
        return Ok(rows);
    }
    // One slot per row per window, filled by index. Writing results back by
    // index rather than reordering rows is what lets two windows with
    // different partitions share one materialisation, and it leaves `rows` in
    // arrival order for the query's own `ORDER BY` afterwards.
    let mut computed: Vec<Vec<Value>> = vec![vec![Value::Null; windows.len()]; rows.len()];

    // Windows that share a specification share a permutation. Two `OVER
    // (PARTITION BY country ORDER BY year)` clauses are one sort, not two,
    // which is the common case: a rank and a running total side by side.
    let mut specs: Vec<(&Window, Vec<usize>)> = Vec::new();
    for (slot, window) in windows.iter().enumerate() {
        match specs
            .iter_mut()
            .find(|(spec, _)| spec.partition == window.partition && spec.order == window.order)
        {
            Some((_, together)) => together.push(slot),
            None => specs.push((window, vec![slot])),
        }
    }

    for (spec, slots) in specs {
        let partition_keys: Vec<SortKey> =
            spec.partition.iter().copied().map(SortKey::asc).collect();
        let mut sort_keys = partition_keys.clone();
        sort_keys.extend(spec.order.iter().copied());

        // Paired with their index so nothing is ever indexed back out of
        // `rows`, and so the permutation carries where each value belongs.
        let mut order: Vec<(usize, &Row)> = rows.iter().enumerate().collect();
        // Stable, so rows equal on every key stay in the order they arrived.
        // That is the only tiebreak available and it makes `ROW_NUMBER`
        // reproducible for a given access path rather than merely plausible.
        order.sort_by(|(_, left), (_, right)| compare_rows(left, right, &sort_keys));

        // `chunk_by` cuts the permutation wherever two adjacent rows differ,
        // which with no `PARTITION BY` is nowhere — one chunk over everything,
        // exactly what SQL means by omitting the clause.
        for partition in order.chunk_by(|(_, left), (_, right)| {
            compare_rows(left, right, &partition_keys) == core::cmp::Ordering::Equal
        }) {
            for &slot in &slots {
                if let Some(window) = windows.get(slot) {
                    fill(partition, window, limits, &mut computed, slot)?;
                }
            }
        }
    }

    Ok(rows
        .into_iter()
        .zip(computed)
        .map(|(row, values)| {
            let mut out = row.into_values();
            out.extend(values);
            Row::new(out)
        })
        .collect())
}

/// Compute `window` over one partition, writing into `computed[row][slot]`.
fn fill(
    partition: &[(usize, &Row)],
    window: &Window,
    limits: ExecutionLimits,
    computed: &mut [Vec<Value>],
    slot: usize,
) -> Result<()> {
    match window.function {
        WindowFunction::RowNumber => {
            for (at, (index, _)) in partition.iter().enumerate() {
                set(computed, *index, slot, Value::U64(at as u64 + 1));
            }
        }
        WindowFunction::Rank | WindowFunction::DenseRank => {
            let dense = matches!(window.function, WindowFunction::DenseRank);
            // `before` is the number of rows already passed and `group` the
            // number of peer groups. A rank is the first, plus one — which is
            // what leaves the gap; a dense rank is the second, which does not.
            let mut before = 0usize;
            for (group, peers) in peer_groups(partition, &window.order).enumerate() {
                let rank = if dense { group } else { before };
                for (index, _) in peers {
                    set(computed, *index, slot, Value::U64(rank as u64 + 1));
                }
                before += peers.len();
            }
        }
        WindowFunction::Over(aggregate) => {
            let mut accumulators = Accumulators::with_limits(&[aggregate], limits);
            if window.order.is_empty() {
                // No frame to move: the whole partition, on every row.
                for (_, row) in partition {
                    accumulators.push(row)?;
                }
                let value = value_of(accumulators.finish());
                for (index, _) in partition {
                    set(computed, *index, slot, value.clone());
                }
            } else {
                // `RANGE … CURRENT ROW`: fold a whole peer group in, *then*
                // read, so every peer sees the value that includes all of
                // them. Reading per row instead would be `ROWS`, and the two
                // differ on any column with duplicates.
                for peers in peer_groups(partition, &window.order) {
                    for (_, row) in peers {
                        accumulators.push(row)?;
                    }
                    let value = value_of(accumulators.clone().finish());
                    for (index, _) in peers {
                        set(computed, *index, slot, value.clone());
                    }
                }
            }
        }
        WindowFunction::Lag { column, offset } => {
            for (at, (index, _)) in partition.iter().enumerate() {
                let value = at
                    .checked_sub(offset)
                    .and_then(|from| partition.get(from))
                    .and_then(|(_, row)| row.get(column))
                    .cloned()
                    .unwrap_or(Value::Null);
                set(computed, *index, slot, value);
            }
        }
        WindowFunction::Lead { column, offset } => {
            for (at, (index, _)) in partition.iter().enumerate() {
                let value = at
                    .checked_add(offset)
                    .and_then(|from| partition.get(from))
                    .and_then(|(_, row)| row.get(column))
                    .cloned()
                    .unwrap_or(Value::Null);
                set(computed, *index, slot, value);
            }
        }
    }
    Ok(())
}

/// The runs of `partition` whose rows are equal under `keys`.
///
/// Only ever called with a non-empty `keys`: every function that asks for peer
/// groups is one [`Window::new`] refuses without an `ORDER BY`, and the one
/// that does not — a whole-partition aggregate — takes the other branch. With
/// an empty `keys` this would answer "one group" rather than "one per row",
/// which is the wrong half of that distinction and is why it is unreachable
/// rather than handled.
fn peer_groups<'a>(
    partition: &'a [(usize, &'a Row)],
    keys: &'a [SortKey],
) -> impl Iterator<Item = &'a [(usize, &'a Row)]> {
    partition.chunk_by(|(_, left), (_, right)| {
        compare_rows(left, right, keys) == core::cmp::Ordering::Equal
    })
}

/// An `Accumulators` built from one spec finishes to one value.
fn value_of(mut finished: Vec<Value>) -> Value {
    if finished.is_empty() {
        Value::Null
    } else {
        finished.swap_remove(0)
    }
}

fn set(computed: &mut [Vec<Value>], row: usize, slot: usize, value: Value) {
    if let Some(cell) = computed.get_mut(row).and_then(|v| v.get_mut(slot)) {
        *cell = value;
    }
}
