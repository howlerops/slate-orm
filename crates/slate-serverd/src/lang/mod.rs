//! The little language a configuration file writes predicates and index
//! expressions in.
//!
//! # Why there is a parser here at all
//!
//! A `CHECK`, a partial index's predicate and a row policy are
//! [`Expr`](slate_kernel::Expr) values; an expression index's key is a
//! [`Scalar`](slate_kernel::Scalar). Neither is something TOML can hold
//! directly, so a configuration file has to encode them somehow, and there are
//! exactly two ways to do it.
//!
//! **Structurally**, as nested tables mirroring the enum:
//!
//! ```toml
//! where = { and = [
//!   { compare = { column = "size", op = ">=", value = 10 } },
//!   { compare = { column = "kind", op = "=",  value = "a" } },
//! ] }
//! ```
//!
//! **As surface syntax**, parsed here:
//!
//! ```toml
//! where = "size >= 10 AND kind = 'a'"
//! ```
//!
//! The structural form was rejected, and not merely for being unreadable —
//! though a file a human is expected to write and review is exactly the place
//! where that decides it. The deciding argument is that the structural form is
//! *also* a language: it needs the same validation, the same error messages
//! and the same column resolution, and on top of that it wants a
//! `#[derive(Deserialize)]` mirror of `Expr`. A serde mirror of an enum in
//! another crate is a second declaration of that enum, and the day `Expr`
//! grows a variant the mirror is silently one variant short. This parser
//! cannot drift that way: it produces `Expr` values, and syntax it does not
//! know is a parse error at startup rather than a variant quietly missing.
//!
//! **This is a surface syntax, not a second semantics.** Nothing here
//! evaluates anything. There is one expression language, one evaluator and one
//! set of three-valued null rules, all in the kernel; this module only builds
//! the values the kernel already defines.
//!
//! # Every literal has a column opposite it
//!
//! [`slate_server::auth`] has to write `u64:7` for a principal id, because
//! `7` could be a [`Value::U64`](slate_tuple::Value::U64) or a
//! [`Value::Str`](slate_tuple::Value::Str) and the two are different
//! principals. That problem does not arise here: a literal in this language
//! always sits opposite a column, and the column's declared type decides what
//! the literal is. `size >= 10` against an `i64` column is `Value::I64(10)`;
//! against a `u64` column it is `Value::U64(10)`. A configuration file
//! therefore needs no type tags at all, and cannot get one wrong.
//!
//! The one exception is a value with no column opposite it — a bearer token's
//! principal id — and there the tagged spelling from `slate_server::auth` is
//! used unchanged rather than invented again. See `crate::value`.
//!
//! # What it deliberately cannot say
//!
//! Recorded in full in [`crate::config`]; the short version is that the
//! predicate grammar covers every [`Expr`] variant, and the scalar grammar
//! covers arithmetic, `lower`, `upper`, `length`, `concat`, `coalesce` and
//! `regexp_replace` but not `CASE`, `EXTRACT`, `DATE_TRUNC` or vector
//! distance. Those four need a sub-language of their own (a branch list, a
//! time unit, a metric), and an index keyed on a vector distance is refused by
//! the schema layer anyway.

pub(crate) mod lex;
pub(crate) mod pred;
pub(crate) mod scalar;

use slate_schema::{Ordinal, TableDef};
use slate_tuple::ValueType;

/// Where an error is, in the source text it is about.
///
/// Byte offsets rather than line and column: these expressions are single-line
/// strings inside a TOML value, so a column number is the only useful
/// coordinate and a caret under the offending token is more useful still.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LangError {
    /// Byte offset into the expression source.
    pub(crate) at: usize,
    /// What is wrong, in a sentence.
    pub(crate) message: String,
}

impl LangError {
    pub(crate) fn new(at: usize, message: impl Into<String>) -> Self {
        Self {
            at,
            message: message.into(),
        }
    }

    /// Render the error under the source it is about.
    ///
    /// Two lines and a caret, because the alternative — "parse error at byte
    /// 17" — makes an operator count characters. The caret is placed by
    /// character count rather than byte count so a non-ASCII literal earlier in
    /// the expression does not push it off the token.
    pub(crate) fn render(&self, source: &str) -> String {
        let prefix = source.get(..self.at.min(source.len())).unwrap_or(source);
        let column = prefix.chars().count();
        format!(
            "  {source}\n  {:>width$} {}",
            "^",
            self.message,
            width = column + 1
        )
    }
}

pub(crate) type LangResult<T> = Result<T, LangError>;

/// What the parser needs to know about the table an expression is written
/// against.
///
/// A trait rather than `&TableDef` so the parser can be unit-tested without
/// building a catalog, and so the two callers that resolve columns differently
/// — a table's own `CHECK`, and a policy over a table already in the catalog —
/// use the same code.
pub(crate) trait Scope {
    /// The ordinal `name` refers to, if any.
    fn ordinal(&self, name: &str) -> Option<Ordinal>;
    /// What the column at `ordinal` holds.
    fn value_type(&self, ordinal: Ordinal) -> Option<ValueType>;
    /// Every column name, for the "did you mean" half of an error.
    fn column_names(&self) -> Vec<String>;
    /// Whether `:principal` and `:tenant` may appear.
    ///
    /// A policy is a function of the caller and a `CHECK` is not: a `CHECK`
    /// that varied by caller would let a row be written that another caller
    /// could not have written, and the record store checks it once at write
    /// time with nobody's context in hand.
    fn allows_placeholders(&self) -> bool;
}

/// A [`Scope`] over a built table definition.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TableScope<'a> {
    table: &'a TableDef,
    placeholders: bool,
}

impl<'a> TableScope<'a> {
    /// A scope in which `:principal` and `:tenant` are errors.
    pub(crate) const fn constant(table: &'a TableDef) -> Self {
        Self {
            table,
            placeholders: false,
        }
    }

    /// A scope in which `:principal` and `:tenant` resolve from the caller.
    pub(crate) const fn per_caller(table: &'a TableDef) -> Self {
        Self {
            table,
            placeholders: true,
        }
    }
}

impl Scope for TableScope<'_> {
    fn ordinal(&self, name: &str) -> Option<Ordinal> {
        self.table.ordinal_of(name)
    }

    fn value_type(&self, ordinal: Ordinal) -> Option<ValueType> {
        self.table
            .column(ordinal)
            .map(slate_schema::ColumnDef::value_type)
    }

    fn column_names(&self) -> Vec<String> {
        self.table
            .columns()
            .iter()
            .filter(|c| !c.is_dropped())
            .map(|c| c.name().to_owned())
            .collect()
    }

    fn allows_placeholders(&self) -> bool {
        self.placeholders
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
    fn a_caret_lands_under_the_offending_token() {
        let error = LangError::new(7, "not an i64");
        let rendered = error.render("size > 'x'");
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2);
        let caret = lines[1].find('^').unwrap_or(usize::MAX);
        let source = lines[0].find("size").unwrap_or(0);
        assert_eq!(caret - source, 7, "caret should sit under the quote");
    }

    #[test]
    fn a_multibyte_literal_does_not_push_the_caret_off() {
        // Byte offset 12 is character offset 8 here: `'é'` is three bytes.
        let source = "kind = 'é' AND size > 'x'";
        let at = source.find("'x'").unwrap_or(0);
        let rendered = LangError::new(at, "nope").render(source);
        let lines: Vec<&str> = rendered.lines().collect();
        let caret = lines[1].find('^').unwrap_or(usize::MAX);
        let start = lines[0].find("kind").unwrap_or(0);
        assert_eq!(caret - start, source.chars().count() - 3);
    }
}
