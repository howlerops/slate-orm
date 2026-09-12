//! Values computed from a row, rather than read out of one.
//!
//! [`Expr`](crate::Expr) says whether a row passes; a [`Scalar`] says what a
//! row *is worth*. Until this existed the layer could filter on a column but
//! not on anything derived from one — no `length(url)`, no `x + 1`, no minute
//! of a timestamp, no `CASE WHEN`. Eight ClickBench queries needed only that,
//! and they needed it in eight different-looking ways.
//!
//! # How it reaches the rest of the layer
//!
//! A scalar is not woven through aggregates and grouping. Instead a query can
//! *compute* extra values, which are appended to the row after the table's own
//! columns and addressed by ordinal like everything else — the same trick
//! [`JoinSchema`](crate::JoinSchema) uses for a joined row's columns.
//!
//! That means `GROUP BY length(url)`, `SUM(width + 1)`, `ORDER BY` a computed
//! value and a predicate over one all work without aggregates, sorting or the
//! planner learning what an expression is. See [`Query::compute`](crate::Query::compute).
//!
//! # Nulls
//!
//! A scalar over a null is null, and arithmetic that cannot be performed —
//! dividing by zero, adding a string to a number — is null rather than an
//! error. That matches SQL, and it matches the three-valued logic the
//! predicate language already uses: a computed null compared to anything is
//! [`Truth::Unknown`](crate::Truth), which admits nothing.

use crate::expr::{Columns, Expr};
use slate_schema::Ordinal;
use slate_tuple::Value;

/// How to measure the distance between two vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Straight-line distance. Smaller is nearer.
    L2,
    /// Squared straight-line distance.
    ///
    /// Ranks identically to [`Metric::L2`] — the square root is monotonic — and
    /// skips the square root per row. For ordering, which is what a k-NN search
    /// does, this is the one to use.
    L2Squared,
    /// One minus cosine similarity, so smaller is nearer and identical
    /// directions are zero. The usual metric for text embeddings, which
    /// encode meaning in direction rather than magnitude.
    Cosine,
    /// The negated dot product, negated so that — like the others — smaller is
    /// nearer. Correct for embeddings already normalised to unit length, where
    /// it ranks the same as cosine for less work.
    NegativeInnerProduct,
}

impl Metric {
    /// Measure between two vectors of the same length.
    #[must_use]
    pub fn between(self, a: &[f32], b: &[f32]) -> Option<f64> {
        if a.len() != b.len() {
            return None;
        }
        match self {
            Self::L2 => Some(Self::L2Squared.between(a, b)?.sqrt()),
            Self::L2Squared => Some(
                a.iter()
                    .zip(b)
                    .map(|(x, y)| {
                        let d = f64::from(*x) - f64::from(*y);
                        d * d
                    })
                    .sum(),
            ),
            Self::NegativeInnerProduct => Some(
                -a.iter()
                    .zip(b)
                    .map(|(x, y)| f64::from(*x) * f64::from(*y))
                    .sum::<f64>(),
            ),
            Self::Cosine => {
                let mut dot = 0.0f64;
                let mut left = 0.0f64;
                let mut right = 0.0f64;
                for (x, y) in a.iter().zip(b) {
                    let (x, y) = (f64::from(*x), f64::from(*y));
                    dot += x * y;
                    left += x * x;
                    right += y * y;
                }
                let magnitude = (left * right).sqrt();
                // A zero vector has no direction, so its cosine distance to
                // anything is undefined rather than zero.
                (magnitude > 0.0).then(|| 1.0 - dot / magnitude)
            }
        }
    }
}

/// A part of a timestamp, for [`Scalar::Extract`] and [`Scalar::DateTrunc`].
///
/// Timestamps here are seconds since the epoch held in an integer column,
/// which is how the datasets this was built against store them. A real date
/// type would make these calendar operations rather than arithmetic; that is a
/// type-system change, and this is not pretending to be one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    /// Seconds.
    Second,
    /// Minutes.
    Minute,
    /// Hours.
    Hour,
    /// Days.
    Day,
}

impl TimeUnit {
    /// How many seconds one of these is.
    #[must_use]
    pub const fn seconds(self) -> i64 {
        match self {
            Self::Second => 1,
            Self::Minute => 60,
            Self::Hour => 3_600,
            Self::Day => 86_400,
        }
    }

    /// The next unit up, which is what `extract` counts within.
    const fn within(self) -> i64 {
        match self {
            Self::Second | Self::Minute => 60,
            Self::Hour => 24,
            // Day of the epoch, which has nothing above it to wrap at.
            Self::Day => i64::MAX,
        }
    }
}

/// A value computed from a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    /// A column, read as-is.
    Column(Ordinal),
    /// A constant.
    Literal(Value),
    /// `a + b`, on numbers; concatenation is [`Scalar::Concat`].
    Add(Box<Scalar>, Box<Scalar>),
    /// `a - b`.
    Sub(Box<Scalar>, Box<Scalar>),
    /// `a * b`.
    Mul(Box<Scalar>, Box<Scalar>),
    /// `a / b`. Null when `b` is zero, rather than an error.
    Div(Box<Scalar>, Box<Scalar>),
    /// Characters in a string, or bytes in a byte string.
    Length(Box<Scalar>),
    /// Strings joined end to end.
    Concat(Vec<Scalar>),
    /// A string in lower case.
    Lower(Box<Scalar>),
    /// A string in upper case.
    Upper(Box<Scalar>),
    /// The value of one part of a timestamp: the minute of the hour, the hour
    /// of the day.
    Extract {
        /// Which part.
        unit: TimeUnit,
        /// The timestamp, in seconds since the epoch.
        value: Box<Scalar>,
    },
    /// A timestamp rounded down to a whole unit.
    DateTrunc {
        /// Which unit to round to.
        unit: TimeUnit,
        /// The timestamp, in seconds since the epoch.
        value: Box<Scalar>,
    },
    /// The first branch whose condition holds, or `otherwise`.
    Case {
        /// Conditions and what they produce, tried in order.
        branches: Vec<(Expr, Scalar)>,
        /// What to produce when none held.
        otherwise: Box<Scalar>,
    },
    /// The first argument that is not null, or null.
    Coalesce(Vec<Scalar>),
    /// How far apart two vectors are.
    ///
    /// This is what makes nearest-neighbour search a query rather than a
    /// feature: compute the distance to a query vector, order by it, take the
    /// first `k`. The bounded top-N sort already keeps only `k` rows, so a
    /// k-NN search over a million embeddings holds `k` of them, not a million.
    ///
    /// Exact, and by brute force — every row's distance is computed. There is
    /// no vector index, so this is linear in the table. That is the same thing
    /// pgvector does before an `ivfflat` or `hnsw` index is built, and it is
    /// honest about what it costs.
    ///
    /// Null when either side is not a vector, or when the two have different
    /// numbers of dimensions: comparing a 768-dimension embedding to a
    /// 1536-dimension one is a mistake, not a distance.
    Distance {
        /// One vector.
        left: Box<Scalar>,
        /// The other, usually a literal query vector.
        right: Box<Scalar>,
        /// How to measure.
        metric: Metric,
    },
    /// Every match of a regular expression replaced.
    ///
    /// Capture groups are referred to as `\1`, which is what SQL's
    /// `REGEXP_REPLACE` uses and what queries in the wild are written with. A
    /// literal `$` is escaped rather than read as the regex crate's own group
    /// syntax — the two conventions cannot both be honoured, and following
    /// SQL is the one that makes a copied query mean what it says.
    ///
    /// A pattern that does not compile yields null rather than failing the
    /// query, the same choice [`Expr::Matches`](crate::Expr::Matches) makes.
    RegexpReplace {
        /// The text to rewrite.
        value: Box<Scalar>,
        /// The pattern to find.
        pattern: String,
        /// What to put in its place.
        replacement: String,
    },
}

impl From<Ordinal> for Scalar {
    fn from(ordinal: Ordinal) -> Self {
        Self::Column(ordinal)
    }
}

impl From<Value> for Scalar {
    fn from(value: Value) -> Self {
        Self::Literal(value)
    }
}

impl From<Vec<f32>> for Scalar {
    fn from(value: Vec<f32>) -> Self {
        Self::Literal(Value::Vector(value))
    }
}

impl From<i64> for Scalar {
    fn from(value: i64) -> Self {
        Self::Literal(Value::I64(value))
    }
}

/// Arithmetic is written as arithmetic. `Scalar::column(w) + 1` rather than a
/// method named after an operator, which is both nicer to read and avoids
/// shadowing the trait it would have been imitating.
macro_rules! arithmetic_op {
    ($trait:ident, $method:ident, $variant:ident) => {
        impl<T: Into<Scalar>> core::ops::$trait<T> for Scalar {
            type Output = Self;
            fn $method(self, other: T) -> Self {
                Self::$variant(Box::new(self), Box::new(other.into()))
            }
        }
    };
}

arithmetic_op!(Add, add, Add);
arithmetic_op!(Sub, sub, Sub);
arithmetic_op!(Mul, mul, Mul);
arithmetic_op!(Div, div, Div);

/// One side of an arithmetic operation, once it is known to be a number.
enum Number {
    Int(i64),
    Real(f64),
}

fn number(value: &Value) -> Option<Number> {
    match value {
        Value::I64(v) => Some(Number::Int(*v)),
        Value::U64(v) => i64::try_from(*v).ok().map(Number::Int),
        Value::F64(v) => Some(Number::Real(*v)),
        _ => None,
    }
}

/// Apply an operation to two values, in whichever domain they share.
///
/// Integers stay integers unless one side is real, which is what keeps
/// `width + 1` an integer instead of quietly becoming a float. Overflow
/// saturates rather than wrapping: a sum that is too large is better reported
/// as the largest thing than as a small negative one.
fn arithmetic(
    a: &Value,
    b: &Value,
    op: fn(i64, i64) -> Option<i64>,
    real: fn(f64, f64) -> f64,
) -> Value {
    match (number(a), number(b)) {
        (Some(Number::Int(x)), Some(Number::Int(y))) => op(x, y).map_or(Value::Null, Value::I64),
        (Some(x), Some(y)) => {
            let to_f = |n: Number| match n {
                Number::Int(v) => v as f64,
                Number::Real(v) => v,
            };
            Value::F64(real(to_f(x), to_f(y)))
        }
        _ => Value::Null,
    }
}

impl Scalar {
    /// A column, by ordinal.
    #[must_use]
    pub const fn column(ordinal: Ordinal) -> Self {
        Self::Column(ordinal)
    }

    /// A constant.
    #[must_use]
    pub const fn literal(value: Value) -> Self {
        Self::Literal(value)
    }

    /// `length(self)`.
    #[must_use]
    pub fn length(self) -> Self {
        Self::Length(Box::new(self))
    }

    /// One part of a timestamp.
    #[must_use]
    pub fn extract(self, unit: TimeUnit) -> Self {
        Self::Extract {
            unit,
            value: Box::new(self),
        }
    }

    /// A timestamp rounded down.
    #[must_use]
    pub fn date_trunc(self, unit: TimeUnit) -> Self {
        Self::DateTrunc {
            unit,
            value: Box::new(self),
        }
    }

    /// How far `self` is from `other`, by `metric`.
    #[must_use]
    pub fn distance(self, other: impl Into<Self>, metric: Metric) -> Self {
        Self::Distance {
            left: Box::new(self),
            right: Box::new(other.into()),
            metric,
        }
    }

    /// `regexp_replace(self, pattern, replacement)`.
    #[must_use]
    pub fn regexp_replace(
        self,
        pattern: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Self {
        Self::RegexpReplace {
            value: Box::new(self),
            pattern: pattern.into(),
            replacement: replacement.into(),
        }
    }

    /// Compute this over a row.
    #[must_use]
    pub fn evaluate<C: Columns + ?Sized>(&self, row: &C) -> Value {
        match self {
            Self::Column(ordinal) => row.value(*ordinal).cloned().unwrap_or(Value::Null),
            Self::Literal(value) => value.clone(),
            Self::Add(a, b) => arithmetic(
                &a.evaluate(row),
                &b.evaluate(row),
                i64::checked_add,
                |x, y| x + y,
            ),
            Self::Sub(a, b) => arithmetic(
                &a.evaluate(row),
                &b.evaluate(row),
                i64::checked_sub,
                |x, y| x - y,
            ),
            Self::Mul(a, b) => arithmetic(
                &a.evaluate(row),
                &b.evaluate(row),
                i64::checked_mul,
                |x, y| x * y,
            ),
            // Division by zero is null, not a panic and not an error: a query
            // over a million rows should not fail because one of them held a
            // zero.
            Self::Div(a, b) => arithmetic(
                &a.evaluate(row),
                &b.evaluate(row),
                i64::checked_div,
                |x, y| if y == 0.0 { f64::NAN } else { x / y },
            ),
            Self::Length(inner) => match inner.evaluate(row) {
                // Characters rather than bytes for a string, which is what
                // anyone asking the length of a URL means.
                Value::Str(s) => Value::I64(s.chars().count() as i64),
                Value::Bytes(b) => Value::I64(b.len() as i64),
                _ => Value::Null,
            },
            Self::Concat(parts) => {
                let mut out = String::new();
                for part in parts {
                    match part.evaluate(row) {
                        Value::Str(s) => out.push_str(&s),
                        // A null anywhere makes the whole thing null, as SQL's
                        // `||` does.
                        Value::Null => return Value::Null,
                        other => out.push_str(&format!("{other:?}")),
                    }
                }
                Value::Str(out)
            }
            Self::Lower(inner) => match inner.evaluate(row) {
                Value::Str(s) => Value::Str(s.to_lowercase()),
                _ => Value::Null,
            },
            Self::Upper(inner) => match inner.evaluate(row) {
                Value::Str(s) => Value::Str(s.to_uppercase()),
                _ => Value::Null,
            },
            Self::Extract { unit, value } => match number(&value.evaluate(row)) {
                Some(Number::Int(seconds)) => {
                    Value::I64(seconds.div_euclid(unit.seconds()).rem_euclid(unit.within()))
                }
                _ => Value::Null,
            },
            Self::DateTrunc { unit, value } => match number(&value.evaluate(row)) {
                Some(Number::Int(seconds)) => {
                    Value::I64(seconds.div_euclid(unit.seconds()) * unit.seconds())
                }
                _ => Value::Null,
            },
            Self::Case {
                branches,
                otherwise,
            } => {
                for (when, then) in branches {
                    // Only a definite true takes a branch. An unknown falls
                    // through, the same way it fails to admit a row.
                    if when.admits_over(row) {
                        return then.evaluate(row);
                    }
                }
                otherwise.evaluate(row)
            }
            Self::Coalesce(parts) => {
                for part in parts {
                    let value = part.evaluate(row);
                    if !value.is_null() {
                        return value;
                    }
                }
                Value::Null
            }
            Self::Distance {
                left,
                right,
                metric,
            } => {
                let (Value::Vector(a), Value::Vector(b)) =
                    (left.evaluate(row), right.evaluate(row))
                else {
                    return Value::Null;
                };
                metric.between(&a, &b).map_or(Value::Null, Value::F64)
            }
            Self::RegexpReplace {
                value,
                pattern,
                replacement,
            } => {
                let Value::Str(text) = value.evaluate(row) else {
                    return Value::Null;
                };
                // Through the same cache the predicate uses, which hands back
                // a shared regex rather than a clone: compiling per row is what
                // made ClickBench's regex query take a minute, and matching on
                // a fresh clone is most of what was left. Rewriting the
                // replacement here is not worth hoisting — it measured 16ms
                // per million rows against seconds for the match.
                let Ok(regex) = crate::expr::compile(pattern, false) else {
                    return Value::Null;
                };
                Value::Str(
                    regex
                        .replace_all(&text, backreferences(replacement).as_str())
                        .into_owned(),
                )
            }
        }
    }

    /// Every column this reads, including through a `CASE`'s conditions.
    ///
    /// The planner needs it: a computed value is only as available as its
    /// inputs, so those have to be decoded even when the caller never asked to
    /// see them.
    pub fn collect_columns(&self, out: &mut impl Extend<Ordinal>) {
        match self {
            Self::Column(ordinal) => out.extend(core::iter::once(*ordinal)),
            Self::Literal(_) => {}
            Self::Add(a, b) | Self::Sub(a, b) | Self::Mul(a, b) | Self::Div(a, b) => {
                a.collect_columns(out);
                b.collect_columns(out);
            }
            Self::Length(inner) | Self::Lower(inner) | Self::Upper(inner) => {
                inner.collect_columns(out);
            }
            Self::Extract { value, .. } | Self::DateTrunc { value, .. } => {
                value.collect_columns(out);
            }
            Self::Concat(parts) | Self::Coalesce(parts) => {
                for part in parts {
                    part.collect_columns(out);
                }
            }
            Self::RegexpReplace { value, .. } => value.collect_columns(out),
            Self::Distance { left, right, .. } => {
                left.collect_columns(out);
                right.collect_columns(out);
            }
            Self::Case {
                branches,
                otherwise,
            } => {
                for (when, then) in branches {
                    out.extend(when.columns());
                    then.collect_columns(out);
                }
                otherwise.collect_columns(out);
            }
        }
    }

    /// Every column this reads.
    #[must_use]
    pub fn columns(&self) -> std::collections::BTreeSet<Ordinal> {
        let mut out = std::collections::BTreeSet::new();
        self.collect_columns(&mut out);
        out
    }
}

/// Rewrite SQL's `\1` capture references into the `$1` the regex crate wants.
///
/// Accepting both is not indulgence: `REGEXP_REPLACE` in every SQL dialect is
/// written with backslashes, and a replacement that silently inserted the
/// literal text `\1` into every row would be the kind of wrong answer that
/// looks like a right one. A doubled backslash is an escaped backslash and is
/// left alone.
#[must_use]
pub fn backreferences(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some(d) if d.is_ascii_digit() => {
                    out.push('$');
                    out.push(*d);
                    chars.next();
                }
                Some('\\') => {
                    out.push('\\');
                    chars.next();
                }
                _ => out.push('\\'),
            },
            // A literal `$` in the replacement would otherwise be read as the
            // start of a group reference.
            '$' => out.push_str("$$"),
            other => out.push(other),
        }
    }
    out
}
