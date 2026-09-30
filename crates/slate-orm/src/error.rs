//! Errors surfaced by the typed layer.

use crate::record::RecordError;
use slate_kernel::KernelError;

/// Anything that can go wrong in a typed operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OrmError {
    /// The record layer refused or failed the operation.
    #[error(transparent)]
    Kernel(#[from] KernelError),

    /// A stored row did not fit its Rust type.
    #[error(transparent)]
    Record(#[from] RecordError),

    /// A row could not be generated. See [`crate::FactoryError`].
    ///
    /// A generation failure is not a kernel failure — nothing was written and
    /// no transaction was involved — but it arrives on the same `Result` as
    /// the insert that follows it, because a caller seeding a table writes one
    /// `?` chain from generate to commit and a second error type there buys
    /// nothing but a `map_err`.
    #[error(transparent)]
    Factory(#[from] crate::factory::FactoryError),

    /// A grouped read was asked for in a shape [`crate::Grouped`] cannot
    /// answer from.
    ///
    /// Its own variant rather than a `KernelError`, because the kernel is
    /// happy to compute `[Count, Count]` — it is *this* layer that cannot
    /// then say which of the two a `get(Count)` meant. Pushing it down would
    /// make the kernel refuse a request it can serve, for the benefit of a
    /// caller that may not be using the typed wrapper at all.
    #[error("{0}")]
    Grouping(String),
}

/// Convenience alias for typed results.
pub type Result<T> = core::result::Result<T, OrmError>;
