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

    // All three are refusals, and all three should be the *same* refusal.
    assert_eq!(
        occupied.code(),
        Code::AlreadyExists,
        "key taken in tenant 2: {occupied:?}"
    );
    assert_eq!(
        free.code(),
        Code::PermissionDenied,
        "key free in tenant 2: {free:?}"
    );
    assert_eq!(
        email_taken.code(),
        Code::AlreadyExists,
        "email taken in tenant 2: {email_taken:?}"
    );
    assert_ne!(
        occupied.code(),
        free.code(),
        "the answers are distinguishable, which is the oracle"
    );
    assert!(
        email_taken.message().contains("by_email"),
        "the refusal even names the index: {}",
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
fn the_first_copy_of_a_duplicated_identity_header_wins() {
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

    let context = slate_server::MetadataIdentity::trusting_the_caller_completely()
        .authenticate(&metadata)
        .expect("authenticated");

    assert_eq!(
        context.principal().id,
        slate_tuple::Value::U64(666),
        "the client's principal won over the proxy's"
    );
    assert_eq!(
        context.principal().tenant,
        Some(slate_tuple::Value::U64(2)),
        "the client's tenant won over the proxy's"
    );
    assert!(
        context.principal().roles.contains("admin"),
        "the client's roles won over the proxy's: {:?}",
        context.principal().roles
    );
    assert!(
        !context.principal().roles.contains("app"),
        "only the first copy is read at all"
    );
}
