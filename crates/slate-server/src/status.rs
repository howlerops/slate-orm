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
//!
//! # The code is lossy, so something structured travels with it
//!
//! The mapping above is many-to-one and cannot be anything else: `UNAVAILABLE`
//! is four kernel errors that want four different responses (retry elsewhere,
//! retry later, lower your freshness, page someone), and `ALREADY_EXISTS` is
//! two — "that id is taken" and "that **email** is taken". The second is the
//! one that hurts, because [`KernelError::UniqueViolation`]'s own payload
//! names the index, which is exactly what an application needs to say
//! "that email address is already registered".
//!
//! The distinction is recoverable from the message, and a message is not an
//! interface: nothing tests its wording, and a client that branches on it
//! breaks the day somebody improves it. So every status carries a
//! [`google.rpc.ErrorInfo`](crate::proto::rpc::ErrorInfo) in
//! `grpc-status-details-bin` as well:
//!
//! - `reason` — a stable token per kernel variant, [`reason_for`]. It is
//!   `SCREAMING_SNAKE_CASE`, is part of the wire contract, and changing one is
//!   a breaking change in the way changing a status code is.
//! - `domain` — [`DOMAIN`], the namespace those tokens live in.
//! - `metadata` — the variant's own payload, one entry per field: `table`,
//!   `index`, `action`, `replica`, `required`, `visible`, `limit`.
//!
//! That is the standard shape, so a client using its language's rich-error
//! helper gets it with no special support here, and one that ignores details
//! is unaffected. The `slate-leader` trailer stays where it is — it predates
//! this, clients read it, and a redirect that only a rich-error client could
//! follow would be worse.
//!
//! An unclassified variant gets `reason: UNCLASSIFIED` rather than a token
//! invented from its `Debug`. A token is a promise that the meaning is stable,
//! and nobody has decided what an unclassified variant means; the wildcard is
//! `INTERNAL` for the same reason.

use crate::proto::rpc;
use prost::Message as _;
use slate_kernel::KernelError;
use std::collections::HashMap;
use tonic::{Code, Status};

/// The metadata key naming the node a write should go to instead.
pub const LEADER_KEY: &str = "slate-leader";

/// The namespace [`reason_for`]'s tokens belong to.
///
/// `google.rpc.ErrorInfo` calls for "the logical grouping to which the reason
/// belongs", usually a service name. It is a name, not a URL, and nothing
/// dereferences it.
pub const DOMAIN: &str = "slate-orm";

/// The type URL a `google.rpc.ErrorInfo` is packed under.
///
/// Written out rather than derived from a `prost::Name` impl, which needs
/// `enable_type_names()` in the build and would put the same constant in a
/// generated file instead of a visible one.
const ERROR_INFO_URL: &str = "type.googleapis.com/google.rpc.ErrorInfo";

/// A kernel error as a gRPC status.
#[must_use]
pub fn from_kernel(error: &KernelError) -> Status {
    let code = code_for(error);
    let message = error.to_string();
    with_details(code, message, error_info(error))
}

/// A status carrying `info` in `grpc-status-details-bin`.
fn with_details(code: Code, message: String, info: rpc::ErrorInfo) -> Status {
    let status = rpc::Status {
        code: i32::from(code as u8),
        message: message.clone(),
        // Hand-built rather than `Any::from_msg`, which needs the generated
        // type to carry a `prost::Name`.
        details: vec![prost_types::Any {
            type_url: ERROR_INFO_URL.to_owned(),
            value: info.encode_to_vec(),
        }],
    };
    Status::with_details(code, message, status.encode_to_vec().into())
}

/// The stable token for a kernel error, and its payload.
fn error_info(error: &KernelError) -> rpc::ErrorInfo {
    let mut metadata = HashMap::new();
    let mut put = |key: &str, value: String| {
        metadata.insert(key.to_owned(), value);
    };
    match error {
        KernelError::UniqueViolation { table, index } => {
            put("table", table.clone());
            // The whole reason this exists: "which constraint fired" is the
            // difference between "that id is taken" and "that email address is
            // already registered", and the status code cannot hold it.
            put("index", index.clone());
        }
        KernelError::DuplicatePrimaryKey { table }
        | KernelError::RowNotFound { table }
        | KernelError::TenantRequired { table }
        | KernelError::RowCheckFailed { table } => put("table", table.clone()),
        KernelError::AccessDenied { table, action } => {
            put("table", table.clone());
            put("action", (*action).to_owned());
        }
        KernelError::UnknownTable(id) => put("table_id", id.0.to_string()),
        KernelError::ReplicaTooStale {
            replica,
            required,
            visible,
        } => {
            put("replica", replica.clone());
            put("required", required.to_string());
            put("visible", visible.to_string());
        }
        KernelError::NoReplicaAvailable { reason } => put("why", (*reason).to_owned()),
        KernelError::NotSummable { found } => put("type", (*found).to_owned()),
        KernelError::CorruptIndexEntry { table, index } => {
            put("table", table.clone());
            put("index", index.clone());
        }
        KernelError::JoinBuildTooLarge { table, limit } => {
            put("table", table.clone());
            put("limit", limit.to_string());
        }
        _ => {}
    }
    rpc::ErrorInfo {
        reason: reason_for(error).to_owned(),
        domain: DOMAIN.to_owned(),
        metadata,
    }
}

/// The stable token a kernel error carries in its details.
///
/// One per variant, so the collapse the status code performs is undone. These
/// are wire contract: a client matches on them, and renaming one breaks it as
/// surely as changing a code would.
#[must_use]
pub fn reason_for(error: &KernelError) -> &'static str {
    match error {
        KernelError::UniqueViolation { .. } => "UNIQUE_VIOLATION",
        KernelError::DuplicatePrimaryKey { .. } => "DUPLICATE_PRIMARY_KEY",
        KernelError::RowNotFound { .. } => "ROW_NOT_FOUND",
        KernelError::UnknownTable(_) => "UNKNOWN_TABLE",
        KernelError::AccessDenied { .. } => "ACCESS_DENIED",
        KernelError::TenantRequired { .. } => "TENANT_REQUIRED",
        KernelError::RowCheckFailed { .. } => "ROW_CHECK_FAILED",
        KernelError::TransactionConflict => "TRANSACTION_CONFLICT",
        KernelError::WriterFenced => "WRITER_FENCED",
        KernelError::CommitTimedOut => "COMMIT_TIMED_OUT",
        KernelError::ReplicaTooStale { .. } => "REPLICA_TOO_STALE",
        KernelError::NoReplicaAvailable { .. } => "NO_REPLICA_AVAILABLE",
        KernelError::Storage(_) => "STORAGE",
        KernelError::Schema(_) => "SCHEMA",
        KernelError::NotSummable { .. } => "NOT_SUMMABLE",
        KernelError::ComparisonTypeMismatch { .. } => "COMPARISON_TYPE_MISMATCH",
        KernelError::JoinNotSupported { .. } => "JOIN_NOT_SUPPORTED",
        KernelError::KeyDecode(_) => "KEY_DECODE",
        KernelError::CorruptIndexEntry { .. } => "CORRUPT_INDEX_ENTRY",
        KernelError::JoinBuildTooLarge { .. } => "JOIN_BUILD_TOO_LARGE",
        // See the module docs: a token is a promise of a stable meaning, and
        // nobody has decided this one's.
        _ => "UNCLASSIFIED",
    }
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
    let mut metadata = HashMap::new();
    if let Some(leader) = leader {
        metadata.insert("leader".to_owned(), leader.to_owned());
    }
    // Not a `KernelError`: this refusal is local, from the watch channel,
    // before any call into storage. It still gets a reason, because a client
    // distinguishing "not the writer" from "the storage is down" is the whole
    // point of the reason — both are `UNAVAILABLE`.
    let mut status = with_details(
        Code::Unavailable,
        message.into(),
        rpc::ErrorInfo {
            reason: "NOT_LEADER".to_owned(),
            domain: DOMAIN.to_owned(),
            metadata,
        },
    );
    if let Some(leader) = leader
        && let Ok(value) = leader.parse()
    {
        status.metadata_mut().insert(LEADER_KEY, value);
    }
    status
}
