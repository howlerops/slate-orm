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

    /// The caller's roles do not grant this action on this table.
    #[error("access denied: no role grants {action} on table `{table}`")]
    AccessDenied {
        /// The table named.
        table: String,
        /// The action attempted.
        action: &'static str,
    },

    /// A tenant-scoped table was reached by a context with no tenant.
    ///
    /// Refused rather than defaulted, because the only available default would
    /// be to show every tenant.
    #[error("table `{table}` is tenant-scoped, but the security context has no tenant")]
    TenantRequired {
        /// The table named.
        table: String,
    },

    /// A write would produce a row the caller is not permitted to have written.
    #[error("row-level security forbids writing this row to table `{table}`")]
    RowCheckFailed {
        /// The table written to.
        table: String,
    },

    /// Another writer has taken over; this one is no longer the writer.
    ///
    /// Terminal, and deliberately distinct from [`KernelError::TransactionConflict`].
    /// A single-writer deployment fences the previous writer when a new one
    /// starts, so a process that retried through this would be a split brain
    /// still trying to write. It must stop instead.
    #[error("this writer has been fenced by a newer one and must stop")]
    WriterFenced,

    /// A read replica has not caught up to the sequence the caller requires.
    ///
    /// Raised rather than served stale, so that read-your-writes is a promise
    /// the caller can rely on. The usual response is to retry on another
    /// replica or fall back to the writer.
    #[error("replica `{replica}` is at sequence {visible}, behind the required {required}")]
    ReplicaTooStale {
        /// Which replica.
        replica: String,
        /// Sequence the caller needs to see.
        required: u64,
        /// Sequence the replica currently reflects.
        visible: u64,
    },

    /// No store in the pool could serve the read.
    #[error("no replica available: {reason}")]
    NoReplicaAvailable {
        /// Why routing failed.
        reason: &'static str,
    },

    /// A sum or average was asked for over a column that does not add up.
    #[error("cannot total a column of type {found}")]
    NotSummable {
        /// The type actually present.
        found: &'static str,
    },

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

    /// A join asked for something the executor does not offer.
    ///
    /// Refused at plan time rather than answered approximately: a join that
    /// silently ignored part of its condition would return more rows than it
    /// was asked for, and on a table under a row policy "more rows" is the
    /// failure that matters.
    #[error("join not supported: {reason}")]
    JoinNotSupported {
        /// What was asked for, and why it cannot be run.
        reason: String,
    },

    /// A predicate compared two columns that hold different types.
    ///
    /// [`Value`](slate_tuple::Value)'s order is type-first, which is what
    /// makes the key encoding sortable, so such a comparison would order by
    /// type and answer the same way for every row. Coercing instead would put
    /// value comparison and encoding order out of step, and scan bounds are
    /// derived from encoding order — so it is refused.
    #[error(
        "comparing {left:?} with {right:?} on `{at}`: they hold different types, \
         so the comparison would order by type rather than by value"
    )]
    ComparisonTypeMismatch {
        /// Where the comparison was written: a table, or a join of two.
        at: String,
        /// The column on the left of the operator.
        left: slate_schema::Ordinal,
        /// The column on the right of the operator.
        right: slate_schema::Ordinal,
    },

    /// A hash join's build side outgrew the memory it was allowed.
    ///
    /// Almost always a join condition that does not relate the two tables.
    /// Reported rather than absorbed, because the alternative to reporting is
    /// being killed by the allocator.
    #[error(
        "join build side exceeded {limit} rows on table `{table}`; \
         check the join condition, or raise the limit"
    )]
    JoinBuildTooLarge {
        /// The table being read into memory.
        table: String,
        /// The limit that was passed.
        limit: usize,
    },
}

impl KernelError {
    /// Whether retrying the whole transaction could succeed.
    ///
    /// Only a conflict qualifies. A constraint violation, an access denial or a
    /// decode failure will fail identically on every attempt, and
    /// [`KernelError::WriterFenced`] will fail forever by design — retrying any
    /// of them turns a clear error into a hang.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::TransactionConflict)
    }
}

/// Convenience alias for kernel results.
pub type Result<T> = core::result::Result<T, KernelError>;
