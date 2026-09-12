//! Errors raised by the record layer kernel.

use slate_schema::{SchemaError, TableId};
use slate_tuple::TupleError;
use std::error::Error as StdError;
use std::fmt;

/// A failure reported by the underlying key-value store.
///
/// The kernel does not know what backs it, so backend errors are boxed. Use
/// [`StorageError::new`] from a backend adapter.
#[derive(Debug)]
pub struct StorageError(Box<dyn StdError + Send + Sync>);

impl StorageError {
    /// Wrap a backend error.
    pub fn new<E: StdError + Send + Sync + 'static>(source: E) -> Self {
        Self(Box::new(source))
    }

    /// Wrap an error that is already boxed.
    #[must_use]
    pub const fn from_boxed(source: Box<dyn StdError + Send + Sync>) -> Self {
        Self(source)
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl StdError for StorageError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Anything that can go wrong inside the kernel.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KernelError {
    /// The storage backend failed.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),

    /// A schema rule was violated, or a stored row did not match its schema.
    #[error(transparent)]
    Schema(#[from] SchemaError),

    /// A stored key could not be decoded.
    #[error("decoding key: {0}")]
    KeyDecode(#[from] TupleError),

    /// A plan or write named a table the catalog does not contain.
    #[error("no table with id {0:?} in the catalog")]
    UnknownTable(TableId),

    /// A write would put two rows in the same unique index slot.
    #[error("unique index `{index}` on table `{table}` already has a row with these values")]
    UniqueViolation {
        /// The table written to.
        table: String,
        /// The index that would collide.
        index: String,
    },

    /// An insert found an existing row with the same primary key.
    #[error("table `{table}` already has a row with this primary key")]
    DuplicatePrimaryKey {
        /// The table written to.
        table: String,
    },

    /// An update or delete named a row that does not exist.
    #[error("table `{table}` has no row with this primary key")]
    RowNotFound {
        /// The table written to.
        table: String,
    },

    /// The transaction conflicted with one that committed first.
    ///
    /// Retrying is the expected response. Two inserts racing for the same
    /// unique index slot surface here, because they write the same key.
    #[error("transaction conflicted with a concurrent commit; retry")]
    TransactionConflict,

    /// A stored index entry pointed at a row key that would not decode.
    ///
    /// This means the index and the table disagree, which the record store
    /// writes are designed to prevent; treat it as corruption.
    #[error("index `{index}` on table `{table}` holds a malformed entry")]
    CorruptIndexEntry {
        /// The table scanned.
        table: String,
        /// The index scanned.
        index: String,
    },
}

/// Convenience alias for kernel results.
pub type Result<T> = core::result::Result<T, KernelError>;
