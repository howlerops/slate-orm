//! The closed value model shared by the codec, the schema layer and the kernel.
//!
//! The set of variants is deliberately fixed. A closed type system is what lets
//! the codec guarantee that byte order equals value order, and lets the kernel
//! reason about index bounds without a dynamic type registry.

use bytes::Bytes;
use core::cmp::Ordering;
use uuid::Uuid;

/// A single tuple element.
///
/// [`Value`] has a *total* order (see the [`Ord`] impl) that is byte-for-byte
/// identical to the order of its encoding. Two values that compare equal always
/// encode to the same bytes, and vice versa.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Value {
    /// SQL-style null. Sorts before every other value.
    Null,
    /// Boolean. `false` sorts before `true`.
    Bool(bool),
    /// Opaque byte string.
    Bytes(Bytes),
    /// UTF-8 text. Ordered by its UTF-8 bytes, which matches code point order.
    Str(String),
    /// Signed 64-bit integer.
    I64(i64),
    /// Unsigned 64-bit integer. Shares an encoding with [`Value::I64`], so the
    /// two order numerically against each other.
    U64(u64),
    /// IEEE-754 double. NaN is canonicalised on encode and sorts above all
    /// other doubles, including positive infinity.
    F64(f64),
    /// UUID, ordered by its 16 big-endian bytes.
    Uuid(Uuid),
}

/// The type tag of a [`Value`], used to drive schema-directed decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ValueType {
    /// See [`Value::Bool`].
    Bool,
    /// See [`Value::Bytes`].
    Bytes,
    /// See [`Value::Str`].
    Str,
    /// See [`Value::I64`].
    I64,
    /// See [`Value::U64`].
    U64,
    /// See [`Value::F64`].
    F64,
    /// See [`Value::Uuid`].
    Uuid,
}

impl ValueType {
    /// A human-readable name, used in error messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Bytes => "bytes",
            Self::Str => "string",
            Self::I64 => "i64",
            Self::U64 => "u64",
            Self::F64 => "f64",
            Self::Uuid => "uuid",
        }
    }
}

impl core::fmt::Display for ValueType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

impl Value {
    /// The type tag of this value, or `None` for [`Value::Null`].
    ///
    /// Null is not a type of its own: nullability is a property of a column, so
    /// a null carries no type tag at the value level.
    #[must_use]
    pub const fn value_type(&self) -> Option<ValueType> {
        match self {
            Self::Null => None,
            Self::Bool(_) => Some(ValueType::Bool),
            Self::Bytes(_) => Some(ValueType::Bytes),
            Self::Str(_) => Some(ValueType::Str),
            Self::I64(_) => Some(ValueType::I64),
            Self::U64(_) => Some(ValueType::U64),
            Self::F64(_) => Some(ValueType::F64),
            Self::Uuid(_) => Some(ValueType::Uuid),
        }
    }

    /// A human-readable type name, used in error messages.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self.value_type() {
            None => "null",
            Some(t) => t.name(),
        }
    }

    /// Whether this value is [`Value::Null`].
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Rank of the value's *class* in the cross-type order.
    ///
    /// Mirrors the ordering of the type codes emitted by the codec:
    /// null < bool < bytes < string < integer < double < uuid.
    /// Signed and unsigned integers share a rank because they share an encoding.
    const fn class_rank(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Bool(_) => 1,
            Self::Bytes(_) => 2,
            Self::Str(_) => 3,
            Self::I64(_) | Self::U64(_) => 4,
            Self::F64(_) => 5,
            Self::Uuid(_) => 6,
        }
    }

    /// Widen any integer variant to `i128` so signed and unsigned compare.
    const fn as_i128(&self) -> Option<i128> {
        match self {
            Self::I64(v) => Some(*v as i128),
            Self::U64(v) => Some(*v as i128),
            _ => None,
        }
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        let rank = self.class_rank().cmp(&other.class_rank());
        if rank != Ordering::Equal {
            return rank;
        }
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Bytes(a), Self::Bytes(b)) => a.cmp(b),
            (Self::Str(a), Self::Str(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Self::Uuid(a), Self::Uuid(b)) => a.as_bytes().cmp(b.as_bytes()),
            // NaN is canonicalised on encode, so all NaNs are one value here.
            (Self::F64(a), Self::F64(b)) => match (a.is_nan(), b.is_nan()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => a.total_cmp(b),
            },
            _ => match (self.as_i128(), other.as_i128()) {
                (Some(a), Some(b)) => a.cmp(&b),
                // Unreachable: equal class ranks are exhausted above.
                _ => Ordering::Equal,
            },
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Value {}

macro_rules! from_impl {
    ($($ty:ty => $variant:ident $(as $cast:ty)?),* $(,)?) => {
        $(impl From<$ty> for Value {
            fn from(v: $ty) -> Self {
                Self::$variant(v $(as $cast)?)
            }
        })*
    };
}

from_impl! {
    bool => Bool,
    i8 => I64 as i64,
    i16 => I64 as i64,
    i32 => I64 as i64,
    i64 => I64,
    u8 => U64 as u64,
    u16 => U64 as u64,
    u32 => U64 as u64,
    u64 => U64,
    f32 => F64 as f64,
    f64 => F64,
    String => Str,
    Bytes => Bytes,
    Uuid => Uuid,
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::Str(v.to_owned())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Self::Bytes(Bytes::from(v))
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Null, Into::into)
    }
}
