//! The typed record trait.

use crate::field::FieldError;
use slate_schema::{Row, TableDef, TableId};
use slate_tuple::Value;

/// A stored row could not be turned back into its Rust type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RecordError {
    /// One column did not fit its field.
    #[error("column `{column}` of table `{table}`: {source}")]
    Field {
        /// The table the row belongs to.
        table: &'static str,
        /// The column at fault.
        column: &'static str,
        /// What went wrong.
        #[source]
        source: FieldError,
    },

    /// The row had a different number of columns than the type expects.
    #[error("table `{table}` expects {expected} column(s), row has {actual}")]
    ColumnCount {
        /// The table the row belongs to.
        table: &'static str,
        /// Columns the Rust type declares.
        expected: usize,
        /// Columns in the row.
        actual: usize,
    },
}

/// A Rust type that maps to one table.
///
/// Implemented by `#[derive(Record)]`. Writing it by hand is supported and is
/// the same amount of work the macro does — the macro exists to keep the schema
/// and the struct from drifting apart, not to hide anything.
pub trait Record: Sized {
    /// The table this type maps to.
    ///
    /// Built once and cached; the same `&'static` is returned every call, so it
    /// is cheap to call in a loop.
    fn table() -> &'static TableDef;

    /// Convert to a storage row.
    fn to_row(&self) -> Row;

    /// Convert back from a storage row.
    ///
    /// # Errors
    /// If a column does not fit its field, or the row has the wrong shape.
    fn from_row(row: &Row) -> Result<Self, RecordError>;

    /// This record's table id, without building the table.
    ///
    /// Defaults to `Self::table().id()`, and `#[derive(Record)]` overrides it
    /// with the literal from `#[record(id = N)]`. The override is not an
    /// optimisation: `table()` caches behind a `OnceLock`, so a foreign key
    /// declared in the attribute and resolved through `table()` would re-enter
    /// that lock from inside its own initialiser the moment a table referenced
    /// itself — `employee.manager_id -> employee` is the ordinary case, not an
    /// exotic one. The id is a literal in the attribute, so nothing has to be
    /// built to read it.
    ///
    /// Provided rather than required so that the hand-written impls this trait
    /// already supports keep compiling. One of those referencing itself through
    /// a derived foreign key is the single shape that still recurses, and it
    /// cannot arise: the attribute is the macro's.
    #[must_use]
    fn table_id() -> TableId {
        Self::table().id()
    }

    /// This record's primary key values, in key order.
    #[must_use]
    fn primary_key(&self) -> Vec<Value> {
        self.to_row().primary_key_values(Self::table())
    }
}
