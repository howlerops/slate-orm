//! Aggregates, and grouping.
//!
//! The reason these belong in the kernel rather than in a caller's loop is the
//! projection. An aggregate reads a handful of columns — often none, in the case
//! of `COUNT(*)` — so the planner can be told exactly that, and an index that
//! holds them answers the whole query without reading a single row. Counting
//! through a loop over full rows cannot do that, however tight the loop is.

use crate::error::{KernelError, Result};
use crate::expr::Expr;
use crate::limits::ExecutionLimits;
use crate::query::SortKey;
use slate_schema::{Ordinal, Row};
use slate_tuple::{Direction, Value, encode, encode_value_into};
use std::collections::{BTreeSet, HashMap, HashSet};

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
            Value::I64(v) => self.add_integer(i128::from(*v)),
            Value::U64(v) => self.add_integer(i128::from(*v)),
            Value::F64(v) => self.add_real(*v),
            other => {
                return Err(KernelError::NotSummable {
                    found: other.type_name(),
                });
            }
        }
        self.count += 1;
        Ok(())
    }

    /// Fold in an integer, on whichever side the total is currently kept.
    ///
    /// The `is_real` branch is the whole point. This used to add to `integer`
    /// unconditionally and then, two lines later, zero `integer` whenever
    /// `is_real` — so every integer arriving *after* the first float was
    /// added and immediately discarded. Nothing caught it while a column had
    /// one declared type and an aggregate therefore only ever saw one domain;
    /// `Scalar` made `coalesce(price, amount)` over a nullable `f64` and an
    /// `i64` an ordinary thing to write, and that produces a float on some
    /// rows and an integer on others.
    ///
    /// It failed as an order dependence, which is the sharpest way it could
    /// have shown up: `SUM` over the same six rows gave 1.5 read forwards and
    /// 11.5 read backwards, because reading backwards put the float first and
    /// dropped every integer behind it. An aggregate is a fold over a set and
    /// nothing about the answer may depend on the order of the fold.
    fn add_integer(&mut self, value: i128) {
        if self.is_real {
            // Exact for every integer up to 2^53, which is far past anything
            // a single row contributes; a total that has already gone real
            // was going to be approximate regardless.
            self.real += value as f64;
        } else {
            self.integer += value;
        }
    }

    /// Fold in a float, moving the total to the real side if it is not there.
    fn add_real(&mut self, value: f64) {
        if !self.is_real {
            self.real = self.integer as f64;
            self.integer = 0;
            self.is_real = true;
        }
        self.real += value;
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
    /// Distinct values one `COUNT(DISTINCT)` here may hold before refusing.
    max_distinct: usize,
}

impl Accumulators {
    /// Start accumulating `specs` under the default limits.
    #[must_use]
    pub fn new(specs: &[Aggregate]) -> Self {
        Self::with_limits(specs, ExecutionLimits::default())
    }

    /// Start accumulating `specs`, refusing beyond `limits`.
    #[must_use]
    pub fn with_limits(specs: &[Aggregate], limits: ExecutionLimits) -> Self {
        Self {
            specs: specs.to_vec(),
            state: specs.iter().map(|a| a.accumulator()).collect(),
            max_distinct: limits.max_distinct,
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
                            // Checked here rather than after inserting so the
                            // limit is a ceiling on what is held, not one
                            // value past it.
                            if seen.len() >= self.max_distinct {
                                return Err(KernelError::TooManyDistinctValues {
                                    limit: self.max_distinct,
                                });
                            }
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

impl Group {
    /// The group as one row: its key, then its aggregates.
    ///
    /// What `HAVING` is evaluated against. Laying them out in one ordinal
    /// space means the ordinary predicate language can filter groups without
    /// gaining a notion of what an aggregate is — the `i`th aggregate is at
    /// `key.len() + i`, and [`Group::aggregate`] does that arithmetic.
    #[must_use]
    pub fn as_row(&self) -> Row {
        let mut values = self.key.clone();
        values.extend(self.values.iter().cloned());
        Row::new(values)
    }

    /// Where the `n`th aggregate sits in [`Group::as_row`], given how many
    /// columns the query grouped by.
    #[must_use]
    pub const fn aggregate(group_columns: usize, n: usize) -> Ordinal {
        Ordinal(group_columns + n)
    }
}

/// What to do with a set of rows: how to group them, what to compute over each
/// group, and what to do with the groups once they exist.
///
/// One value rather than six arguments, because the same request now arrives
/// from two places — a single-table cursor and a join — and a six-argument
/// method that grows to eight is how two callers come to disagree about the
/// order of `having` and `group`. It also gives `ORDER BY` and `LIMIT` over
/// groups somewhere to live that is not another overload.
///
/// The ordinals are in the space of whatever is being grouped: a table's own
/// columns for a single-table read, and [`JoinSchema`](crate::JoinSchema)'s
/// space for a join — the same space `Join::having` is already written in.
/// [`Grouping::sort`] is the exception and is deliberately different: it names
/// the *group*, not the row, so it is a key position or an aggregate position,
/// exactly as `having` is. See [`Group::as_row`].
#[derive(Debug, Clone, PartialEq)]
pub struct Grouping {
    /// Columns whose distinct combinations make the groups. Empty means one
    /// group over everything.
    pub group: Vec<Ordinal>,
    /// What to compute per group.
    pub aggregates: Vec<Aggregate>,
    /// Which groups survive. Evaluated over [`Group::as_row`].
    pub having: Expr,
    /// How to order the groups. Over [`Group::as_row`] as well, so
    /// `ORDER BY count(*) DESC` is a sort key on [`Group::aggregate`].
    ///
    /// Empty leaves the groups in the order the encoded key sorts them, which
    /// is what this returned before there was any way to ask for another.
    pub sort: Vec<SortKey>,
    /// At most this many groups.
    pub limit: Option<usize>,
    /// Groups to discard first.
    pub offset: usize,
}

impl Grouping {
    /// Group by `columns`, computing `aggregates`.
    #[must_use]
    pub fn by<I: IntoIterator<Item = Ordinal>>(columns: I, aggregates: &[Aggregate]) -> Self {
        Self {
            group: columns.into_iter().collect(),
            aggregates: aggregates.to_vec(),
            having: Expr::True,
            sort: Vec::new(),
            limit: None,
            offset: 0,
        }
    }

    /// Keep only the groups `having` admits.
    #[must_use]
    pub fn having(mut self, having: Expr) -> Self {
        self.having = having;
        self
    }

    /// Order the groups.
    #[must_use]
    pub fn sort_by<I: IntoIterator<Item = SortKey>>(mut self, keys: I) -> Self {
        self.sort = keys.into_iter().collect();
        self
    }

    /// Return at most `limit` groups.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Discard the first `offset` groups.
    #[must_use]
    pub const fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Every column of the grouped row this reads.
    ///
    /// The projection a read has to satisfy before grouping can happen: the
    /// grouping columns and whatever the aggregates look at. `having` and
    /// `sort` are *not* here — they read the group, which does not exist yet.
    #[must_use]
    pub fn columns(&self) -> BTreeSet<Ordinal> {
        let mut columns = Aggregate::columns(&self.aggregates);
        columns.extend(self.group.iter().copied());
        columns
    }
}

/// Rows being folded into groups.
///
/// The one hash-grouping implementation in the layer. It takes rows rather
/// than a cursor precisely so that it can serve a single-table scan and a join
/// alike: a second copy of this over a different source is how a `GROUP BY`
/// comes to mean two different things depending on whether a join was
/// involved, which is the shape of bug the oracles here exist to prevent.
#[derive(Debug)]
pub struct Grouper<'g> {
    grouping: &'g Grouping,
    /// What this request may spend. See [`ExecutionLimits`].
    limits: ExecutionLimits,
    /// Keyed on the *encoded* grouping values rather than ordered on the
    /// values themselves. An ordered map gives group order for free, which was
    /// worth having until it was measured: a million distinct keys cost
    /// O(log k) comparisons of a `Vec<Value>` on every row, and the same query
    /// with a filter that cut the keys down ran nearly four times faster.
    /// Encoding the key once per row and hashing the bytes replaces those
    /// comparisons with one hash, and it is the same equality an index uses —
    /// two rows group together exactly when they would collide in a key.
    groups: HashMap<Vec<u8>, (Vec<Value>, Accumulators)>,
    /// Reused across rows so grouping a million rows is not a million
    /// allocations.
    encoded: Vec<u8>,
}

impl<'g> Grouper<'g> {
    /// Start grouping for `grouping` under the default limits.
    #[must_use]
    pub fn new(grouping: &'g Grouping) -> Self {
        Self::with_limits(grouping, ExecutionLimits::default())
    }

    /// Start grouping for `grouping`, refusing beyond `limits`.
    #[must_use]
    pub fn with_limits(grouping: &'g Grouping, limits: ExecutionLimits) -> Self {
        Self {
            grouping,
            limits,
            groups: HashMap::new(),
            encoded: Vec::new(),
        }
    }

    /// Fold one row into its group.
    pub fn push(&mut self, row: &Row) -> Result<()> {
        self.encoded.clear();
        for ordinal in &self.grouping.group {
            let value = row.get(*ordinal).unwrap_or(&Value::Null);
            encode_value_into(&mut self.encoded, value, Direction::Asc);
        }
        match self.groups.get_mut(self.encoded.as_slice()) {
            Some((_, accumulators)) => accumulators.push(row)?,
            None => {
                // A new group is about to be held, so the ceiling is checked
                // before it is rather than after.
                if self.groups.len() >= self.limits.max_groups {
                    return Err(KernelError::TooManyGroups {
                        limit: self.limits.max_groups,
                    });
                }
                let key: Vec<Value> = self
                    .grouping
                    .group
                    .iter()
                    .map(|c| row.get(*c).cloned().unwrap_or(Value::Null))
                    .collect();
                let mut accumulators =
                    Accumulators::with_limits(&self.grouping.aggregates, self.limits);
                accumulators.push(row)?;
                self.groups
                    .insert(self.encoded.clone(), (key, accumulators));
            }
        }
        Ok(())
    }

    /// The groups, filtered, ordered and windowed.
    ///
    /// The order of the three is SQL's and is not negotiable: `HAVING` decides
    /// which groups exist, `ORDER BY` arranges the ones that do, and
    /// `LIMIT`/`OFFSET` take a slice of that. Windowing before ordering would
    /// return a different set of groups, not the same set in a different
    /// order.
    #[must_use]
    pub fn finish(self) -> Vec<Group> {
        let grouping = self.grouping;
        // Sorted once at the end rather than maintained throughout. The order
        // is the same one an ordered map produced — the encoding sorts as the
        // values do, which is the property the whole keyspace rests on — so
        // this is still deterministic, and callers that depended on it still
        // get it.
        let mut out: Vec<(Vec<u8>, Group)> = self
            .groups
            .into_iter()
            .map(|(encoded, (key, accumulators))| {
                (
                    encoded,
                    Group {
                        key,
                        values: accumulators.finish(),
                    },
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));

        // `HAVING` filters groups, not rows, so it runs here and reads the
        // aggregates rather than the columns. A group's key comes first and
        // its aggregates after, in one ordinal space, so the same predicate
        // language serves without learning anything new.
        let mut groups: Vec<Group> = out
            .into_iter()
            .map(|(_, group)| group)
            .filter(|group| {
                matches!(grouping.having, Expr::True) || grouping.having.admits(&group.as_row())
            })
            .collect();

        if !grouping.sort.is_empty() {
            // Stable, and on top of the encoded-key order above, so ties in the
            // requested keys fall back to it rather than to whatever the hash
            // map happened to yield. Without that, `ORDER BY count(*) DESC
            // LIMIT 3` over groups that tie would return a different three on
            // different runs — the same defect as an `ORDER BY` that does not
            // pin its ties, which every oracle here appends the primary key to
            // avoid.
            //
            // A full sort rather than the bounded top-N heap the row path uses,
            // and deliberately. That heap exists for memory: sorting a million
            // rows to return ten holds a million decoded rows. Groups are
            // already all in memory before the first one can be returned —
            // nothing can be known about the last group until the last row is
            // read — so a heap would save nothing and be a second sorting
            // implementation to keep in step with the first. The comparator is
            // shared with the row path for the same reason.
            let keys = &grouping.sort;
            groups.sort_by(|a, b| crate::exec::compare_rows(&a.as_row(), &b.as_row(), keys));
        }

        if grouping.offset > 0 {
            groups.drain(..grouping.offset.min(groups.len()));
        }
        if let Some(limit) = grouping.limit {
            groups.truncate(limit);
        }
        groups
    }
}
