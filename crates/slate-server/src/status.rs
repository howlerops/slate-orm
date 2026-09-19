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
use slate_schema::SchemaError;
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

/// The stable reason token carried in a `Status`'s details, or `""`.
///
/// The inverse of [`with_details`], and it exists for one caller: a batch
/// reports each independent operation's failure as data rather than as an
/// error, so it has to put back into a message what the lone RPC puts into
/// trailers. Decoding what this module encoded is closing a loop rather than
/// parsing something foreign — if the encoding changes, this fails to compile
/// beside it.
///
/// `""` for a status that carries no details, which is every status built by
/// hand at a handler rather than from a [`KernelError`]. A caller reads that
/// as "no stable token", which is true: those messages are prose.
pub fn reason_of(status: &Status) -> String {
    let Ok(decoded) = rpc::Status::decode(status.details()) else {
        return String::new();
    };
    for detail in &decoded.details {
        if detail.type_url == ERROR_INFO_URL
            && let Ok(info) = rpc::ErrorInfo::decode(detail.value.as_slice())
        {
            return info.reason;
        }
    }
    String::new()
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
        | KernelError::RowCheckFailed { table }
        | KernelError::InvalidCursor { table, .. } => put("table", table.clone()),
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
        // The same reasoning as `UniqueViolation` above, one step further. The
        // status message already carries the check's `message` when the schema
        // wrote one, because it is part of the error's text; `column` is not
        // text, it is the field a form puts the error beside, and a caller
        // parsing it out of a sentence is the contract this avoids.
        KernelError::NotSoftDeleting { table } => put("table", table.clone()),
        KernelError::Schema(SchemaError::CheckViolation { table, violations }) => {
            put("table", table.clone());
            put("violations", violations.len().to_string());
            // Indexed rather than comma-joined. A join needs a separator that
            // cannot occur in a name; identifiers cannot contain a comma
            // today, and relying on that is a constraint nobody wrote down.
            // Indexed keys also carry the *message* per failure, which a form
            // needs to build a field-to-error map and cannot recover from the
            // status text without parsing prose.
            for (at, failure) in violations.iter().enumerate() {
                put(&format!("check.{at}"), failure.check.clone());
                if let Some(column) = &failure.column {
                    put(&format!("column.{at}"), column.clone());
                }
                if let Some(message) = &failure.message {
                    put(&format!("message.{at}"), message.clone());
                }
            }
            // The unindexed pair is the first failure, kept for the common
            // client that shows one error at a time and should not have to
            // learn the indexed form to do it.
            if let Some(first) = violations.first() {
                put("check", first.check.clone());
                if let Some(column) = &first.column {
                    put("column", column.clone());
                }
            }
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
        // Split out of `SCHEMA` deliberately. A check violation is the
        // caller's *data* being wrong, which a form retries after editing a
        // field; every other schema error is the caller's *schema* being
        // wrong, which retrying cannot fix. Collapsing them is exactly the
        // loss of information this token exists to undo. Safe to add: nothing
        // in any client matched `SCHEMA` when this was written.
        KernelError::Schema(SchemaError::CheckViolation { .. }) => "CHECK_VIOLATION",
        KernelError::Schema(_) => "SCHEMA",
        KernelError::NotSummable { .. } => "NOT_SUMMABLE",
        KernelError::ComparisonTypeMismatch { .. } => "COMPARISON_TYPE_MISMATCH",
        KernelError::JoinNotSupported { .. } => "JOIN_NOT_SUPPORTED",
        KernelError::DecimalScale { .. } => "DECIMAL_SCALE",
        KernelError::KeyDecode(_) => "KEY_DECODE",
        KernelError::CorruptIndexEntry { .. } => "CORRUPT_INDEX_ENTRY",
        KernelError::JoinBuildTooLarge { .. } => "JOIN_BUILD_TOO_LARGE",
        KernelError::InvalidCursor { .. } => "INVALID_CURSOR",
        KernelError::PredicateWriteTooLarge { .. } => "PREDICATE_WRITE_TOO_LARGE",
        KernelError::NotSoftDeleting { .. } => "NOT_SOFT_DELETING",
        KernelError::SortTooLarge { .. } => "SORT_TOO_LARGE",
        KernelError::TooManyGroups { .. } => "TOO_MANY_GROUPS",
        KernelError::TooManyDistinctValues { .. } => "TOO_MANY_DISTINCT_VALUES",
        KernelError::DuplicateAssignment { .. } => "DUPLICATE_ASSIGNMENT",
        KernelError::NoSuchColumn { .. } => "NO_SUCH_COLUMN",
        KernelError::RowChanged { .. } => "ROW_CHANGED",
        KernelError::MigrationRefused { .. } => "MIGRATION_REFUSED",
        KernelError::TransactionPoisoned => "TRANSACTION_POISONED",
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
        | KernelError::JoinNotSupported { .. }
        // A computed expression over a decimal whose answer would be at no
        // scale. The caller's expression, not the server's problem — and it
        // reached the wire as `INTERNAL`/`UNCLASSIFIED` until the conformance
        // corpus grew a case that provoked it, which is the same way
        // `InvalidCursor` above was found. A retry on `INTERNAL` fails
        // identically forever, so the classification is the whole fix.
        | KernelError::DecimalScale { .. } => Code::InvalidArgument,

        // A cursor that is not a whole primary key, or one on a read that
        // cannot be resumed from a key — an index scan, a sort the key does
        // not give, a grouped read. All of them are the request, and the
        // message says which.
        //
        // This fell to the wildcard and came back `INTERNAL` until the wire
        // grew a cursor field, because until then no remote caller could set
        // one and the mapping was unreachable. A caller cannot tell "I sent a
        // bad request" from "the server broke" at `INTERNAL`, and the second
        // invites a retry that will fail identically forever.
        KernelError::InvalidCursor { .. } => Code::InvalidArgument,
        // `FailedPrecondition`, not `InvalidArgument`: the request is
        // well-formed and the *table* is the wrong shape for it. gRPC's own
        // guidance draws that line — `InvalidArgument` is an argument bad
        // regardless of state, `FailedPrecondition` is an argument that would
        // be fine against a differently-configured system. Declaring
        // `soft_delete` makes this same call succeed.
        KernelError::NotSoftDeleting { .. } => Code::FailedPrecondition,

        // A stored key or index entry did not decode. This is the index and
        // the table disagreeing, which the write path is built to prevent, so
        // it is corruption rather than a bad request.
        KernelError::KeyDecode(_) | KernelError::CorruptIndexEntry { .. } => Code::DataLoss,

        // The server declined to use the memory the query would need.
        //
        // All four are the same refusal at four different stages, and the
        // other three reached the wildcard until predicate writes went looking
        // for it: a caller that sorted more rows than the limit allows, or
        // grouped into more groups, got `INTERNAL` — which reads as "the
        // server broke" and invites a retry that will exceed the same limit
        // again. They are reachable from any sorted or grouped query and have
        // been since those went on the wire.
        //
        // `PredicateWriteTooLarge` joins them, and is the only one of the five
        // that is about the *answer* rather than the node's memory: it fires
        // when a predicate write matched more rows than one response can
        // carry. `ResourceExhausted` all the same, and for the same reason —
        // the caller's remedy is a narrower request.
        KernelError::JoinBuildTooLarge { .. }
        | KernelError::SortTooLarge { .. }
        | KernelError::TooManyGroups { .. }
        | KernelError::TooManyDistinctValues { .. }
        | KernelError::PredicateWriteTooLarge { .. } => Code::ResourceExhausted,

        // The caller's own request, malformed against this schema.
        // `DuplicateAssignment` and `NoSuchColumn` became reachable when
        // predicate writes crossed the wire; before that nothing could send an
        // assignment at all.
        KernelError::DuplicateAssignment { .. } | KernelError::NoSuchColumn { .. } => {
            Code::InvalidArgument
        }

        // The stored row moved between the read and the write. Re-read,
        // recompute, retry — the same shape as `TransactionConflict`, and the
        // same code. Not reachable from the wire yet: conditional writes are
        // kernel-only. Classified anyway, because the next person to put them
        // on the wire should not have to discover this the way this commit
        // discovered the three above.
        KernelError::RowChanged { .. } => Code::Aborted,

        // Both are "not in this state": a migration the catalog will not
        // accept as it stands, and a transaction that is unusable after an
        // error and must be rolled back. Retrying either unchanged fails
        // identically, which is what `FAILED_PRECONDITION` says and `ABORTED`
        // does not.
        KernelError::MigrationRefused { .. } | KernelError::TransactionPoisoned => {
            Code::FailedPrecondition
        }

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
