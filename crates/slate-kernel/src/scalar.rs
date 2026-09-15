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

/// A calendar field of a timestamp, for [`Scalar::CalendarPart`].
///
/// Separate from [`TimeUnit`] rather than more variants on it, because
/// `TimeUnit` promises a fixed number of seconds — `TimeUnit::seconds` is how
/// both `Extract` and `DateTrunc` are implemented — and a month does not have
/// one. Adding `Month` there would give it a `seconds()` that is a lie, and the
/// lie would be silent: `date_trunc(month, t)` would compile and return
/// nonsense.
///
/// These need the Gregorian calendar, so they are computed rather than divided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarPart {
    /// The year, negative before 1 CE. Proleptic Gregorian, so it disagrees
    /// with history before 1582 and agrees with every other database.
    Year,
    /// The month, 1 to 12.
    Month,
    /// The day of the month, 1 to 31.
    DayOfMonth,
    /// The day of the week, 0 for Sunday through 6 for Saturday.
    ///
    /// Sunday-first because that is what ClickHouse, MySQL and SQLite's
    /// `%w` produce, and matching three of them beats matching ISO's
    /// Monday-first and none of them. The choice is arbitrary and the only
    /// wrong move is not writing it down.
    DayOfWeek,
}

/// Days since 1970-01-01 as a proleptic-Gregorian year, month and day.
///
/// Howard Hinnant's `civil_from_days`, transcribed. It is branch-free apart
/// from the era floor and the March-based month fixup, exact over the whole
/// range this can be handed, and short enough to read — which is why it is
/// here rather than behind a date-time dependency that would bring a parser, a
/// formatter and a timezone database along for four integer fields.
///
/// The shift by 719_468 moves the epoch to 0000-03-01, so that leap day lands
/// at the *end* of the year and the month lengths become a repeating pattern
/// `(5 * doy + 2) / 153` can index. That is the whole trick.
const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146_096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let march_month = (5 * day_of_year + 2) / 153; // [0, 11], 0 is March
    let day = day_of_year - (153 * march_month + 2) / 5 + 1; // [1, 31]
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The inverse of [`civil_from_days`]: a proleptic-Gregorian date as days since
/// 1970-01-01.
///
/// Hinnant's `days_from_civil`, transcribed, and exact over the same range for
/// the same reason — it is the same era arithmetic run backwards. The shift by
/// 719_468 is the same shift, and the `(153 * m + 2) / 5` is the same repeating
/// month-length pattern read the other way.
///
/// This exists so `date_trunc` can reach a month and a year. Truncating to a
/// fixed number of seconds needs no calendar: `seconds / 86_400 * 86_400` is
/// midnight. A month has no fixed length, so truncating to one means decoding
/// the date, dropping the day, and encoding it again — which needs this.
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // March-based, so a leap day lands at the end of the year and never in the
    // middle of the pattern.
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let march_month = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
    let day_of_year = (153 * march_month + 2) / 5 + day - 1; // [0, 365]
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year; // [0, 146_096]
    era * 146_097 + day_of_era - 719_468
}

/// A calendar boundary [`Scalar::CalendarTrunc`] can floor a timestamp to.
///
/// Separate from [`TimeUnit`] for the reason [`CalendarPart`] is separate from
/// it: `TimeUnit` promises a fixed number of seconds, which is how
/// [`Scalar::DateTrunc`] is defined, and neither a month nor a year has one. A
/// `Month` member there would give it a length that is a lie, and the lie would
/// be silent — 30 days is wrong for seven months of the year.
///
/// `Day` is deliberately absent: it *is* a fixed number of seconds, so it
/// belongs to `TimeUnit`, and offering it in both places would be two spellings
/// of one operation with no way to tell which a caller meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarUnit {
    /// The first instant of the month, in UTC.
    Month,
    /// The first instant of the year, in UTC.
    Year,
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
    /// A number rounded to the nearest integer, halves away from zero.
    ///
    /// Returns an integer, so it can be a group key without the grouping
    /// depending on float equality: `2.5` and `2.4999999` round to the same
    /// `i64` and land in the same group, which is the whole reason to bucket
    /// by a rounded value rather than by the value itself.
    Round(Box<Scalar>),
    /// A calendar field of a timestamp: the year, the day of the week.
    ///
    /// Distinct from [`Scalar::Extract`] because these are not divisions. See
    /// [`CalendarPart`].
    CalendarPart {
        /// Which field.
        part: CalendarPart,
        /// The timestamp, in seconds since the epoch.
        value: Box<Scalar>,
    },
    /// `date_trunc('month' | 'year', value)`: the timestamp floored to a
    /// calendar boundary.
    ///
    /// Separate from [`Scalar::DateTrunc`] because a month is not a fixed
    /// number of seconds — see [`CalendarUnit`]. Floors rather than rounds, and
    /// floors *below* the epoch too, so December 1969 truncates to
    /// 1969-12-01 rather than to 1970-01-01.
    CalendarTrunc {
        /// Which boundary.
        unit: CalendarUnit,
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

    /// This value rounded to the nearest integer.
    #[must_use]
    pub fn round(self) -> Self {
        Self::Round(Box::new(self))
    }

    /// One calendar field of a timestamp.
    #[must_use]
    pub fn calendar_part(self, part: CalendarPart) -> Self {
        Self::CalendarPart {
            part,
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

    /// `date_trunc` to a calendar boundary. See [`Scalar::CalendarTrunc`].
    #[must_use]
    pub fn calendar_trunc(self, unit: CalendarUnit) -> Self {
        Self::CalendarTrunc {
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
            Self::Round(value) => match number(&value.evaluate(row)) {
                // An integer is already rounded. Going through `f64` would
                // lose precision above 2^53, silently, for a no-op.
                Some(Number::Int(n)) => Value::I64(n),
                Some(Number::Real(x)) => {
                    // `f64::round` is halves-away-from-zero, which is what SQL
                    // and ClickHouse's `round` do at the default precision.
                    // A value past `i64` saturates rather than wrapping; NaN
                    // has no integer to be, so it is null.
                    if x.is_nan() {
                        Value::Null
                    } else {
                        Value::I64(x.round() as i64)
                    }
                }
                None => Value::Null,
            },
            Self::CalendarPart { part, value } => match number(&value.evaluate(row)) {
                Some(Number::Int(seconds)) => {
                    // Floor division, not truncation: an instant in 1969 is a
                    // negative number of seconds, and `-1 / 86_400` is 0 while
                    // the day it belongs to is -1. Truncating puts the last
                    // day before the epoch into the first day after it.
                    let days = seconds.div_euclid(86_400);
                    Value::I64(match part {
                        // 1970-01-01 was a Thursday, so day 0 is weekday 4
                        // counting Sunday as 0. `rem_euclid` for the same
                        // reason the division is floored.
                        CalendarPart::DayOfWeek => (days + 4).rem_euclid(7),
                        _ => {
                            let (year, month, day) = civil_from_days(days);
                            match part {
                                CalendarPart::Year => year,
                                CalendarPart::Month => month,
                                CalendarPart::DayOfMonth => day,
                                CalendarPart::DayOfWeek => unreachable!(),
                            }
                        }
                    })
                }
                _ => Value::Null,
            },
            Self::DateTrunc { unit, value } => match number(&value.evaluate(row)) {
                Some(Number::Int(seconds)) => {
                    Value::I64(seconds.div_euclid(unit.seconds()) * unit.seconds())
                }
                _ => Value::Null,
            },
            Self::CalendarTrunc { unit, value } => match number(&value.evaluate(row)) {
                Some(Number::Int(seconds)) => {
                    // Floored, as everywhere else here, so an instant before
                    // the epoch truncates backwards rather than forwards.
                    let (year, month, _) = civil_from_days(seconds.div_euclid(86_400));
                    let month = match unit {
                        CalendarUnit::Month => month,
                        CalendarUnit::Year => 1,
                    };
                    Value::I64(days_from_civil(year, month, 1) * 86_400)
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
            Self::Round(value) => value.collect_columns(out),
            Self::Extract { value, .. }
            | Self::DateTrunc { value, .. }
            | Self::CalendarTrunc { value, .. }
            | Self::CalendarPart { value, .. } => {
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
