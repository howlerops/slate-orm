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

use crate::error::KernelError;
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

/// A non-string value as the text [`Scalar::Concat`] should splice in.
///
/// `None` for a value with no textual form anyone means, which makes the whole
/// concatenation null — the same answer a type error gets everywhere else here.
///
/// This existed as `format!("{other:?}")`, which is the *Debug* form: a
/// `u64` book id concatenated into a label came out as `U64(10)` rather than
/// `10`, and a double as `F64(1.5)`. It was found by a client test asserting
/// `ada/a-one/10` and getting `ada/a-one/U64(10)` — nothing in the Rust suites
/// concatenated a non-string, so the bug had never been looked at. SQL's `||`
/// renders a number as its digits, and a label with a type tag in it is wrong
/// in a way nobody would think to check for.
///
/// Bytes and vectors are null rather than rendered. A byte string has no
/// canonical text — hex and base64 are both defensible and neither is what a
/// caller silently wants spliced into a label — and a vector's textual form
/// would be a hundred floats. SQL refuses both outright; null is this layer's
/// way of saying the same thing, since a `Scalar` has nowhere to put an error.
fn concat_text(value: &Value) -> Option<String> {
    Some(match value {
        Value::Str(s) => s.clone(),
        Value::I64(n) => n.to_string(),
        Value::U64(n) => n.to_string(),
        // Rust's `Display` for `f64` gives `1.5` and `1` rather than SQL's
        // `1.0`. Left as Rust spells it: the alternative is a float formatter
        // in here, and this is a label rather than a serialisation format.
        Value::F64(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Uuid(id) => id.to_string(),
        Value::Null | Value::Bytes(_) | Value::Vector(_) => return None,
        // `Value` is `#[non_exhaustive]`, so a new variant lands here rather
        // than failing to compile. Null is the right default for one: a value
        // this does not know how to render is a value it must not guess at.
        _ => return None,
    })
}

/// The offset a named zone is from UTC at an instant, in seconds east.
///
/// A binary search over that zone's transition table. The table is generated —
/// see [`crate::zones`] — and its first entry is the window's start, so a
/// lookup before any real transition still finds an offset rather than falling
/// off the front.
///
/// `None` for a name the table does not have, which is what makes an unknown
/// zone a refusal rather than a guess.
fn zone_offset(name: &str, instant: i64) -> Option<i32> {
    let zone = crate::zones::ZONES
        .binary_search_by(|zone| zone.name.cmp(name))
        .ok()
        .and_then(|at| crate::zones::ZONES.get(at))?;
    // The last transition at or before the instant. `partition_point` gives the
    // count of entries strictly before the first one *after* it, so subtracting
    // one lands on the entry in force — and the table is never empty, so the
    // saturating subtraction only matters for an instant before the window,
    // where entry zero is the right answer anyway.
    let at = zone
        .transitions
        .partition_point(|(when, _)| *when <= instant)
        .saturating_sub(1);
    zone.transitions.get(at).map(|(_, offset)| *offset)
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
    /// A UTC timestamp read as local time in a named zone.
    ///
    /// Adds the zone's offset *at that instant*, so the result is "local
    /// seconds": feed it to [`Scalar::Extract`], [`Scalar::CalendarPart`] or
    /// either truncation and every one of them reads the local wall clock with
    /// no change of its own. That is the same arrangement a fixed offset uses —
    /// `Scalar::Add` of a constant — generalised from a constant to a table
    /// lookup, which is the whole difference between an offset and a zone.
    ///
    /// One variant rather than a zone field on each of the four calendar
    /// scalars, for the reason the fixed offset needed none: the shift composes,
    /// so the planner, the wire, the covering scan and the round-trip property
    /// handle it already.
    ///
    /// A zone the table does not have evaluates to null. The refusal a caller
    /// should see belongs at the edge — the SQL front end and the clients name
    /// the zones that exist — because a `Scalar` has nowhere to put an error.
    ZoneShift {
        /// The IANA name, as [`crate::zones`] spells it.
        zone: String,
        /// The timestamp, in seconds since the epoch, UTC.
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

/// The one scale a list of operands agrees on, or `None` if they disagree.
///
/// `Some(None)` is "none of them is a decimal", `Some(Some(s))` is "all the
/// decimals among them are at scale `s`", and `None` is a disagreement. The
/// nesting is ugly and the alternative — a three-state enum used in two
/// places — was uglier.
fn agree(scales: &[Option<u8>]) -> Option<Option<u8>> {
    let mut seen: Option<u8> = None;
    for scale in scales.iter().flatten() {
        match seen {
            None => seen = Some(*scale),
            Some(first) if first == *scale => {}
            Some(_) => return None,
        }
    }
    Some(seen)
}

/// Refuse every computed expression in `compute` whose decimal result has no
/// scale to be at.
///
/// # Why a list rather than one expression at a time
///
/// Because a computed value may read an *earlier* computed value, and the
/// scale of that one is not in any schema — it is whatever
/// [`Scalar::decimal_scale`] said about the expression that produced it. So
/// the scales are worked out left to right and each expression is checked
/// against the columns plus the answers already given.
///
/// Doing this one expression at a time, with a `scale_of` that knows only the
/// table, would report `price + computed_price` as "a decimal added to a plain
/// number" — a refusal of something that is perfectly well defined. That was
/// the first version and the test that caught it is
/// `compute_reading_an_earlier_computed_decimal`.
///
/// `columns` is where the table's own ordinals stop and the computed ones
/// begin; `column_scale` answers for the ordinals below it.
///
/// # Errors
/// [`KernelError::DecimalScale`], from the first expression that has no scale.
pub(crate) fn check_scales(
    at: &str,
    columns: usize,
    column_scale: &dyn Fn(Ordinal) -> Option<u8>,
    compute: &[Scalar],
) -> Result<(), KernelError> {
    if compute.is_empty() {
        return Ok(());
    }
    let mut computed: Vec<Option<u8>> = Vec::with_capacity(compute.len());
    for expression in compute {
        let scale_of = |ordinal: Ordinal| {
            if ordinal.0 < columns {
                column_scale(ordinal)
            } else {
                // An ordinal past the computed values this far is out of range
                // and has its own refusal elsewhere; here it is simply not a
                // decimal, which is what an unreadable ordinal evaluates to.
                computed.get(ordinal.0 - columns).copied().flatten()
            }
        };
        let scale = expression.decimal_scale(at, &scale_of)?;
        computed.push(scale);
    }
    Ok(())
}

/// One side of an arithmetic operation, once it is known to be a number.
enum Number {
    Int(i64),
    Real(f64),
    /// A count of a decimal column's smallest unit. The scale is not here —
    /// it is the column's — which is exactly why the rules in [`decimal_op`]
    /// are what they are.
    Units(i64),
}

fn number(value: &Value) -> Option<Number> {
    match value {
        Value::I64(v) => Some(Number::Int(*v)),
        Value::U64(v) => i64::try_from(*v).ok().map(Number::Int),
        Value::F64(v) => Some(Number::Real(*v)),
        Value::Decimal(v) => Some(Number::Units(*v)),
        _ => None,
    }
}

/// Which arithmetic operation is being applied, for the decimal rules.
///
/// The four ops share [`arithmetic`] because on integers and reals they differ
/// only by the closure. Decimals are the case where they do not: multiplying
/// two decimals changes the scale and adding them does not, so the operation
/// has to be nameable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// Arithmetic where at least one side is a decimal.
///
/// # Why this is not just "convert to the widest type"
///
/// A [`Value::Decimal`] is a count of the column's smallest unit and carries no
/// scale. So the question for every operation is not "what type comes out" but
/// **"is the answer still a count of the same unit?"** — because if it is not,
/// there is nowhere to put the new scale. Nothing on the wire carries one,
/// nothing in a `Row` carries one, and a computed column that silently changed
/// scale would be a number a hundred times wrong with no error anywhere.
///
/// That question has a clean answer per operation:
///
/// - **`units ± units` keeps the unit.** 1250 + 250 cents is 1500 cents.
/// - **`units × n` keeps the unit** for a whole number `n`. Twelve items at
///   1250 cents is 15000 cents — which is `price * quantity`, the example the
///   README has carried as a gap since decimals were built.
/// - **`units ÷ n` keeps the unit**, truncating toward zero. See below.
/// - **`units × units` does not.** Scale 2 times scale 2 is scale 4, and there
///   is no scale 4 to write it into.
/// - **anything with a float does not**, because the whole point of the type
///   is that it is not a float.
///
/// The refusals are enforced at plan time by [`Scalar::decimal_scale`], which
/// can see the schema. This function is the evaluator, which cannot, so it
/// returns `Null` for the cases that check refuses rather than inventing an
/// answer — belt and braces, not the real guard.
///
/// # Truncation, stated rather than assumed
///
/// `units ÷ n` truncates *toward zero*, which is `i64`'s `/`. A 999-cent total
/// split three ways is 333 each and a cent unaccounted for. Rounding half away
/// from zero was considered and rejected: it makes the parts sum to more than
/// the whole as often as not, and a caller splitting money wants to see the
/// remainder rather than have it invented. `Scalar::round` exists for callers
/// who want the other behaviour on a float.
fn decimal_op(op: Op, a: &Number, b: &Number) -> Value {
    match (op, a, b) {
        (Op::Add, Number::Units(x), Number::Units(y)) => {
            x.checked_add(*y).map_or(Value::Null, Value::Decimal)
        }
        (Op::Sub, Number::Units(x), Number::Units(y)) => {
            x.checked_sub(*y).map_or(Value::Null, Value::Decimal)
        }
        (Op::Mul, Number::Units(x), Number::Int(y))
        | (Op::Mul, Number::Int(x), Number::Units(y)) => {
            x.checked_mul(*y).map_or(Value::Null, Value::Decimal)
        }
        (Op::Div, Number::Units(x), Number::Int(y)) => {
            x.checked_div(*y).map_or(Value::Null, Value::Decimal)
        }
        // Every remaining shape is one `decimal_scale` refuses: two decimals
        // multiplied or divided, a decimal against a float, an integer divided
        // by a decimal. `Null` rather than a panic because this is a per-row
        // path and the real refusal happened before the scan started.
        _ => Value::Null,
    }
}

/// Apply an operation to two values, in whichever domain they share.
///
/// Integers stay integers unless one side is real, which is what keeps
/// `width + 1` an integer instead of quietly becoming a float. Overflow
/// saturates rather than wrapping: a sum that is too large is better reported
/// as the largest thing than as a small negative one.
fn arithmetic(
    which: Op,
    a: &Value,
    b: &Value,
    op: fn(i64, i64) -> Option<i64>,
    real: fn(f64, f64) -> f64,
) -> Value {
    match (number(a), number(b)) {
        (Some(Number::Int(x)), Some(Number::Int(y))) => op(x, y).map_or(Value::Null, Value::I64),
        // Before the widening arm below, and that order is the whole point: a
        // decimal widened to `f64` would come back as a float, which is the
        // silent loss the type exists to prevent.
        (Some(x), Some(y)) if matches!(x, Number::Units(_)) || matches!(y, Number::Units(_)) => {
            decimal_op(which, &x, &y)
        }
        (Some(x), Some(y)) => {
            let to_f = |n: Number| match n {
                Number::Int(v) => v as f64,
                Number::Real(v) => v,
                // Unreachable: the arm above catches every pair with a
                // decimal in it. Mapped rather than panicked so a future
                // variant cannot turn a query into a crash.
                Number::Units(v) => v as f64,
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

    /// Read this UTC timestamp as local time in `zone`. See
    /// [`Scalar::ZoneShift`].
    #[must_use]
    pub fn in_zone(self, zone: impl Into<String>) -> Self {
        Self::ZoneShift {
            zone: zone.into(),
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

    /// The scale this expression's value is at, if it is a decimal at all.
    ///
    /// `Ok(None)` means "not a decimal" — an integer, a string, a timestamp,
    /// anything the scale question does not apply to. `Ok(Some(s))` means a
    /// count of a unit at scale `s`. An error means the expression *is* about
    /// decimals and its result has no scale to be at; see
    /// [`KernelError::DecimalScale`] for why that is a refusal rather than an
    /// answer.
    ///
    /// # Why this exists, and why it is here and not in the evaluator
    ///
    /// The evaluator sees a [`Row`](slate_schema::Row) and no schema, so it
    /// cannot know that ordinal 3 is scale 2. The schema is what knows, and
    /// the schema is available exactly once — when a query is planned against
    /// a table. So the refusals happen there, before a row is read, and
    /// `decimal_op` in the evaluator is belt and braces.
    ///
    /// # The rule, in one sentence
    ///
    /// An expression is expressible when its answer is still a count of the
    /// *same* unit its operands were counts of.
    ///
    /// # `scale_of`
    ///
    /// Maps an ordinal to its scale, or `None` for a column that is not a
    /// decimal. A closure rather than a `&TableDef` because the ordinals a
    /// joined query computes over span two tables, and the caller is the only
    /// thing that knows which side an ordinal fell on.
    ///
    /// # Errors
    /// [`KernelError::DecimalScale`], naming the operation and what to write
    /// instead.
    pub fn decimal_scale(
        &self,
        at: &str,
        scale_of: &dyn Fn(Ordinal) -> Option<u8>,
    ) -> Result<Option<u8>, KernelError> {
        let refuse = |what: &str, why: &str| {
            Err(KernelError::DecimalScale {
                at: at.to_owned(),
                what: what.to_owned(),
                why: why.to_owned(),
            })
        };
        match self {
            Self::Column(ordinal) => Ok(scale_of(*ordinal)),
            // Every literal, a decimal one included, is *scale-agnostic* —
            // which is the same thing a numeric literal is in SQL.
            // `Value::Decimal(100)` is a hundred units, and which number that
            // stands for is the column's business, so `price + Decimal(100)`
            // is scale 2 because `price` is.
            //
            // A separate `Literal(Value::Decimal(_))` arm was here first,
            // returning the same `Ok(None)` so that the comment had somewhere
            // to sit. Deleting it changed no test, because the catch-all
            // already answered identically — an equivalent mutation, and a
            // second arm nobody could break is a second arm that can drift.
            // What actually makes a decimal literal special is
            // `mentions_decimal`, which is how `Decimal(1) + Decimal(2)` —
            // a decimal at a scale nobody stated — is told apart from `1 + 2`.
            Self::Literal(_) => Ok(None),

            Self::Add(a, b) | Self::Sub(a, b) => {
                let (x, y) = (
                    a.decimal_scale(at, scale_of)?,
                    b.decimal_scale(at, scale_of)?,
                );
                match (x, y, self.mentions_decimal(scale_of)) {
                    // Neither side is a decimal: ordinary arithmetic, and the
                    // scale question does not arise.
                    (None, None, false) => Ok(None),
                    (Some(p), Some(q), _) if p == q => Ok(Some(p)),
                    (Some(p), Some(q), _) => refuse(
                        "adding or subtracting decimals at different scales",
                        &format!(
                            "one side is at scale {p} and the other at scale {q}, and the \
                             result would be a count of neither unit; rescale one of them \
                             in the caller, where the intended scale is known"
                        ),
                    ),
                    // One side is a decimal column and the other is a bare
                    // literal or another decimal-agnostic term: it adopts the
                    // column's scale.
                    (Some(p), None, _) | (None, Some(p), _) if self.other_side_is_agnostic() => {
                        Ok(Some(p))
                    }
                    (Some(_), None, _) | (None, Some(_), _) => refuse(
                        "adding or subtracting a decimal and a number that is not one",
                        "an integer or a float beside a decimal has no unit, so the sum \
                         would be a count of nothing; write the other side as a decimal, \
                         whose value is a count of the column's smallest unit",
                    ),
                    (None, None, true) => refuse(
                        "adding or subtracting decimals with no column to take a scale from",
                        "both sides are literals, so there is no column to say what unit \
                         they count; compare or combine against a decimal column",
                    ),
                }
            }

            Self::Mul(a, b) => {
                let (x, y) = (
                    a.decimal_scale(at, scale_of)?,
                    b.decimal_scale(at, scale_of)?,
                );
                // Which side *mentions* a decimal, separately from which side
                // has a scale. A decimal literal has no scale of its own and
                // is still money, and `Add` has always asked this — `Mul` and
                // `Div` did not, which is how `price * Decimal(3)` came to
                // plan cleanly and evaluate to null on every row. See
                // `money_times_a_decimal_literal_is_refused_not_nulled`.
                let (ma, mb) = (a.mentions_decimal(scale_of), b.mentions_decimal(scale_of));
                let literal_units = "one side is a decimal literal, which counts the other \
                     side's smallest unit rather than whole things — so this is units times \
                     units, at a scale nothing carries. Multiply by a whole number instead";
                match (x, y) {
                    (Some(p), Some(q)) => refuse(
                        "multiplying two decimals",
                        &format!(
                            "scale {p} times scale {q} is scale {}, and nothing in a row, \
                             an index entry or the protocol carries a scale — so the \
                             answer would be a number {} times wrong with no error \
                             anywhere. Multiply by a whole number instead",
                            u32::from(p) + u32::from(q),
                            10u64.saturating_pow(u32::from(q.min(18)))
                        ),
                    ),
                    // `price * quantity`: a whole number of things at a price
                    // is still a count of the same unit.
                    (Some(p), None) if !mb => Ok(Some(p)),
                    (None, Some(p)) if !ma => Ok(Some(p)),
                    (Some(_), None) | (None, Some(_)) => {
                        refuse("multiplying two decimals", literal_units)
                    }
                    (None, None) if !ma && !mb => Ok(None),
                    (None, None) => refuse("multiplying two decimals", literal_units),
                }
            }

            Self::Div(a, b) => {
                let (x, y) = (
                    a.decimal_scale(at, scale_of)?,
                    b.decimal_scale(at, scale_of)?,
                );
                // The same two questions `Mul` asks, for the same reason:
                // `price / Decimal(4)` divides by four *hundredths*, not by
                // four, and used to plan as the second one.
                let (ma, mb) = (a.mentions_decimal(scale_of), b.mentions_decimal(scale_of));
                let literal_units = "the denominator is a decimal literal, which counts the \
                     numerator's smallest unit rather than whole parts — so the answer is a \
                     ratio, not a count of anything. Divide by a whole number instead";
                match (x, y) {
                    (Some(p), Some(q)) => refuse(
                        "dividing one decimal by another",
                        &format!(
                            "scale {p} over scale {q} is a ratio, which is not a count of \
                             any unit; divide by a whole number instead"
                        ),
                    ),
                    (None, Some(_)) => refuse(
                        "dividing by a decimal",
                        "a count of units in the denominator gives an answer in no unit \
                         at all; divide by a whole number instead",
                    ),
                    // `total / parts`, truncating toward zero. See `decimal_op`.
                    (Some(p), None) if !mb => Ok(Some(p)),
                    (Some(_), None) => refuse("dividing by a decimal", literal_units),
                    (None, None) if !ma && !mb => Ok(None),
                    (None, None) => refuse("dividing by a decimal", literal_units),
                }
            }

            // Rounding a decimal means rounding *to its scale*, and `round`
            // returns an `i64` — so the answer would be either the units
            // unchanged (a no-op dressed as a rounding) or a number this
            // function cannot compute. Refused rather than given one of those.
            Self::Round(value) => match value.decimal_scale(at, scale_of)? {
                None => Ok(None),
                Some(_) => refuse(
                    "rounding a decimal",
                    "`round` yields an integer, and rounding a decimal to a whole number \
                     means dividing by its scale — which this expression cannot see. \
                     Divide by the scale explicitly if that is what was meant",
                ),
            },

            // A decimal has no characters, no case and no calendar. These
            // already evaluate to null for one; the refusal says so at plan
            // time instead, which is the difference between "no rows" and "you
            // wrote something that cannot mean anything".
            Self::Length(inner) | Self::Lower(inner) | Self::Upper(inner) => {
                match inner.decimal_scale(at, scale_of)? {
                    None => Ok(None),
                    Some(_) => refuse(
                        "a string function over a decimal",
                        "a decimal is a number, not text",
                    ),
                }
            }
            Self::Extract { value, .. }
            | Self::CalendarPart { value, .. }
            | Self::CalendarTrunc { value, .. }
            | Self::DateTrunc { value, .. }
            | Self::ZoneShift { value, .. } => match value.decimal_scale(at, scale_of)? {
                None => Ok(None),
                Some(_) => refuse(
                    "a calendar function over a decimal",
                    "a timestamp here is seconds since the epoch, as an `i64`; a decimal \
                     is a count of a currency-like unit and is not one",
                ),
            },

            // Every branch has to agree, for the reason two sides of an `Add`
            // do: the column that results has one scale, and a `CASE` that
            // produced scale 2 for some rows and scale 4 for others would be a
            // column whose meaning varied by row.
            Self::Case {
                branches,
                otherwise,
            } => {
                let mut scales = Vec::with_capacity(branches.len() + 1);
                for (_, then) in branches {
                    scales.push(then.decimal_scale(at, scale_of)?);
                }
                scales.push(otherwise.decimal_scale(at, scale_of)?);
                agree(&scales).map_or_else(
                    || {
                        refuse(
                            "a `case` whose branches are decimals at different scales",
                            "the column it produces has one scale, so its meaning would \
                             vary by row",
                        )
                    },
                    Ok,
                )
            }
            Self::Coalesce(parts) => {
                let mut scales = Vec::with_capacity(parts.len());
                for part in parts {
                    scales.push(part.decimal_scale(at, scale_of)?);
                }
                agree(&scales).map_or_else(
                    || {
                        refuse(
                            "a `coalesce` of decimals at different scales",
                            "the column it produces has one scale, so its meaning would \
                             vary by row",
                        )
                    },
                    Ok,
                )
            }

            // Not decimals, and their arguments cannot usefully be: a distance
            // is between vectors and a replacement is over text. Left to the
            // evaluator's existing nulls rather than given a refusal apiece,
            // because neither has a form where a decimal is plausible enough
            // for a caller to have meant it.
            Self::Concat(_) | Self::Distance { .. } | Self::RegexpReplace { .. } => Ok(None),
        }
    }

    /// Whether either side of a two-sided operator mentions a decimal at all,
    /// including a bare literal one.
    ///
    /// `decimal_scale` returns `None` for a literal decimal because a literal
    /// adopts the scale beside it. That makes "neither side is a decimal" and
    /// "both sides are scale-agnostic decimal literals" indistinguishable from
    /// its return value alone, and the second is a refusal.
    fn mentions_decimal(&self, scale_of: &dyn Fn(Ordinal) -> Option<u8>) -> bool {
        match self {
            Self::Literal(Value::Decimal(_)) => true,
            Self::Column(ordinal) => scale_of(*ordinal).is_some(),
            Self::Add(a, b) | Self::Sub(a, b) | Self::Mul(a, b) | Self::Div(a, b) => {
                a.mentions_decimal(scale_of) || b.mentions_decimal(scale_of)
            }
            _ => false,
        }
    }

    /// Whether one side of a sum is a bare decimal literal, which takes its
    /// scale from the other side.
    ///
    /// This was written recursively, so that an expression built only from
    /// decimal literals counted too — `price + (Decimal(1) + Decimal(2))`.
    /// The recursion is unreachable: `decimal_scale` works bottom up, so the
    /// inner sum is asked first, has two scale-agnostic sides and no column,
    /// and is refused there. A mutation flipping the recursive `&&` to `||`
    /// changed no test, which is what said the arm was dead; it is gone rather
    /// than left for someone to maintain.
    ///
    /// Anything more structured than a literal — a `case` whose branches are
    /// all decimal literals, say — is *not* agnostic here, and a sum with one
    /// is refused. Conservative on purpose: the refusal says to write the
    /// other side as a decimal, and a caller can always hoist the literal.
    fn other_side_is_agnostic(&self) -> bool {
        let agnostic = |side: &Self| matches!(side, Self::Literal(Value::Decimal(_)));
        match self {
            Self::Add(a, b) | Self::Sub(a, b) => agnostic(a) || agnostic(b),
            _ => false,
        }
    }

    /// Compute this over a row.
    #[must_use]
    pub fn evaluate<C: Columns + ?Sized>(&self, row: &C) -> Value {
        match self {
            Self::Column(ordinal) => row.value(*ordinal).cloned().unwrap_or(Value::Null),
            Self::Literal(value) => value.clone(),
            Self::Add(a, b) => arithmetic(
                Op::Add,
                &a.evaluate(row),
                &b.evaluate(row),
                i64::checked_add,
                |x, y| x + y,
            ),
            Self::Sub(a, b) => arithmetic(
                Op::Sub,
                &a.evaluate(row),
                &b.evaluate(row),
                i64::checked_sub,
                |x, y| x - y,
            ),
            Self::Mul(a, b) => arithmetic(
                Op::Mul,
                &a.evaluate(row),
                &b.evaluate(row),
                i64::checked_mul,
                |x, y| x * y,
            ),
            // Division by zero is null, not a panic and not an error: a query
            // over a million rows should not fail because one of them held a
            // zero.
            Self::Div(a, b) => arithmetic(
                Op::Div,
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
                        other => match concat_text(&other) {
                            Some(text) => out.push_str(&text),
                            None => return Value::Null,
                        },
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
                // Null rather than the units, and rather than a guess at the
                // number they stand for. `round` returns an `i64` so that a
                // group key does not depend on float equality; rounding a
                // decimal means rounding *to its scale*, which this function
                // cannot see — `Decimal(1250)` at scale 2 rounds to 13 and at
                // scale 0 to 1250, and answering one of those would be wrong
                // half the time. `Scalar::decimal_scale` refuses it at plan
                // time and says so; this arm is what the evaluator does with
                // one that got past a caller who built the expression by hand.
                Some(Number::Units(_)) => Value::Null,
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
            Self::ZoneShift { zone, value } => match number(&value.evaluate(row)) {
                Some(Number::Int(seconds)) => match zone_offset(zone, seconds) {
                    // Saturating, like the rest of the arithmetic here: a zone
                    // shift at the very end of `i64` should not wrap into the
                    // distant past.
                    Some(offset) => Value::I64(seconds.saturating_add(i64::from(offset))),
                    None => Value::Null,
                },
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
            | Self::ZoneShift { value, .. }
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
