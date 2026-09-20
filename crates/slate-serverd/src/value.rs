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
pub(crate) fn from_toml(
    value: &toml::Value,
    declared: ValueType,
    element: Option<ValueType>,
    field: &str,
) -> Started<Value> {
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
        // A count of the column's smallest unit, not a number: at `scale = 2`,
        // `default = 1250` is 12.50. Written as an integer rather than as
        // `12.50` on purpose — a float default for a decimal column would be
        // parsed by TOML as a binary double and rounded before this code ever
        // saw it, which is the exact loss the type exists to avoid.
        (toml::Value::Integer(n), ValueType::Decimal) => Ok(Value::Decimal(*n)),
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
        // The element type is the column's, so it has to be threaded in; there
        // is no way to read it off the TOML. A `None` here for an array column
        // cannot happen through `columns_builder`, which refuses an array with
        // no element type before this is reached — but this function is public
        // within the crate and the seed path calls it too, so it says so
        // rather than picking a type.
        (toml::Value::Array(elements), ValueType::Array) => {
            let Some(element) = element else {
                return Err(Fault::new(format!(
                    "`{field}` is a list and its column declares no element type"
                )));
            };
            elements
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    from_toml(item, element, None, &format!("{field}[{index}]"))
                })
                .collect::<Started<Vec<Value>>>()
                .map(Value::Array)
        }
        _ => Err(mismatch()),
    }
}

/// The type names this parser accepts, as a readable list.
///
/// Derived from `ValueType::ALL` so it cannot go one short. `str` is listed
/// beside `string` because the parser takes both and `name()` only gives the
/// longer one.
fn known_type_names() -> String {
    let mut names: Vec<&str> = ValueType::ALL.iter().map(|kind| kind.name()).collect();
    names.push("str");
    names.sort_unstable();
    match names.split_last() {
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
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
        "decimal" => Ok(ValueType::Decimal),
        "array" => Ok(ValueType::Array),
        // The list is built from `ValueType::ALL` rather than written out, for
        // the reason everything else in this repository derives its rosters:
        // a hand-written one goes one short the day a type is added, and a
        // message that lies about what is available is worse than a bare
        // refusal. `str` is the one spelling `name()` does not give — the
        // parser accepts both it and `string` — so it is added by hand.
        other => Err(Fault::new(format!(
            "`{field} = \"{other}\"` is not a type; there are {}",
            known_type_names()
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
            from_toml(&seven, ValueType::U64, None, "d").unwrap(),
            Value::U64(7)
        );
        assert_eq!(
            from_toml(&seven, ValueType::I64, None, "d").unwrap(),
            Value::I64(7)
        );
    }

    #[test]
    fn a_negative_default_for_an_unsigned_column_is_refused() {
        let error = from_toml(&toml::Value::Integer(-1), ValueType::U64, None, "d")
            .unwrap_err()
            .to_string();
        assert!(error.contains("negative"), "{error}");
    }

    #[test]
    fn a_default_of_the_wrong_shape_names_both_sides() {
        let error = from_toml(
            &toml::Value::String("x".into()),
            ValueType::I64,
            None,
            "docs.size.default",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("a string"), "{error}");
        assert!(error.contains("i64"), "{error}");
    }

    /// The refusal lists every type, derived rather than written out.
    ///
    /// The list used to be a string literal naming nine types, which is the
    /// shape that goes one short: it would have said `array` was not a type
    /// while the parser accepted it. Held to `ValueType::ALL`, which the
    /// compiler holds to the enum.
    #[test]
    fn an_unknown_type_lists_the_types_there_are() {
        let error = value_type("varchar", "type").unwrap_err().to_string();
        for kind in ValueType::ALL {
            assert!(
                error.contains(kind.name()),
                "{} is a type and the refusal does not list it: {error}",
                kind.name()
            );
        }
        assert!(error.contains("str"), "{error}");
    }

    #[test]
    fn an_array_default_takes_the_columns_element_type() {
        let list = toml::Value::Array(vec![
            toml::Value::String("a".into()),
            toml::Value::String("b".into()),
        ]);
        assert_eq!(
            from_toml(&list, ValueType::Array, Some(ValueType::Str), "d").unwrap(),
            Value::Array(vec![Value::Str("a".into()), Value::Str("b".into())])
        );

        // An element of the wrong shape names the element, not just the list.
        let mixed = toml::Value::Array(vec![
            toml::Value::String("a".into()),
            toml::Value::Integer(2),
        ]);
        let error = from_toml(&mixed, ValueType::Array, Some(ValueType::Str), "d")
            .unwrap_err()
            .to_string();
        assert!(error.contains("d[1]"), "{error}");

        // And a list with no element type is refused rather than guessed at.
        let error = from_toml(&list, ValueType::Array, None, "d")
            .unwrap_err()
            .to_string();
        assert!(error.contains("element type"), "{error}");
    }
}
