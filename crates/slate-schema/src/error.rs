//! Errors raised when defining or using a schema.

use slate_tuple::{TupleError, ValueType};

/// A schema definition was rejected, or a row did not match its schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SchemaError {
    /// A column name referenced in a key or index does not exist on the table.
    #[error("table `{table}` has no column named `{column}`")]
    UnknownColumn {
        /// The table being defined.
        table: String,
        /// The name that could not be resolved.
        column: String,
    },

    /// Two columns on the same table share a name.
    #[error("table `{table}` declares column `{column}` more than once")]
    DuplicateColumn {
        /// The table being defined.
        table: String,
        /// The repeated name.
        column: String,
    },

    /// A table was defined without a primary key.
    #[error("table `{table}` has no primary key")]
    MissingPrimaryKey {
        /// The table being defined.
        table: String,
    },

    /// A column appears twice in the same key or index.
    #[error("`{key}` on table `{table}` lists column `{column}` more than once")]
    DuplicateKeyColumn {
        /// The table being defined.
        table: String,
        /// The primary key or index at fault.
        key: String,
        /// The repeated column.
        column: String,
    },

    /// A primary key column was declared nullable.
    ///
    /// A null in a key position would make two rows share an identity, so the
    /// record layer forbids it rather than picking a tie-break rule.
    #[error("primary key column `{column}` on table `{table}` may not be nullable")]
    NullablePrimaryKeyColumn {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// Two indexes on the same table share an id or a name.
    #[error("table `{table}` declares index `{index}` more than once")]
    DuplicateIndex {
        /// The table being defined.
        table: String,
        /// The repeated index id or name.
        index: String,
    },

    /// An index was defined with no columns.
    #[error("index `{index}` on table `{table}` has no columns")]
    EmptyIndex {
        /// The table being defined.
        table: String,
        /// The offending index.
        index: String,
    },

    /// The tenant column is not the first primary key column.
    ///
    /// Tenant scoping is enforced by key prefix, so the tenant must be the
    /// leading component of every key on the table.
    #[error(
        "tenant column `{column}` on table `{table}` must be the first primary key column, \
         so that tenant scoping is a key prefix"
    )]
    TenantColumnNotKeyPrefix {
        /// The table being defined.
        table: String,
        /// The offending column.
        column: String,
    },

    /// Two tables in a catalog share an id or a name.
    #[error("catalog declares table `{table}` more than once")]
    DuplicateTable {
        /// The repeated table id or name.
        table: String,
    },

    /// A row had the wrong number of columns for its table.
    #[error("table `{table}` expects {expected} column(s), row has {actual}")]
    ColumnCountMismatch {
        /// The table the row belongs to.
        table: String,
        /// Column count from the schema.
        expected: usize,
        /// Column count in the row.
        actual: usize,
    },

    /// A row value had the wrong type for its column.
    #[error("column `{column}` on table `{table}` expects {expected}, got {actual}")]
    ValueTypeMismatch {
        /// The table the row belongs to.
        table: String,
        /// The offending column.
        column: String,
        /// The declared column type.
        expected: ValueType,
        /// The type actually supplied.
        actual: &'static str,
    },

    /// A null was supplied for a non-nullable column.
    #[error("column `{column}` on table `{table}` is not nullable")]
    UnexpectedNull {
        /// The table the row belongs to.
        table: String,
        /// The offending column.
        column: String,
    },

    /// A stored row body could not be decoded.
    #[error("decoding a row of table `{table}`: {source}")]
    RowDecode {
        /// The table the row belongs to.
        table: String,
        /// The underlying codec failure.
        #[source]
        source: TupleError,
    },

    /// A stored row was written by a newer schema than this binary knows.
    #[error(
        "row of table `{table}` was written at schema version {found}, \
         but this build only understands up to {known}"
    )]
    RowFromFutureSchema {
        /// The table the row belongs to.
        table: String,
        /// Version stamped on the stored row.
        found: u32,
        /// Version this build declares.
        known: u32,
    },

    /// A stored row body used an unrecognised container format.
    #[error("row of table `{table}` has unsupported storage format {format}")]
    UnsupportedRowFormat {
        /// The table the row belongs to.
        table: String,
        /// The format byte found.
        format: u8,
    },

    /// A column was added in a later schema version without being nullable, so
    /// rows written before it cannot be read back.
    #[error(
        "column `{column}` was added to table `{table}` after schema version {written}, \
         so it must be nullable to read rows written then"
    )]
    IncompatibleSchemaEvolution {
        /// The table the row belongs to.
        table: String,
        /// The column missing from the stored row.
        column: String,
        /// Schema version the stored row was written at.
        written: u32,
    },
}

/// Convenience alias for schema results.
pub type Result<T> = core::result::Result<T, SchemaError>;
