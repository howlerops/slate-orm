//! Values that have no column opposite them.
//!
//! Almost every literal in a configuration file sits next to a column, and the
//! column's declared type decides what it is — see [`crate::lang`]. Two do
//! not, and each gets its own rule here.
//!
//! **A column's `default`** is a TOML value being given to a column whose type
//! is declared right beside it, so the declared type decides, exactly as in an
//! expression. The only wrinkle is that TOML's own scalars are a different set
//! from [`Value`]'s: TOML has one integer type and no uuid.
//!
//! **A bearer token's principal id** has nothing beside it at all. It is
//! written tagged — `u64:7`, `str:ada` — using the same spelling
//! `slate_server::auth` already defines for the identity headers, with the
//! same argument: a principal id of `7` could be a [`Value::U64`] or a
//! [`Value::Str`], `Value`'s order is type-first, and the two are different
//! principals that compare unequal to each other and to whatever is stored in
//! the rows. Reusing the spelling rather than inventing one means an operator
//! who moves a deployment from `trusted-header` to `token` writes the same
//! text in both places.

use crate::error::{Fault, Started};
use slate_tuple::{Value, ValueType};

/// Parse a `<type>:<text>` identity value.
///
/// The tag set is `slate_server::auth`'s, deliberately: str, i64, u64, bool
/// and uuid. `f64`, `bytes` and `vector` are not in it there and are not here,
/// because a principal identified by a float is a bug rather than a feature.
pub(crate) fn tagged(text: &str, field: &str) -> Started<Value> {
    let Some((tag, rest)) = text.split_once(':') else {
        return Err(Fault::new(format!(
            "`{field} = \"{text}\"` has no type tag; write one of str:, i64:, u64:, bool: or uuid: in front of it, as the `slate-principal` header does"
        )));
    };
    let bad = |what: &str| Fault::new(format!("`{field} = \"{text}\"`: `{rest}` is not {what}"));
    match tag {
        "str" => Ok(Value::Str(rest.to_owned())),
        "i64" => rest.parse().map(Value::I64).map_err(|_| bad("an i64")),
        "u64" => rest.parse().map(Value::U64).map_err(|_| bad("a u64")),
        "bool" => rest.parse().map(Value::Bool).map_err(|_| bad("a bool")),
        "uuid" => uuid::Uuid::parse_str(rest)
            .map(Value::Uuid)
            .map_err(|_| bad("a uuid")),
        other => Err(Fault::new(format!(
            "`{field} = \"{text}\"`: `{other}:` is not a type tag; there are str:, i64:, u64:, bool: and uuid:"
        ))),
    }
}

/// Convert a TOML value to the type its column declares.
///
/// TOML has one integer type, so `7` is `u64` opposite a `u64` column and
/// `i64` opposite an `i64` one, and neither needs saying in the file. A uuid
/// and a byte string are written as strings, because TOML has no syntax for
/// either — hexadecimal for bytes, for the reason given in
/// [`crate::lang::pred`].
pub(crate) fn from_toml(value: &toml::Value, declared: ValueType, field: &str) -> Started<Value> {
    let mismatch = || {
        Fault::new(format!(
            "`{field}` is {}, and the column it belongs to holds {declared}",
            describe(value)
        ))
    };
    match (value, declared) {
        (toml::Value::Boolean(b), ValueType::Bool) => Ok(Value::Bool(*b)),
        (toml::Value::String(s), ValueType::Str) => Ok(Value::Str(s.clone())),
        (toml::Value::Integer(n), ValueType::I64) => Ok(Value::I64(*n)),
        (toml::Value::Integer(n), ValueType::U64) => u64::try_from(*n)
            .map(Value::U64)
            .map_err(|_| Fault::new(format!("`{field} = {n}` is negative and the column holds u64"))),
        (toml::Value::Integer(n), ValueType::F64) => Ok(Value::F64(*n as f64)),
        (toml::Value::Float(x), ValueType::F64) => Ok(Value::F64(*x)),
        (toml::Value::String(s), ValueType::Uuid) => uuid::Uuid::parse_str(s)
            .map(Value::Uuid)
            .map_err(|_| Fault::new(format!("`{field} = \"{s}\"` is not a uuid"))),
        (toml::Value::String(s), ValueType::Bytes) => hex(s)
            .map(|bytes| Value::Bytes(bytes.into()))
            .ok_or_else(|| {
                Fault::new(format!(
                    "`{field} = \"{s}\"` is not hexadecimal; a bytes column's default is an even number of hex digits"
                ))
            }),
        (toml::Value::Array(elements), ValueType::Vector) => elements
            .iter()
            .map(|element| match element {
                toml::Value::Float(x) => Ok(*x as f32),
                toml::Value::Integer(n) => Ok(*n as f32),
                _ => Err(Fault::new(format!("`{field}` holds something that is not a number"))),
            })
            .collect::<Started<Vec<f32>>>()
            .map(Value::Vector),
        _ => Err(mismatch()),
    }
}

fn describe(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "a string",
        toml::Value::Integer(_) => "an integer",
        toml::Value::Float(_) => "a float",
        toml::Value::Boolean(_) => "a boolean",
        toml::Value::Datetime(_) => "a datetime, which this project has no value type for",
        toml::Value::Array(_) => "an array",
        toml::Value::Table(_) => "a table",
    }
}

fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect()
}

/// Parse a column type name.
pub(crate) fn value_type(name: &str, field: &str) -> Started<ValueType> {
    match name {
        "bool" => Ok(ValueType::Bool),
        "bytes" => Ok(ValueType::Bytes),
        "str" | "string" => Ok(ValueType::Str),
        "i64" => Ok(ValueType::I64),
        "u64" => Ok(ValueType::U64),
        "f64" => Ok(ValueType::F64),
        "uuid" => Ok(ValueType::Uuid),
        "vector" => Ok(ValueType::Vector),
        other => Err(Fault::new(format!(
            "`{field} = \"{other}\"` is not a type; there are bool, bytes, str, i64, u64, f64, uuid and vector"
        ))),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn an_untagged_principal_is_refused_and_names_the_header_it_matches() {
        let error = tagged("7", "principal").unwrap_err().to_string();
        assert!(error.contains("slate-principal"), "{error}");
        assert!(error.contains("u64:"), "{error}");
    }

    #[test]
    fn the_tag_decides_the_type_rather_than_the_text() {
        assert_eq!(tagged("u64:7", "p").unwrap(), Value::U64(7));
        assert_eq!(tagged("str:7", "p").unwrap(), Value::Str("7".into()));
        assert_ne!(tagged("u64:7", "p").unwrap(), tagged("str:7", "p").unwrap());
    }

    #[test]
    fn an_integer_default_takes_the_columns_signedness() {
        let seven = toml::Value::Integer(7);
        assert_eq!(
            from_toml(&seven, ValueType::U64, "d").unwrap(),
            Value::U64(7)
        );
        assert_eq!(
            from_toml(&seven, ValueType::I64, "d").unwrap(),
            Value::I64(7)
        );
    }

    #[test]
    fn a_negative_default_for_an_unsigned_column_is_refused() {
        let error = from_toml(&toml::Value::Integer(-1), ValueType::U64, "d")
            .unwrap_err()
            .to_string();
        assert!(error.contains("negative"), "{error}");
    }

    #[test]
    fn a_default_of_the_wrong_shape_names_both_sides() {
        let error = from_toml(
            &toml::Value::String("x".into()),
            ValueType::I64,
            "docs.size.default",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("a string"), "{error}");
        assert!(error.contains("i64"), "{error}");
    }

    #[test]
    fn an_unknown_type_lists_the_types_there_are() {
        let error = value_type("varchar", "type").unwrap_err().to_string();
        assert!(error.contains("uuid"), "{error}");
    }
}
