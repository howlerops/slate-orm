//! Mapping Rust types onto the record layer's closed value model.
//!
//! [`Field`] is what lets the derive macro work without parsing types. A field's
//! column type and nullability come from associated constants, so
//! `Option<String>` declares itself nullable and the macro never has to
//! recognise the token `Option` — which also means a type alias, or a
//! user-defined newtype with its own `Field` impl, behaves correctly.

use bytes::Bytes;
use slate_tuple::{Value, ValueType};
use uuid::Uuid;

/// A value that did not fit the column it was read from or written to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FieldError {
    /// The stored value had the wrong type for this Rust type.
    #[error("expected {expected}, found {found}")]
    TypeMismatch {
        /// The type the Rust field wanted.
        expected: ValueType,
        /// The type actually stored.
        found: &'static str,
    },

    /// A null was read into a non-optional field.
    #[error("read a null into a non-optional field of type {expected}")]
    UnexpectedNull {
        /// The type the Rust field wanted.
        expected: ValueType,
    },

    /// The value is of the right type but outside this Rust type's range.
    #[error("value out of range for {target}")]
    OutOfRange {
        /// Name of the Rust type.
        target: &'static str,
    },
}

/// A Rust type that can be stored in one column.
pub trait Field: Sized {
    /// The column type this maps to.
    const VALUE_TYPE: ValueType;
    /// Whether the column must accept nulls.
    const NULLABLE: bool;

    /// Convert to a stored value.
    fn to_value(&self) -> Value;

    /// Convert back from a stored value.
    ///
    /// # Errors
    /// If the stored value has the wrong type, is null in a non-optional field,
    /// or falls outside this type's range.
    fn from_value(value: &Value) -> Result<Self, FieldError>;
}

/// Integers all share one encoding, so a narrow Rust type is a range check on a
/// wide stored value rather than a different column type.
macro_rules! integer_field {
    ($($ty:ty => $variant:ident, $value_type:expr);* $(;)?) => {
        $(impl Field for $ty {
            const VALUE_TYPE: ValueType = $value_type;
            const NULLABLE: bool = false;

            fn to_value(&self) -> Value {
                Value::$variant((*self).into())
            }

            fn from_value(value: &Value) -> Result<Self, FieldError> {
                match value {
                    Value::$variant(v) => Self::try_from(*v).map_err(|_| FieldError::OutOfRange {
                        target: stringify!($ty),
                    }),
                    Value::Null => Err(FieldError::UnexpectedNull {
                        expected: Self::VALUE_TYPE,
                    }),
                    other => Err(FieldError::TypeMismatch {
                        expected: Self::VALUE_TYPE,
                        found: other.type_name(),
                    }),
                }
            }
        })*
    };
}

integer_field! {
    i8 => I64, ValueType::I64;
    i16 => I64, ValueType::I64;
    i32 => I64, ValueType::I64;
    u8 => U64, ValueType::U64;
    u16 => U64, ValueType::U64;
    u32 => U64, ValueType::U64;
}

/// Types whose stored representation is exactly the Rust value.
macro_rules! exact_field {
    ($($ty:ty => $variant:ident, $value_type:expr, $to:expr, $from:expr);* $(;)?) => {
        $(impl Field for $ty {
            const VALUE_TYPE: ValueType = $value_type;
            const NULLABLE: bool = false;

            fn to_value(&self) -> Value {
                #[allow(clippy::redundant_closure_call)]
                Value::$variant(($to)(self))
            }

            fn from_value(value: &Value) -> Result<Self, FieldError> {
                match value {
                    #[allow(clippy::redundant_closure_call)]
                    Value::$variant(v) => Ok(($from)(v)),
                    Value::Null => Err(FieldError::UnexpectedNull {
                        expected: Self::VALUE_TYPE,
                    }),
                    other => Err(FieldError::TypeMismatch {
                        expected: Self::VALUE_TYPE,
                        found: other.type_name(),
                    }),
                }
            }
        })*
    };
}

exact_field! {
    bool => Bool, ValueType::Bool, |v: &bool| *v, |v: &bool| *v;
    i64 => I64, ValueType::I64, |v: &i64| *v, |v: &i64| *v;
    u64 => U64, ValueType::U64, |v: &u64| *v, |v: &u64| *v;
    f64 => F64, ValueType::F64, |v: &f64| *v, |v: &f64| *v;
    String => Str, ValueType::Str, |v: &String| v.clone(), |v: &String| v.clone();
    Bytes => Bytes, ValueType::Bytes, |v: &Bytes| v.clone(), |v: &Bytes| v.clone();
    Uuid => Uuid, ValueType::Uuid, |v: &Uuid| *v, |v: &Uuid| *v;
}

/// A count of a decimal column's smallest unit.
///
/// A newtype rather than a bare `i64`, and that is the whole point: `i64` maps
/// to an ordinary integer column, so a field declared as one would be stored
/// and compared as an integer no matter what the schema said. Giving units
/// their own Rust type is what makes `#[derive(Record)]` emit a decimal column
/// and what keeps a caller from passing a price where a count belongs.
///
/// **It does not know its own scale.** The column does — see
/// [`ColumnDef::scale`](slate_schema::ColumnDef::scale) — so `Units(1250)` in a
/// scale-2 column is `12.50`, and rendering it needs the table. That is stated
/// here rather than hidden because it is the one thing a caller has to
/// remember, and it is what buys exact arithmetic: every value in the column
/// is a count of the same unit, so comparison and `SUM` are integer operations.
///
/// ```
/// use slate_orm::{Record, Units};
///
/// #[derive(Record)]
/// #[record(table = "invoices", id = 1)]
/// struct Invoice {
///     #[record(pk)]
///     id: u64,
///     #[record(scale = 2)]
///     total: Units,
/// }
///
/// let table = Invoice::table();
/// assert_eq!(table.column(table.ordinal_of("total").unwrap()).unwrap().scale(), Some(2));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Units(pub i64);

impl Units {
    /// The raw count of the column's smallest unit.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Render against a scale, as a decimal string.
    ///
    /// Takes the scale rather than reading one, because a value does not have
    /// one: pass [`ColumnDef::scale`](slate_schema::ColumnDef::scale).
    #[must_use]
    pub fn to_string_with_scale(self, scale: u8) -> String {
        if scale == 0 {
            return self.0.to_string();
        }
        let divisor = 10i64.saturating_pow(u32::from(scale));
        let negative = self.0 < 0;
        // Through `i128` so `i64::MIN` has a magnitude that fits.
        let magnitude = (i128::from(self.0)).unsigned_abs();
        let whole = magnitude / divisor.unsigned_abs() as u128;
        let part = magnitude % divisor.unsigned_abs() as u128;
        format!(
            "{}{whole}.{part:0width$}",
            if negative { "-" } else { "" },
            width = usize::from(scale)
        )
    }
}

impl Field for Units {
    const VALUE_TYPE: ValueType = ValueType::Decimal;
    const NULLABLE: bool = false;

    fn to_value(&self) -> Value {
        Value::Decimal(self.0)
    }

    fn from_value(value: &Value) -> Result<Self, FieldError> {
        match value {
            Value::Decimal(v) => Ok(Self(*v)),
            Value::Null => Err(FieldError::UnexpectedNull {
                expected: Self::VALUE_TYPE,
            }),
            other => Err(FieldError::TypeMismatch {
                expected: Self::VALUE_TYPE,
                found: other.type_name(),
            }),
        }
    }
}

impl Field for Vec<u8> {
    const VALUE_TYPE: ValueType = ValueType::Bytes;
    const NULLABLE: bool = false;

    fn to_value(&self) -> Value {
        Value::Bytes(Bytes::from(self.clone()))
    }

    fn from_value(value: &Value) -> Result<Self, FieldError> {
        match value {
            Value::Bytes(b) => Ok(b.to_vec()),
            Value::Null => Err(FieldError::UnexpectedNull {
                expected: Self::VALUE_TYPE,
            }),
            other => Err(FieldError::TypeMismatch {
                expected: Self::VALUE_TYPE,
                found: other.type_name(),
            }),
        }
    }
}

/// `f32` is stored as `f64`.
///
/// Widening is exact, so a value written from `f32` always reads back
/// unchanged. A `f64` written by something else may not fit, and that is an
/// error rather than a silent rounding — a narrowed key would not match the one
/// that was stored.
impl Field for f32 {
    const VALUE_TYPE: ValueType = ValueType::F64;
    const NULLABLE: bool = false;

    fn to_value(&self) -> Value {
        Value::F64(f64::from(*self))
    }

    fn from_value(value: &Value) -> Result<Self, FieldError> {
        match value {
            Value::F64(v) => {
                #[allow(clippy::cast_possible_truncation)]
                let narrowed = *v as Self;
                // NaN never equals itself, so accept it explicitly.
                if f64::from(narrowed) == *v || v.is_nan() {
                    Ok(narrowed)
                } else {
                    Err(FieldError::OutOfRange { target: "f32" })
                }
            }
            Value::Null => Err(FieldError::UnexpectedNull {
                expected: Self::VALUE_TYPE,
            }),
            other => Err(FieldError::TypeMismatch {
                expected: Self::VALUE_TYPE,
                found: other.type_name(),
            }),
        }
    }
}

/// An optional field is the same column, made nullable.
impl<T: Field> Field for Option<T> {
    const VALUE_TYPE: ValueType = T::VALUE_TYPE;
    const NULLABLE: bool = true;

    fn to_value(&self) -> Value {
        self.as_ref().map_or(Value::Null, T::to_value)
    }

    fn from_value(value: &Value) -> Result<Self, FieldError> {
        if value.is_null() {
            Ok(None)
        } else {
            T::from_value(value).map(Some)
        }
    }
}

/// An instant, as seconds since the Unix epoch.
///
/// A newtype over `i64` that maps to an ordinary integer column, so **nothing
/// on disk changes** — the same bytes an `i64` field would write. That is
/// deliberate, and it is the difference between this and [`Units`]: a decimal
/// needed its own column type because its ordering and arithmetic differ from
/// an integer's, and an instant's do not. Chronological order *is* integer
/// order, already proven and fuzzed by the tuple property suite.
///
/// What the newtype buys is in Rust and in the reader's head:
///
/// - a count of seconds and an instant are different things, and the compiler
///   now says so — `Timestamp` cannot be passed where a `u64` id belongs;
/// - it names the unit. The kernel's calendar functions
///   ([`Scalar::calendar_part`](slate_kernel::Scalar::calendar_part),
///   [`Scalar::date_trunc`](slate_kernel::Scalar::date_trunc), the timezone
///   lookups) all read **seconds**, and a column holding milliseconds would be
///   answered by them with a year somewhere around 55000 and no error;
/// - it gives the docs one place to say there is no date type and why.
///
/// There is no timezone in it. The stored instant is an absolute point in time;
/// which calendar day it falls on is a question about a zone, and
/// [`Scalar::in_zone`](slate_kernel::Scalar::in_zone) is where that is asked.
/// Storing a zone beside the instant would let the two disagree.
///
/// ```
/// use slate_orm::{CalendarPart, Query, Record, Scalar, Timestamp};
///
/// #[derive(Record)]
/// #[record(table = "events", id = 1)]
/// struct Event {
///     #[record(pk)]
///     id: u64,
///     at: Timestamp,
/// }
///
/// // The column is an ordinary integer: a timestamp is not a new value type.
/// let table = Event::table();
/// assert_eq!(
///     table.column(Event::COLUMNS.at).unwrap().value_type(),
///     slate_orm::ValueType::I64,
/// );
///
/// // And the calendar questions are scalars over it, evaluated per row.
/// let query = Query::all().computing([
///     Scalar::column(Event::COLUMNS.at).calendar_part(CalendarPart::Year),
///     Scalar::column(Event::COLUMNS.at)
///         .in_zone("America/New_York")
///         .calendar_part(CalendarPart::DayOfWeek),
/// ]);
/// assert_eq!(Query::computed(&table, 0), slate_orm::Ordinal(2));
/// let _ = query;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Seconds since 1970-01-01T00:00:00Z. Negative is before it.
    #[must_use]
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self(seconds)
    }

    /// The instant, as seconds since the epoch.
    #[must_use]
    pub const fn seconds(self) -> i64 {
        self.0
    }

    /// The epoch itself, which is what `Default` gives.
    pub const EPOCH: Self = Self(0);
}

impl Field for Timestamp {
    const VALUE_TYPE: ValueType = ValueType::I64;
    const NULLABLE: bool = false;

    fn to_value(&self) -> Value {
        Value::I64(self.0)
    }

    fn from_value(value: &Value) -> Result<Self, FieldError> {
        match value {
            Value::I64(v) => Ok(Self(*v)),
            Value::Null => Err(FieldError::UnexpectedNull {
                expected: Self::VALUE_TYPE,
            }),
            other => Err(FieldError::TypeMismatch {
                expected: Self::VALUE_TYPE,
                found: other.type_name(),
            }),
        }
    }
}
