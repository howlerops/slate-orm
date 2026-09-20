//! Cross-tenant probes over the wire, for a security review.
//!
//! The kernel-level demonstration is in
//! `crates/slate-kernel/tests/security_probe_cascade.rs`; this establishes that
//! the same oracle is reachable by an ordinary gRPC client with nothing but an
//! `app` role and a tenant of its own.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    unreachable_pub
)]

mod common;

use common::{app_in, claim, serving_leader, user};
use slate_kernel::memory::MemoryStore;
use slate_server::proto as pb;
use std::sync::Arc;
use tonic::Code;

fn wire(row: &slate_schema::Row) -> pb::Row {
    slate_server::convert::row_to_proto(row)
}

fn insert(rows: Vec<pb::Row>) -> pb::InsertRequest {
    pb::InsertRequest {
        transaction: String::new(),
        table: "users".to_owned(),
        rows,
        upsert: false,
        schema: Some(claim("users")),
    }
}

/// FINDING: an ordinary client in tenant 1 learns which primary keys exist in
/// tenant 2, and which email addresses are taken there.
#[tokio::test]
async fn a_client_probes_another_tenants_rows_through_insert() {
    let backing = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&backing)).await;
    let mut client = serving.client().await;

    // Tenant 2 has one user. Tenant 1's client never sees it.
    client
        .insert(app_in(
            insert(vec![wire(&user(2, 7, 999, "secret@two.example"))]),
            999,
            2,
        ))
        .await
        .expect("tenant 2 writes its own row");

    // Tenant 1, probing. Every one of these is refused; what differs is how.
    let occupied = client
        .insert(app_in(
            insert(vec![wire(&user(2, 7, 1, "mine@one.example"))]),
            1,
            1,
        ))
        .await
        .expect_err("writing into tenant 2 must fail");
    let free = client
        .insert(app_in(
            insert(vec![wire(&user(2, 8, 1, "mine@one.example"))]),
            1,
            1,
        ))
        .await
        .expect_err("writing into tenant 2 must fail");
    let email_taken = client
        .insert(app_in(
            insert(vec![wire(&user(2, 8, 1, "secret@two.example"))]),
            1,
            1,
        ))
        .await
        .expect_err("writing into tenant 2 must fail");

    // FIXED. All three are refusals and all three are now the *same* refusal.
    // `write_many` decides the row policy before it reads anything, exactly as
    // single-row `insert` does, so a taken key, a taken email and a free slot
    // in another tenant are indistinguishable from here.
    //
    // Asserted end to end as well as in the kernel because this is the shape
    // that mattered: the probe was free, repeatable, batched, and every insert
    // on the wire went through it.
    for (label, status) in [
        ("key taken in tenant 2", &occupied),
        ("key free in tenant 2", &free),
        ("email taken in tenant 2", &email_taken),
    ] {
        assert_eq!(
            status.code(),
            Code::PermissionDenied,
            "{label} still answers differently: {status:?}"
        );
    }
    assert!(
        !email_taken.message().contains("by_email"),
        "the refusal still names the index another tenant's row occupies: {}",
        email_taken.message()
    );

    // Nothing was written, so the probe is free and repeatable.
    drop(serving);
}

// --- what a duplicated identity header does --------------------------------

/// `MetadataIdentity` is correct "behind a proxy that ... sets the headers
/// itself, and strips any copies the client supplied". This records what
/// happens when the proxy *appends* rather than strips — the difference
/// between `proxy_set_header` and `add_header` in the two commonest proxies —
/// so the requirement is a demonstrated one rather than a stated one.
#[test]
fn a_duplicated_identity_header_is_refused_rather_than_resolved() {
    use slate_server::Authenticator as _;
    use tonic::metadata::MetadataMap;

    let mut metadata = MetadataMap::new();
    // The client's own headers arrive first...
    metadata.append("slate-principal", "u64:666".parse().unwrap());
    metadata.append("slate-roles", "admin".parse().unwrap());
    metadata.append("slate-tenant", "u64:2".parse().unwrap());
    // ...and the proxy appends its own after them.
    metadata.append("slate-principal", "u64:1".parse().unwrap());
    metadata.append("slate-roles", "app".parse().unwrap());
    metadata.append("slate-tenant", "u64:1".parse().unwrap());

    // Was: the client's copy won, because `get` returns the first. Now the
    // request is refused outright. Resolving it either way would be a guess
    // about a proxy this server cannot see: taking the last trusts one that
    // appends, taking the first trusts one that replaces.
    let status = slate_server::MetadataIdentity::trusting_the_caller_completely()
        .authenticate(&metadata)
        .expect_err("a duplicated identity header must not be resolved");

    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(
        status.message().contains("more than once"),
        "the error should name the duplicate rather than the value it rejected: {}",
        status.message()
    );
    assert!(
        !status.message().contains("666"),
        "the refusal must not echo the identity the caller tried to claim: {}",
        status.message()
    );
}

/// FINDING 8: a caller with no grant could confirm a table's shape.
///
/// The fingerprint check used to run before anything authorised the caller, so
/// someone holding a role with no grant on `users` could send a guessed
/// `(name, type, key)` layout and learn from the answer whether the guess was
/// right — one 64-bit fingerprint at a time, against a table they cannot read.
///
/// The fix is ordering: the handlers authorise before they fingerprint-check.
/// Both a right and a wrong guess now answer `PERMISSION_DENIED`, so the
/// answer carries no information about the shape.
///
/// Table *existence* is still disclosed — an unknown name answers `NOT_FOUND`
/// — and that is deliberate. See `authorized_table` for why.
#[tokio::test]
async fn a_caller_with_no_grant_cannot_confirm_a_tables_shape() {
    let backing = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&backing)).await;
    let mut client = serving.client().await;

    // `stranger` is a real role that grants nothing on `users`.
    let stranger = |message: pb::InsertRequest| {
        common::as_principal(message, "u64:9", Some("u64:1"), "stranger")
    };

    let right = insert(vec![wire(&user(1, 1, 1, "a@example.com"))]);
    let mut wrong = right.clone();
    wrong.schema = Some(pb::SchemaCheck {
        columns: 99,
        fingerprint: 0xDEAD_BEEF,
    });

    let with_right_guess = client.insert(stranger(right)).await.expect_err("denied");
    let with_wrong_guess = client.insert(stranger(wrong)).await.expect_err("denied");

    assert_eq!(
        with_right_guess.code(),
        Code::PermissionDenied,
        "a caller with no grant must be refused before the fingerprint runs"
    );
    assert_eq!(
        with_wrong_guess.code(),
        with_right_guess.code(),
        "a right and a wrong schema guess must be indistinguishable: {} vs {}",
        with_right_guess.message(),
        with_wrong_guess.message()
    );
    assert_eq!(
        with_wrong_guess.message(),
        with_right_guess.message(),
        "the message must not vary with the guess either"
    );
}

/// Each handler must authorise the action it actually performs.
///
/// The hazard of checking early is checking *differently*. Testing with the
/// `app` role cannot detect it: `app` holds every action on `users`, so a
/// handler asking for `Explain` where it meant `Delete` passes — which it did,
/// when this test first existed in that form.
///
/// So each path is exercised by a role holding exactly one action. A handler
/// that names any other action refuses a caller who should get through.
#[tokio::test]
async fn each_handler_authorizes_the_action_it_performs() {
    let backing = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&backing)).await;
    let mut client = serving.client().await;

    // A `fn`, not a closure: a closure monomorphises to whatever type it is
    // first called with, and this is called with four different requests.
    fn only<T>(message: T, role: &str) -> tonic::Request<T> {
        common::as_principal(message, "u64:1", Some("u64:1"), role)
    }
    let key = || {
        wire(&slate_schema::Row::new(vec![
            slate_tuple::Value::U64(1),
            slate_tuple::Value::U64(1),
        ]))
    };

    client
        .insert(only(
            insert(vec![wire(&user(1, 1, 1, "a@example.com"))]),
            "inserter_only",
        ))
        .await
        .expect("a role granted only `insert` must be able to insert");

    client
        .get(only(
            pb::GetRequest {
                transaction: String::new(),
                table: "users".to_owned(),
                primary_key: Some(key()),
                freshness: None,
                schema: Some(claim("users")),
            },
            "reader_only",
        ))
        .await
        .expect("a role granted only `read` must be able to get");

    client
        .update(only(
            pb::UpdateRequest {
                transaction: String::new(),
                table: "users".to_owned(),
                rows: vec![wire(&user(1, 1, 1, "b@example.com"))],
                expected: Vec::new(),
                schema: Some(claim("users")),
            },
            "updater_only",
        ))
        .await
        .expect("a role granted only `update` must be able to update");

    client
        .delete(only(
            pb::DeleteRequest {
                transaction: String::new(),
                table: "users".to_owned(),
                primary_keys: vec![key()],
                schema: Some(claim("users")),
                expected: Vec::new(),
            },
            "deleter_only",
        ))
        .await
        .expect("a role granted only `delete` must be able to delete");
}

/// And the ordinary blanket-granted caller still gets through every path.
#[tokio::test]
async fn the_early_authorization_does_not_refuse_a_permitted_caller() {
    let backing = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&backing)).await;
    let mut client = serving.client().await;

    let row = user(1, 1, 1, "a@example.com");
    client
        .insert(app_in(insert(vec![wire(&row)]), 1, 1))
        .await
        .expect("insert is permitted");

    client
        .get(app_in(
            pb::GetRequest {
                transaction: String::new(),
                table: "users".to_owned(),
                primary_key: Some(wire(&slate_schema::Row::new(vec![
                    slate_tuple::Value::U64(1),
                    slate_tuple::Value::U64(1),
                ]))),
                freshness: None,
                schema: Some(claim("users")),
            },
            1,
            1,
        ))
        .await
        .expect("get is permitted");

    let updated = user(1, 1, 1, "b@example.com");
    client
        .update(app_in(
            pb::UpdateRequest {
                transaction: String::new(),
                table: "users".to_owned(),
                rows: vec![wire(&updated)],
                expected: Vec::new(),
                schema: Some(claim("users")),
            },
            1,
            1,
        ))
        .await
        .expect("update is permitted");

    client
        .delete(app_in(
            pb::DeleteRequest {
                transaction: String::new(),
                table: "users".to_owned(),
                primary_keys: vec![wire(&slate_schema::Row::new(vec![
                    slate_tuple::Value::U64(1),
                    slate_tuple::Value::U64(1),
                ]))],
                schema: Some(claim("users")),
                expected: Vec::new(),
            },
            1,
            1,
        ))
        .await
        .expect("delete is permitted");
}

/// Finding 8 again, on the read paths the fix did not cover.
///
/// The fix ordered the *fingerprint* check behind `authorized_table`, closing
/// the channel the finding named. `query` and `explain` resolve their table
/// with the bare `self.table(..)` and then run `query_from_proto`, which turns
/// a `ColumnRef` into a flat ordinal — and, as the proto says, "only the
/// server knows how wide each table is". So a caller with no grant can ask
/// about column *n* and learn from the answer whether the table has one.
///
/// Two requests that differ only in an ordinal, from a role granted nothing on
/// `users`. If the answers differ, the width of a table the caller cannot read
/// is a binary search away.
#[tokio::test]
async fn a_caller_with_no_grant_cannot_probe_a_tables_width() {
    let backing = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&backing)).await;
    let mut client = serving.client().await;

    let stranger = |message: pb::QueryRequest| {
        common::as_principal(message, "u64:9", Some("u64:1"), "stranger")
    };
    let projecting = |ordinal: u32| {
        let mut query = common::plain_query("users");
        query.projection = Some(pb::Projection {
            all_columns: false,
            columns: vec![pb::ColumnRef {
                input: 0,
                of: Some(pb::column_ref::Of::Column(ordinal)),
            }],
        });
        pb::QueryRequest {
            transaction: String::new(),
            query: Some(query),
            freshness: None,
        }
    };

    // Ordinal 0 exists in every table; 99 exists in none of this size.
    let real = client.query(stranger(projecting(0))).await.err();
    let absent = client.query(stranger(projecting(99))).await.err();

    let code = |e: &Option<tonic::Status>| e.as_ref().map(tonic::Status::code);
    let message = |e: &Option<tonic::Status>| {
        e.as_ref()
            .map(|s| s.message().to_owned())
            .unwrap_or_default()
    };
    assert_eq!(
        code(&real),
        code(&absent),
        "a real and an absent column must be indistinguishable to a caller \
         with no grant: {:?} vs {:?}",
        message(&real),
        message(&absent)
    );
    assert_eq!(
        message(&real),
        message(&absent),
        "and the message must not vary with the ordinal either"
    );
    assert_eq!(
        code(&real),
        Some(Code::PermissionDenied),
        "a caller with no grant must be refused before the query is converted"
    );
}

/// The same channel on `explain`, which resolves and converts identically.
///
/// `Action::Explain` rather than `Read`, because that is what the kernel
/// checks first — a handler authorising the wrong action refuses a caller the
/// kernel would allow, which is the hazard `each_handler_authorizes_the_action_it_performs`
/// exists for.
#[tokio::test]
async fn explaining_does_not_leak_a_tables_width_either() {
    let backing = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&backing)).await;
    let mut client = serving.client().await;

    let explaining = |ordinal: u32| {
        let mut query = common::plain_query("users");
        query.projection = Some(pb::Projection {
            all_columns: false,
            columns: vec![pb::ColumnRef {
                input: 0,
                of: Some(pb::column_ref::Of::Column(ordinal)),
            }],
        });
        common::as_principal(
            pb::ExplainRequest {
                transaction: String::new(),
                query: Some(query),
                freshness: None,
            },
            "u64:9",
            Some("u64:1"),
            "stranger",
        )
    };

    let real = client.explain(explaining(0)).await.expect_err("denied");
    let absent = client.explain(explaining(99)).await.expect_err("denied");
    assert_eq!(real.code(), Code::PermissionDenied);
    // And the action named is `explain`, not `read`. The kernel checks
    // `Action::Explain` first and `Read` second, so a handler authorising
    // `Read` early answers a caller holding neither with the wrong one — which
    // is `each_handler_authorizes_the_action_it_performs`'s hazard, "checking
    // early is checking *differently*", on a handler that test does not cover.
    // Without this line, swapping the action here changes nothing observable
    // and the mutation survives.
    assert!(
        real.message().contains("explain"),
        "the refusal should name the action the kernel checks first: {}",
        real.message()
    );
    assert_eq!(
        real.message(),
        absent.message(),
        "a real and an absent column must be indistinguishable: {} vs {}",
        real.message(),
        absent.message()
    );
}

/// And on `load`, where the schema disclosed is the foreign keys rather than
/// the width.
///
/// `resolve_relation` refuses an unknown foreign key by *listing the ones that
/// exist*, and it ran before anything authorised the caller. Two requests
/// differing only in a relation name, from a role granted nothing.
#[tokio::test]
async fn loading_does_not_leak_a_tables_foreign_keys() {
    let backing = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&backing)).await;
    let mut client = serving.client().await;

    let loading = |foreign_key: &str| {
        common::as_principal(
            pb::RelatedRequest {
                transaction: String::new(),
                relation: None,
                keys: vec![pb::Value {
                    kind: Some(pb::value::Kind::Uint64Value(1)),
                }],
                freshness: None,
                schema: None,
                path: vec![pb::RelatedStep {
                    relation: Some(pb::Relation {
                        table: "users".to_owned(),
                        foreign_key: foreign_key.to_owned(),
                        direction: pb::relation::Direction::Children as i32,
                    }),
                    schema: None,
                }],
            },
            "u64:9",
            Some("u64:1"),
            "stranger",
        )
    };

    let one = client
        .related(loading("no_such_key"))
        .await
        .expect_err("denied");
    let other = client
        .related(loading("also_not_a_key"))
        .await
        .expect_err("denied");
    assert_eq!(
        one.code(),
        Code::PermissionDenied,
        "a caller with no grant must be refused before the relation resolves: {}",
        one.message()
    );
    assert_eq!(
        one.message(),
        other.message(),
        "and the refusal must not name the foreign keys that do exist"
    );
}
