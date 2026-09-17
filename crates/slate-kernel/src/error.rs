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

    /// A write to the store failed, so this transaction holds part of a
    /// statement and will not commit.
    ///
    /// Deliberately *not* retryable in the sense [`TransactionConflict`] is:
    /// the write failed for a reason of its own — a full disk, a fenced
    /// writer — and the caller has to begin again rather than commit what is
    /// buffered. Raised by `commit` rather than swallowed, so a caller that
    /// read the earlier error as "some of this worked" is told otherwise
    /// instead of landing a row without its index entries.
    #[error("transaction holds a partly written statement and cannot commit; begin again")]
    TransactionPoisoned,

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

    /// A conditional write found the row already changed.
    ///
    /// Distinct from [`KernelError::TransactionConflict`], and the distinction
    /// is the whole point: a transaction conflict is two writers overlapping in
    /// *time*, and retrying is the right response. This is a writer whose
    /// decision was made from a row that is no longer there — retrying the same
    /// write would apply an edit computed from stale data, which is the lost
    /// update it exists to prevent. Re-read, re-decide, then write.
    #[error("row in `{table}` changed since it was read; re-read it before writing")]
    RowChanged {
        /// The table written to.
        table: String,
    },

    /// A pagination cursor cannot be applied to this query.
    ///
    /// Its own variant rather than a general-purpose "invalid", because the
    /// caller's next move depends on which reason it is: a malformed cursor is
    /// a bug in their code, and a cursor over a query that walks an index is a
    /// query they have to change.
    #[error("cannot resume `{table}` from this cursor: {reason}")]
    InvalidCursor {
        /// The table being paged.
        table: String,
        /// Why not.
        reason: String,
    },

    /// A migration will not run, and why.
    ///
    /// Its own variant rather than a general-purpose "invalid": every use is a
    /// schema that cannot be reconciled with what is on disk, and an operator
    /// reading a log wants to know that before reading the sentence.
    #[error("migration refused: {reason}")]
    MigrationRefused {
        /// What is in the way.
        reason: String,
    },

    /// Another writer has taken over; this one is no longer the writer.
    ///
    /// Terminal, and deliberately distinct from [`KernelError::TransactionConflict`].
    /// A single-writer deployment fences the previous writer when a new one
    /// starts, so a process that retried through this would be a split brain
    /// still trying to write. It must stop instead.
    #[error("this writer has been fenced by a newer one and must stop")]
    WriterFenced,

    /// A commit did not finish within the configured limit.
    ///
    /// **This does not mean the write did not happen.** The commit may have
    /// been applied already, may land later, or may never land; the timeout
    /// says only that nobody knows yet. That is why it is not retryable —
    /// a caller that retried could apply the same change twice.
    ///
    /// Raised only when a backend is configured with a commit timeout. It
    /// exists because a stalled object store makes SlateDB wait indefinitely
    /// rather than fail, and a writer that hangs with no error is harder to
    /// operate than one that stops loudly. Reconcile from what is actually in
    /// the store rather than assuming either outcome.
    #[error("commit did not complete within the configured timeout; its outcome is unknown")]
    CommitTimedOut,

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

    /// A predicate write named a column the table does not have.
    ///
    /// Refused rather than skipped. An assignment to an ordinal past the end of
    /// the row is a caller whose idea of the schema has drifted from the
    /// catalog's, and silently dropping it writes every *other* assignment
    /// against a row shape nobody checked.
    #[error("`{table}` has no column at {ordinal:?}")]
    NoSuchColumn {
        /// The table the write named.
        table: String,
        /// The ordinal that is not one of its columns.
        ordinal: slate_schema::Ordinal,
    },

    /// A predicate write assigned to the same column twice.
    ///
    /// Refused rather than resolved, because both resolutions are a guess.
    /// Last-wins silently discards the first assignment; first-wins silently
    /// discards the second; and the caller who wrote both meant something, so
    /// the only answer that cannot be wrong is to say so.
    #[error("`{table}` is assigned at {ordinal:?} twice in one update")]
    DuplicateAssignment {
        /// The table the write named.
        table: String,
        /// The ordinal assigned more than once.
        ordinal: slate_schema::Ordinal,
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
    #[error(
        "grouping produced more than {limit} distinct keys; \
         group by fewer columns, filter first, or raise the limit"
    )]
    /// A `GROUP BY` produced more distinct keys than the node will hold.
    TooManyGroups {
        /// The limit that was passed.
        limit: usize,
    },
    #[error(
        "COUNT(DISTINCT) saw more than {limit} distinct values; \
         filter first, or raise the limit"
    )]
    /// A `COUNT(DISTINCT)` saw more distinct values than the node will hold.
    TooManyDistinctValues {
        /// The limit that was passed.
        limit: usize,
    },
    #[error(
        "an ORDER BY with no LIMIT selected more than {limit} rows; \
         add a LIMIT, which sorts with a bounded heap instead"
    )]
    /// An `ORDER BY` with no `LIMIT` selected more rows than the node will
    /// materialise. A `LIMIT` makes the sort bounded and this unreachable.
    SortTooLarge {
        /// The limit that was passed.
        limit: usize,
    },

    /// A predicate write matched more rows than the caller allowed it to.
    ///
    /// Unlike the three limits above this one is not a memory guard, and it is
    /// per call rather than per store: a caller passes it when it has to
    /// *deliver* the matched rows and knows a ceiling on how many it can
    /// carry. The head node passes it for a predicate write with `RETURNING`,
    /// because a response is one message and a large enough set of rows makes
    /// it undeliverable.
    ///
    /// The refusal happens before anything is written, which is the whole
    /// point: without it the write commits and then the response fails to
    /// arrive, so the caller sees an error for a delete that really happened.
    #[error(
        "a predicate write matched more than {limit} rows, and its rows were asked for; \
         narrow the predicate, drop the request for the rows, or raise the limit"
    )]
    PredicateWriteTooLarge {
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
    ///
    /// [`KernelError::CommitTimedOut`] is excluded for the opposite reason: it
    /// might well succeed on a retry, but the first attempt may also have
    /// landed, so retrying risks applying the change twice.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::TransactionConflict)
    }
}

/// Convenience alias for kernel results.
pub type Result<T> = core::result::Result<T, KernelError>;
