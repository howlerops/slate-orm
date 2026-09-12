//! Aggregates, and grouping.
//!
//! The reason these belong in the kernel rather than in a caller's loop is the
//! projection. An aggregate reads a handful of columns — often none, in the case
//! of `COUNT(*)` — so the planner can be told exactly that, and an index that
//! holds them answers the whole query without reading a single row. Counting
//! through a loop over full rows cannot do that, however tight the loop is.

use crate::error::{KernelError, Result};
use slate_schema::{Ordinal, Row};
use slate_tuple::{Value, encode};
use std::collections::{BTreeSet, HashSet};

/// A value computed over a set of rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    /// `COUNT(*)`: rows, including those where every column is null.
    Count,
    /// `COUNT(column)`: rows where the column is not null.
    CountColumn(Ordinal),
    /// Smallest non-null value, or null if there is none.
    Min(Ordinal),
    /// Largest non-null value, or null if there is none.
    Max(Ordinal),
    /// Sum of the non-null values, or null if there are none.
    Sum(Ordinal),
    /// Mean of the non-null values, as a double, or null if there are none.
    Avg(Ordinal),
    /// `COUNT(DISTINCT column)`: how many different non-null values appeared.
    ///
    /// Exact, and over the *encoded* value, so two rows count as one exactly
    /// when they would collide in an index — the same definition of equality
    /// the rest of the layer uses. Exactness costs memory proportional to the
    /// number of distinct values; an approximate counter would not, and is a
    /// different aggregate rather than a cheaper version of this one.
    CountDistinct(Ordinal),
}

impl Aggregate {
    /// The column this aggregate reads, if any.
    #[must_use]
    pub const fn column(self) -> Option<Ordinal> {
        match self {
            Self::Count => None,
            Self::CountColumn(c)
            | Self::Min(c)
            | Self::Max(c)
            | Self::Sum(c)
            | Self::Avg(c)
            | Self::CountDistinct(c) => Some(c),
        }
    }

    /// Every column a set of aggregates reads.
    #[must_use]
    pub fn columns(aggregates: &[Self]) -> BTreeSet<Ordinal> {
        aggregates.iter().filter_map(|a| a.column()).collect()
    }

    fn accumulator(self) -> Accumulator {
        match self {
            Self::Count => Accumulator::Count(0),
            Self::CountColumn(_) => Accumulator::Count(0),
            Self::Min(_) => Accumulator::Extreme(None, true),
            Self::Max(_) => Accumulator::Extreme(None, false),
            Self::Sum(_) => Accumulator::Total(Total::default()),
            Self::Avg(_) => Accumulator::Total(Total::default()),
            Self::CountDistinct(_) => Accumulator::Distinct(HashSet::new()),
        }
    }
}

/// Running sums, kept in whichever form the input demands.
///
/// Integers stay exact until something forces a double, because silently
/// turning a sum of identifiers into a float is the kind of thing that is only
/// noticed in a reconciliation report.
#[derive(Debug, Clone, Copy, Default)]
struct Total {
    integer: i128,
    real: f64,
    is_real: bool,
    count: u64,
}

impl Total {
    fn add(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::I64(v) => self.integer += i128::from(*v),
            Value::U64(v) => self.integer += i128::from(*v),
            Value::F64(v) => {
                if !self.is_real {
                    self.real = self.integer as f64;
                    self.is_real = true;
                }
                self.real += *v;
            }
            other => {
                return Err(KernelError::NotSummable {
                    found: other.type_name(),
                });
            }
        }
        if self.is_real {
            // Keep the integer side consistent for a later integer input.
            self.integer = 0;
        }
        self.count += 1;
        Ok(())
    }

    fn sum(&self) -> Value {
        if self.count == 0 {
            return Value::Null;
        }
        if self.is_real {
            return Value::F64(self.real);
        }
        i64::try_from(self.integer).map_or(Value::F64(self.integer as f64), Value::I64)
    }

    fn average(&self) -> Value {
        if self.count == 0 {
            return Value::Null;
        }
        let total = if self.is_real {
            self.real
        } else {
            self.integer as f64
        };
        Value::F64(total / self.count as f64)
    }
}

#[derive(Debug, Clone)]
enum Accumulator {
    Count(u64),
    /// The extreme seen so far, and whether we are looking for the minimum.
    Extreme(Option<Value>, bool),
    Total(Total),
    /// Every distinct encoding seen. Encoded rather than held as values
    /// because [`Value`] is not hashable — it can hold a float — and because
    /// the encoding is what decides equality everywhere else here.
    Distinct(HashSet<Vec<u8>>),
}

/// Aggregates being computed over a stream of rows.
#[derive(Debug, Clone)]
pub struct Accumulators {
    specs: Vec<Aggregate>,
    state: Vec<Accumulator>,
}

impl Accumulators {
    /// Start accumulating `specs`.
    #[must_use]
    pub fn new(specs: &[Aggregate]) -> Self {
        Self {
            specs: specs.to_vec(),
            state: specs.iter().map(|a| a.accumulator()).collect(),
        }
    }

    /// Fold one row in.
    pub fn push(&mut self, row: &Row) -> Result<()> {
        for (spec, state) in self.specs.iter().zip(&mut self.state) {
            let value = spec.column().and_then(|c| row.get(c));
            match state {
                Accumulator::Count(n) => match spec.column() {
                    // COUNT(*) counts rows; COUNT(column) skips nulls.
                    None => *n += 1,
                    Some(_) => {
                        if value.is_some_and(|v| !v.is_null()) {
                            *n += 1;
                        }
                    }
                },
                Accumulator::Extreme(best, want_min) => {
                    if let Some(value) = value.filter(|v| !v.is_null()) {
                        let replace = match best.as_ref() {
                            None => true,
                            Some(current) => {
                                if *want_min {
                                    value < current
                                } else {
                                    value > current
                                }
                            }
                        };
                        if replace {
                            *best = Some(value.clone());
                        }
                    }
                }
                Accumulator::Total(total) => {
                    if let Some(value) = value.filter(|v| !v.is_null()) {
                        total.add(value)?;
                    }
                }
                Accumulator::Distinct(seen) => {
                    if let Some(value) = value.filter(|v| !v.is_null()) {
                        let encoded = encode(core::slice::from_ref(value));
                        // Checked before inserting so a repeated value does
                        // not allocate: on a low-cardinality column that is
                        // nearly every row.
                        if !seen.contains(&encoded) {
                            seen.insert(encoded);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The results, in the order the aggregates were given.
    #[must_use]
    pub fn finish(self) -> Vec<Value> {
        self.specs
            .iter()
            .zip(self.state)
            .map(|(spec, state)| match state {
                Accumulator::Count(n) => Value::U64(n),
                Accumulator::Extreme(best, _) => best.unwrap_or(Value::Null),
                Accumulator::Total(total) => match spec {
                    Aggregate::Avg(_) => total.average(),
                    _ => total.sum(),
                },
                Accumulator::Distinct(seen) => Value::U64(seen.len() as u64),
            })
            .collect()
    }
}

/// One row of a grouped result.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The grouping columns' values, in the order they were requested.
    pub key: Vec<Value>,
    /// The aggregates, in the order they were requested.
    pub values: Vec<Value>,
}
