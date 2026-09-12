//! Which gRPC code each kernel error carries.
//!
//! This looks like a table test and is not one. A gRPC client library retries
//! on `UNAVAILABLE` and on nothing else by default, so the code is not a label,
//! it is an instruction to every client in the fleet — and two of the mappings
//! exist only because of that. Changing one silently changes what a thousand
//! clients do with a failure.
//!
//! `KernelError` is `#[non_exhaustive]`, so this cannot be made exhaustive at
//! compile time and it does not pretend to be: a variant added later falls
//! through to `INTERNAL`, which is the code nobody retries. That is the safe
//! direction, and `an_unclassified_error_is_not_retryable` says so out loud.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::KernelError;
use slate_kernel::error::StorageError;
use slate_schema::TableId;
use slate_server::code_for;
use tonic::Code;

fn table_of_mappings() -> Vec<(KernelError, Code)> {
    vec![
        (
            KernelError::UniqueViolation {
                table: "users".to_owned(),
                index: "by_email".to_owned(),
            },
            Code::AlreadyExists,
        ),
        (
            KernelError::DuplicatePrimaryKey {
                table: "users".to_owned(),
            },
            Code::AlreadyExists,
        ),
        (
            KernelError::RowNotFound {
                table: "users".to_owned(),
            },
            Code::NotFound,
        ),
        (KernelError::UnknownTable(TableId(9)), Code::NotFound),
        (
            KernelError::AccessDenied {
                table: "users".to_owned(),
                action: "read",
            },
            Code::PermissionDenied,
        ),
        (
            KernelError::TenantRequired {
                table: "users".to_owned(),
            },
            Code::PermissionDenied,
        ),
        (
            KernelError::RowCheckFailed {
                table: "users".to_owned(),
            },
            Code::PermissionDenied,
        ),
        (KernelError::TransactionConflict, Code::Aborted),
        (KernelError::WriterFenced, Code::Unavailable),
        (KernelError::CommitTimedOut, Code::Unknown),
        (
            KernelError::ReplicaTooStale {
                replica: "replica-0".to_owned(),
                required: 9,
                visible: 2,
            },
            Code::Unavailable,
        ),
        (
            KernelError::NoReplicaAvailable {
                reason: "the pool has no replicas",
            },
            Code::Unavailable,
        ),
        (
            KernelError::Storage(StorageError::new(std::io::Error::other("no route to host"))),
            Code::Unavailable,
        ),
        (
            KernelError::NotSummable { found: "string" },
            Code::InvalidArgument,
        ),
        (
            KernelError::JoinNotSupported {
                reason: "a nested loop cannot serve a right outer join".to_owned(),
            },
            Code::InvalidArgument,
        ),
        (
            KernelError::CorruptIndexEntry {
                table: "users".to_owned(),
                index: "by_email".to_owned(),
            },
            Code::DataLoss,
        ),
        (
            KernelError::JoinBuildTooLarge {
                table: "users".to_owned(),
                limit: 100_000,
            },
            Code::ResourceExhausted,
        ),
    ]
}

#[test]
fn every_error_the_kernel_defines_today_has_a_code_that_was_chosen() {
    for (error, expected) in table_of_mappings() {
        let actual = code_for(&error);
        assert_eq!(
            actual, expected,
            "`{error}` maps to {actual:?}, not {expected:?}"
        );
    }
}

/// The two mappings that are easy to get wrong, called out on their own so a
/// change to either fails a test whose name says what is at stake.
#[test]
fn a_timed_out_commit_does_not_invite_a_retry() {
    // The write may already have landed. `DEADLINE_EXCEEDED` is the obvious
    // code and it is the wrong one: a retried insert that already succeeded is
    // a duplicate row. `UNKNOWN` says what is true.
    assert_eq!(code_for(&KernelError::CommitTimedOut), Code::Unknown);
    assert_ne!(
        code_for(&KernelError::CommitTimedOut),
        Code::DeadlineExceeded
    );
}

#[test]
fn being_fenced_tells_the_client_to_retry_elsewhere() {
    // `FAILED_PRECONDITION` would tell the client to stop until an operator
    // fixes something. Nothing is broken: another node is the writer now.
    assert_eq!(code_for(&KernelError::WriterFenced), Code::Unavailable);
}

#[test]
fn a_conflict_is_aborted_which_is_the_code_that_means_retry_the_transaction() {
    assert_eq!(code_for(&KernelError::TransactionConflict), Code::Aborted);
}

#[test]
fn the_message_survives_into_the_status() {
    let error = KernelError::UniqueViolation {
        table: "users".to_owned(),
        index: "by_email".to_owned(),
    };
    let status = slate_server::from_kernel(&error);
    assert!(
        status.message().contains("by_email"),
        "the index should be named so a caller can act on it: {}",
        status.message()
    );
}

/// A variant nobody has classified must not be one clients retry.
///
/// No variant reaches the wildcard today and none can be constructed from
/// outside the kernel, so the rule cannot be tested directly. What can be
/// tested is that `INTERNAL` is reserved for it: no classified error uses that
/// code, so a variant that starts falling through will be distinguishable from
/// every one that was thought about.
#[test]
fn an_unclassified_error_is_not_retryable() {
    let classified: Vec<Code> = table_of_mappings()
        .into_iter()
        .map(|(error, _)| code_for(&error))
        .collect();
    assert!(
        !classified.contains(&Code::Internal),
        "INTERNAL is the wildcard, so no classified error may also use it — \
         otherwise a variant that fell through would be indistinguishable"
    );
}
