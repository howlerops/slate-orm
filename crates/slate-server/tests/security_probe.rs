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
