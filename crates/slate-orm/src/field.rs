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
