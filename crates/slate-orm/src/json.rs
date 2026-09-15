//! A serialized document in a `Str` column.
//!
//! This is the smallest honest version of a JSON type, and the limits are the
//! interesting part.
//!
//! **There is no JSON value type, and there will not be one.** The kernel's
//! value model is closed and its ordering is the tuple encoding's — a document
//! has no useful total order, so it could not be a key, could not be indexed,
//! and could not answer a range predicate. What a `Json<T>` column *is* is a
//! string that happens to hold JSON, and every query against it is a query
//! against that string: `Expr::eq` compares the whole serialized text, `LIKE`
//! matches substrings of it, and there is no way to ask about a field inside.
//!
//! If a field inside the document needs querying, it is a column. That is not a
//! limitation to work around; it is the answer.

use crate::field::{Field, FieldError};
use serde::Serialize;
use serde::de::DeserializeOwned;
use slate_tuple::{Value, ValueType};

/// A document that could not be turned into JSON, or back.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct JsonError(#[from] serde_json::Error);

/// A `T` stored as JSON in a `Str` column.
///
/// **Serialized at construction, not at write time.** [`Field::to_value`]
/// cannot fail — it returns a `Value`, not a `Result` — so a type that
/// serialized lazily would have to panic or write a wrong value when
/// serialization failed. `Json::new` does the work and hands back the error
/// where a caller can still do something about it, and the encoded string is
/// kept so the write is infallible and costs no second pass.
///
/// ```
/// use serde::{Deserialize, Serialize};
/// use slate_orm::{Field, Json, Value};
///
/// #[derive(Serialize, Deserialize, PartialEq, Debug)]
/// struct Settings { theme: String, rows: u32 }
///
/// let settings = Json::new(Settings { theme: "dark".into(), rows: 50 })?;
/// assert_eq!(
///     settings.to_value(),
///     Value::Str(r#"{"theme":"dark","rows":50}"#.into()),
/// );
/// # Ok::<(), slate_orm::JsonError>(())
/// ```
///
/// ## What is stored is a string, and the string has to be stable
///
/// Two documents a caller considers equal must serialize to the same bytes, or
/// equality and uniqueness are wrong: a unique index over the column would
/// admit both, and `Expr::eq` against one would miss the other. For a `struct`
/// this holds — `serde` emits fields in declaration order — and for a
/// `BTreeMap` it holds, because the keys are ordered. For a `HashMap` it does
/// **not**: the iteration order varies between processes, so the same map can
/// be stored twice under two different strings. `json_hash_maps_do_not_have_a
/// _stable_encoding` in `tests/json.rs` demonstrates that rather than warning
/// about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Json<T> {
    value: T,
    encoded: String,
}

impl<T: Serialize> Json<T> {
    /// Serialize now, so that storing later cannot fail.
    ///
    /// # Errors
    /// If `T` cannot be represented as JSON — a map with non-string keys, a
    /// float that is `NaN`, a `Serialize` impl that returns an error.
    pub fn new(value: T) -> Result<Self, JsonError> {
        let encoded = serde_json::to_string(&value)?;
        Ok(Self { value, encoded })
    }
}

impl<T> Json<T> {
    /// The document.
    pub const fn get(&self) -> &T {
        &self.value
    }

    /// The document, taken out.
    #[allow(clippy::missing_const_for_fn)]
    pub fn into_inner(self) -> T {
        self.value
    }

    /// The exact text that is stored, which is what a predicate compares.
    pub fn as_json(&self) -> &str {
        &self.encoded
    }
}

impl<T: Serialize + DeserializeOwned> Field for Json<T> {
    const VALUE_TYPE: ValueType = ValueType::Str;
    const NULLABLE: bool = false;

    fn to_value(&self) -> Value {
        // Infallible because `new` already did the fallible part. Cloning the
        // string rather than re-serializing also means the stored bytes are
        // exactly the ones the caller can read with `as_json`, so a test that
        // asserts on one is asserting on the other.
        Value::Str(self.encoded.clone())
    }

    fn from_value(value: &Value) -> Result<Self, FieldError> {
        match value {
            Value::Str(text) => {
                let parsed: T = serde_json::from_str(text.as_ref()).map_err(|_| {
                    // The column's type is right and its contents are not, so
                    // this is neither a type mismatch nor a null. A row written
                    // by a build with a different `T` lands here, which is the
                    // case worth being loud about.
                    FieldError::OutOfRange {
                        target: "Json<T>: the stored text is not this document",
                    }
                })?;
                Ok(Self {
                    value: parsed,
                    // Re-encoded rather than kept, so that a value read back
                    // and written again is byte-identical to what `new` would
                    // have produced — otherwise a round trip could change the
                    // stored text and move the row in a unique index.
                    encoded: text.to_string(),
                })
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
