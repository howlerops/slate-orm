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
}

/// Convenience alias for typed results.
pub type Result<T> = core::result::Result<T, OrmError>;
