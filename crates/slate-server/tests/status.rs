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

use prost::Message as _;
use slate_kernel::KernelError;
use slate_kernel::error::StorageError;
use slate_schema::TableId;
use slate_server::code_for;
use slate_server::proto::rpc;
use tonic::Code;

/// The `google.rpc.ErrorInfo` a status carries in `grpc-status-details-bin`.
///
/// Decoded the way a client in another language would: parse the details as a
/// `google.rpc.Status`, find the `Any`, check its type URL, decode it. Written
/// out rather than helped along by anything in this crate, because the thing
/// under test is whether a client that has never seen this code can read it.
fn error_info(status: &tonic::Status) -> rpc::ErrorInfo {
    let details = rpc::Status::decode(status.details()).expect("details are a google.rpc.Status");
    let any = details.details.first().expect("one detail");
    assert_eq!(any.type_url, "type.googleapis.com/google.rpc.ErrorInfo");
    rpc::ErrorInfo::decode(any.value.as_slice()).expect("an ErrorInfo")
}

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

// --- what travels beside the code -----------------------------------------

/// The mapping above is many-to-one and cannot be otherwise, so the
/// distinctions have to survive somewhere. They survive in `ErrorInfo.reason`.
///
/// Asserted as a *set* rather than one by one: what matters is not that
/// `UniqueViolation` says `UNIQUE_VIOLATION` — that is a rename away from
/// meaning nothing — but that no two errors sharing a status code share a
/// reason. A reason that collapsed the same way the code does would be a
/// second lossy channel rather than a fix.
#[test]
fn no_two_errors_behind_one_status_code_share_a_reason() {
    let mut by_code: std::collections::HashMap<Code, Vec<String>> =
        std::collections::HashMap::new();
    for (error, _) in table_of_mappings() {
        let status = slate_server::from_kernel(&error);
        by_code
            .entry(status.code())
            .or_default()
            .push(error_info(&status).reason);
    }
    for (code, reasons) in by_code {
        let mut unique = reasons.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            reasons.len(),
            "{code:?} has {} errors behind it and {} reasons: {reasons:?}",
            reasons.len(),
            unique.len()
        );
    }
}

/// The row of the table that hurt: an application inserting a user wants to
/// say "that email address is already registered", and `ALREADY_EXISTS` cannot
/// tell it which constraint fired. `UniqueViolation` knows; now so does the
/// client, without matching on prose.
#[test]
fn a_unique_violation_names_its_index_in_the_details() {
    let status = slate_server::from_kernel(&KernelError::UniqueViolation {
        table: "users".to_owned(),
        index: "by_email".to_owned(),
    });
    let info = error_info(&status);
    assert_eq!(info.reason, "UNIQUE_VIOLATION");
    assert_eq!(info.domain, slate_server::DOMAIN);
    assert_eq!(
        info.metadata.get("index").map(String::as_str),
        Some("by_email")
    );
    assert_eq!(
        info.metadata.get("table").map(String::as_str),
        Some("users")
    );

    // The other error behind `ALREADY_EXISTS`, which a client must be able to
    // tell apart from it — "this id is taken" is a different sentence from
    // "this email is taken", and only one of them has an index.
    let duplicate = slate_server::from_kernel(&KernelError::DuplicatePrimaryKey {
        table: "users".to_owned(),
    });
    assert_eq!(duplicate.code(), status.code());
    assert_eq!(error_info(&duplicate).reason, "DUPLICATE_PRIMARY_KEY");
    assert!(!error_info(&duplicate).metadata.contains_key("index"));
}

/// The four errors behind `UNAVAILABLE` want four different responses: retry
/// elsewhere, retry later, lower your freshness, page someone.
#[test]
fn the_four_unavailable_errors_are_told_apart() {
    let cases = [
        (KernelError::WriterFenced, "WRITER_FENCED"),
        (
            KernelError::ReplicaTooStale {
                replica: "replica-0".to_owned(),
                required: 9,
                visible: 2,
            },
            "REPLICA_TOO_STALE",
        ),
        (
            KernelError::NoReplicaAvailable {
                reason: "the pool has no replicas",
            },
            "NO_REPLICA_AVAILABLE",
        ),
        (
            KernelError::Storage(StorageError::new(std::io::Error::other("no route to host"))),
            "STORAGE",
        ),
    ];
    for (error, reason) in cases {
        let status = slate_server::from_kernel(&error);
        assert_eq!(status.code(), Code::Unavailable);
        assert_eq!(error_info(&status).reason, reason);
    }

    // How far behind, and how far behind it needed to be: the two numbers that
    // decide whether to wait or to lower the freshness asked for.
    let stale = slate_server::from_kernel(&KernelError::ReplicaTooStale {
        replica: "replica-0".to_owned(),
        required: 9,
        visible: 2,
    });
    let info = error_info(&stale);
    assert_eq!(info.metadata.get("required").map(String::as_str), Some("9"));
    assert_eq!(info.metadata.get("visible").map(String::as_str), Some("2"));
    assert_eq!(
        info.metadata.get("replica").map(String::as_str),
        Some("replica-0")
    );
}

/// A refusal that is not a kernel error at all — the node is not the writer —
/// is `UNAVAILABLE` like three kernel errors are, so it needs a reason too.
/// The `slate-leader` trailer stays: it predates this and clients read it.
#[test]
fn a_redirect_carries_a_reason_and_keeps_its_trailer() {
    let status = slate_server::status::redirect("this node is not the writer", Some("node-2"));
    assert_eq!(status.code(), Code::Unavailable);
    let info = error_info(&status);
    assert_eq!(info.reason, "NOT_LEADER");
    assert_eq!(
        info.metadata.get("leader").map(String::as_str),
        Some("node-2")
    );
    assert_eq!(
        status
            .metadata()
            .get(slate_server::LEADER_KEY)
            .and_then(|v| v.to_str().ok()),
        Some("node-2")
    );
}

/// The details are the standard shape, so a client using its language's
/// rich-error helper reads them with no special support here.
#[test]
fn the_details_are_a_google_rpc_status_agreeing_with_the_outer_one() {
    let error = KernelError::AccessDenied {
        table: "users".to_owned(),
        action: "read",
    };
    let status = slate_server::from_kernel(&error);
    let details = rpc::Status::decode(status.details()).expect("a google.rpc.Status");
    assert_eq!(details.code, i32::from(status.code() as u8));
    assert_eq!(details.message, status.message());
    assert_eq!(
        error_info(&status)
            .metadata
            .get("action")
            .map(String::as_str),
        Some("read")
    );
}

/// A check violation names its column in the details, the same way a unique
/// violation names its index — and for the same reason.
///
/// The `message` travels in the status text, because it is a sentence and that
/// is where a sentence belongs. The `column` does not: it is the field a form
/// puts the sentence beside, and a client recovering it by parsing the text
/// would be matching on prose, which is the contract this avoids.
#[test]
fn a_check_violation_names_its_column_in_the_details() {
    let status = slate_server::from_kernel(&KernelError::Schema(
        slate_schema::SchemaError::CheckViolation {
            table: "docs".to_owned(),
            violations: vec![slate_schema::CheckFailure {
                check: "title_length".to_owned(),
                column: Some("title".to_owned()),
                message: Some("Title must be 1 to 80 characters.".to_owned()),
            }],
        },
    ));
    let info = error_info(&status);
    assert_eq!(info.reason, "CHECK_VIOLATION");
    assert_eq!(info.metadata.get("table").map(String::as_str), Some("docs"));
    assert_eq!(
        info.metadata.get("check").map(String::as_str),
        Some("title_length")
    );
    assert_eq!(
        info.metadata.get("column").map(String::as_str),
        Some("title")
    );
    assert!(
        status
            .message()
            .contains("Title must be 1 to 80 characters."),
        "{}",
        status.message()
    );
}

/// A check with no column omits the key rather than sending an empty one.
///
/// An empty string is a value: a client reading `column` would render the
/// error beside a field named "", which is worse than being told nothing and
/// falling back to a form-level error.
#[test]
fn a_check_violation_with_no_column_omits_the_key() {
    let status = slate_server::from_kernel(&KernelError::Schema(
        slate_schema::SchemaError::CheckViolation {
            table: "docs".to_owned(),
            violations: vec![slate_schema::CheckFailure {
                check: "discount_under_price".to_owned(),
                column: None,
                message: None,
            }],
        },
    ));
    let info = error_info(&status);
    assert_eq!(info.reason, "CHECK_VIOLATION");
    assert!(!info.metadata.contains_key("column"), "{:?}", info.metadata);
    // With no message the text is the default, which names the check.
    assert!(
        status.message().contains("discount_under_price"),
        "{}",
        status.message()
    );
}

/// Several failures travel as an indexed set, message included.
///
/// This is the whole point of collecting them: a form with three bad fields
/// gets three field-to-message pairs from one round trip. A client that can
/// only show one error still reads the unindexed `check`/`column`, which are
/// the first failure.
#[test]
fn every_failing_check_reaches_the_client_with_its_own_message() {
    let status = slate_server::from_kernel(&KernelError::Schema(
        slate_schema::SchemaError::CheckViolation {
            table: "docs".to_owned(),
            violations: vec![
                slate_schema::CheckFailure {
                    check: "title_length".to_owned(),
                    column: Some("title".to_owned()),
                    message: Some("Title must be 1 to 80 characters.".to_owned()),
                },
                slate_schema::CheckFailure {
                    check: "size_positive".to_owned(),
                    column: Some("size".to_owned()),
                    message: Some("Size cannot be negative.".to_owned()),
                },
                slate_schema::CheckFailure {
                    check: "discount_under_price".to_owned(),
                    column: None,
                    message: None,
                },
            ],
        },
    ));
    let info = error_info(&status);
    assert_eq!(info.reason, "CHECK_VIOLATION");
    assert_eq!(
        info.metadata.get("violations").map(String::as_str),
        Some("3")
    );

    let get = |key: &str| info.metadata.get(key).map(String::as_str);
    assert_eq!(get("check.0"), Some("title_length"));
    assert_eq!(get("column.0"), Some("title"));
    assert_eq!(get("message.0"), Some("Title must be 1 to 80 characters."));
    assert_eq!(get("check.1"), Some("size_positive"));
    assert_eq!(get("column.1"), Some("size"));
    assert_eq!(get("message.1"), Some("Size cannot be negative."));
    // The cross-column one contributes a name and nothing to hang it on.
    assert_eq!(get("check.2"), Some("discount_under_price"));
    assert_eq!(get("column.2"), None);
    assert_eq!(get("message.2"), None);

    // The unindexed pair is the first, for a one-error client.
    assert_eq!(get("check"), Some("title_length"));
    assert_eq!(get("column"), Some("title"));

    // The text summarises rather than naming only the first, because naming
    // only the first is what this change exists to stop.
    let text = status.message();
    assert!(text.contains("3 checks"), "{text}");
    assert!(text.contains("title_length"), "{text}");
    assert!(text.contains("size_positive"), "{text}");
    assert!(text.contains("discount_under_price"), "{text}");
}

/// One failure reads exactly as it did before several were possible.
///
/// Worth pinning: the wording of the single-failure case is what every log
/// line and every existing test matches on, and a list of one rendering as
/// "row violates 1 checks" would be a gratuitous break.
#[test]
fn a_single_failure_reads_the_way_it_always_did() {
    let status = slate_server::from_kernel(&KernelError::Schema(
        slate_schema::SchemaError::CheckViolation {
            table: "docs".to_owned(),
            violations: vec![slate_schema::CheckFailure {
                check: "size_positive".to_owned(),
                column: None,
                message: None,
            }],
        },
    ));
    assert_eq!(
        status.message(),
        "row violates check `size_positive` on table `docs`"
    );
}
