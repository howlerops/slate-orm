//! Turning a kernel error into a gRPC status.
//!
//! The code matters more than the message. A gRPC client library retries on
//! `UNAVAILABLE` and on nothing else by default, so the code is not a label —
//! it is an instruction to every client in the fleet. Two of the mappings below
//! exist entirely because of that:
//!
//! - [`KernelError::CommitTimedOut`] becomes `UNKNOWN`, not `DEADLINE_EXCEEDED`.
//!   The write may already have landed. `DEADLINE_EXCEEDED` invites a retry,
//!   and a retried insert that already succeeded is a duplicate row.
//! - [`KernelError::WriterFenced`] becomes `UNAVAILABLE`, not
//!   `FAILED_PRECONDITION`. Another node is the writer now, so the request
//!   should be retried — just not here. The `slate-leader` trailer names where.
//!
//! # This cannot be exhaustive, and says so
//!
//! `KernelError` is `#[non_exhaustive]`, so a `match` over it needs a wildcard
//! and a new variant will compile straight through into whatever that wildcard
//! says. The wildcard is `INTERNAL`, chosen because it is the code no client
//! retries: a variant nobody has classified is one nobody has established is
//! safe to repeat.
//!
//! `tests/status.rs` pins every variant that exists today, so a mapping that
//! changes has to be changed deliberately. It cannot pin one that does not
//! exist yet.

use slate_kernel::KernelError;
use tonic::{Code, Status};

/// The metadata key naming the node a write should go to instead.
pub const LEADER_KEY: &str = "slate-leader";

/// A kernel error as a gRPC status.
#[must_use]
pub fn from_kernel(error: &KernelError) -> Status {
    let code = code_for(error);
    Status::new(code, error.to_string())
}

/// The code a kernel error carries.
///
/// Separate from [`from_kernel`] so the classification can be tested without
/// depending on the wording of any message.
#[must_use]
pub fn code_for(error: &KernelError) -> Code {
    match error {
        // The caller asked for something that already exists. Retrying will
        // not change that.
        KernelError::UniqueViolation { .. } | KernelError::DuplicatePrimaryKey { .. } => {
            Code::AlreadyExists
        }

        // A row the policy hides also reads as absent, which is the point: the
        // status must not distinguish "not there" from "not yours".
        KernelError::RowNotFound { .. } => Code::NotFound,
        KernelError::UnknownTable(_) => Code::NotFound,

        // All three are the security layer refusing. `TenantRequired` is a
        // refusal to serve a request that carries no tenant, which is a denial
        // rather than a bad argument: telling the caller to add a tenant would
        // be telling them how to widen their own access.
        KernelError::AccessDenied { .. }
        | KernelError::TenantRequired { .. }
        | KernelError::RowCheckFailed { .. } => Code::PermissionDenied,

        // The one code gRPC defines as "retry the whole transaction".
        KernelError::TransactionConflict => Code::Aborted,

        // Another node is the writer. Retry, elsewhere.
        KernelError::WriterFenced => Code::Unavailable,

        // The outcome is genuinely unknown, and `UNKNOWN` is the code that
        // says so without inviting a retry.
        KernelError::CommitTimedOut => Code::Unknown,

        // Routing could not find a view fresh enough. A later attempt may find
        // one, so it is retryable — that is the whole reason the pool refuses
        // rather than serving a stale read.
        KernelError::ReplicaTooStale { .. } | KernelError::NoReplicaAvailable { .. } => {
            Code::Unavailable
        }

        // Object storage failed. Almost always transient, and the caller has
        // nothing to fix.
        KernelError::Storage(_) => Code::Unavailable,

        // The request does not make sense against this schema. Nothing the
        // server can do differently on a second attempt.
        KernelError::Schema(_)
        | KernelError::NotSummable { .. }
        | KernelError::ComparisonTypeMismatch { .. }
        | KernelError::JoinNotSupported { .. } => Code::InvalidArgument,

        // A stored key or index entry did not decode. This is the index and
        // the table disagreeing, which the write path is built to prevent, so
        // it is corruption rather than a bad request.
        KernelError::KeyDecode(_) | KernelError::CorruptIndexEntry { .. } => Code::DataLoss,

        // The server declined to use the memory the query would need.
        KernelError::JoinBuildTooLarge { .. } => Code::ResourceExhausted,

        // See the module docs: unclassified means do not retry.
        _ => Code::Internal,
    }
}

/// A status that also tells the caller which node to try instead.
///
/// The hint is best effort — the lease may have moved again by the time it
/// arrives — so it goes in the metadata rather than the message, where a client
/// that wants it can find it and one that does not is unaffected.
#[must_use]
pub fn redirect(message: impl Into<String>, leader: Option<&str>) -> Status {
    let mut status = Status::new(Code::Unavailable, message);
    if let Some(leader) = leader
        && let Ok(value) = leader.parse()
    {
        status.metadata_mut().insert(LEADER_KEY, value);
    }
    status
}
