//! The shipped fixture, run.
//!
//! `fixtures/python-client.toml` claims to be `clients/python/testserver`'s
//! schema and security as configuration. A claim like that is worth exactly
//! what it is tested to: this file starts the real binary on the real fixture
//! and checks the handful of properties the Python suite depends on, so that
//! "the testserver is replaceable" is a thing the build says rather than a
//! thing this crate asserts about itself.
//!
//! What is *not* checked here is the oracle. `testserver` also runs a fixed
//! list of reads in process against `slate-kernel` and dumps the answers, and
//! that half stays where it is — it is a test instrument, not a server
//! feature. See the report at the top of `fixtures/python-client.toml`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_update,
    unreachable_pub
)]

mod harness;

use harness::{Identity, Serving, connect, proto, query, rows};

/// The identity `clients/python/tests/conftest.py` calls `APP`.
const APP: Identity = Identity::app("u64:1", "u64:1");

fn fixture(name: &str) -> String {
    // `CARGO_MANIFEST_DIR` rather than a relative path: `cargo test` runs with
    // the workspace root as the working directory for some invocations and the
    // crate root for others, and a fixture that loads only under one of them
    // is a test that fails in CI and passes locally.
    format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn serving() -> Serving {
    Serving::start(&[
        "--config",
        &fixture("python-client.toml"),
        "--seed",
        &fixture("python-client-seed.toml"),
    ])
}

/// One column of every returned row, as a string.
fn column(returned: &[proto::Row], ordinal: usize) -> Vec<String> {
    returned
        .iter()
        .map(
            |row| match row.values.get(ordinal).and_then(|v| v.kind.as_ref()) {
                Some(proto::value::Kind::StringValue(text)) => text.clone(),
                Some(proto::value::Kind::Uint64Value(n)) => n.to_string(),
                Some(proto::value::Kind::Int64Value(n)) => n.to_string(),
                other => format!("{other:?}"),
            },
        )
        .collect()
}

#[tokio::test]
async fn the_shipped_fixture_starts_and_serves_every_table() {
    let serving = serving();
    let mut client = connect(&serving).await;

    // Seven docs, all visible: `docs` has no tenant column and no policy.
    let docs = rows(&mut client, &APP, query("docs")).await.expect("docs");
    assert_eq!(docs.len(), 7);
    assert_eq!(column(&docs, 1)[0], "kind-a");

    // `authors`: tenant 1 only, and the caller owns three of the four.
    let authors = rows(&mut client, &APP, query("authors"))
        .await
        .expect("authors");
    assert_eq!(column(&authors, 3), vec!["ada", "bo", "di"]);

    // `books`: the `modern_books` policy hides `a-two`, which is from 1990.
    let books = rows(&mut client, &APP, query("books"))
        .await
        .expect("books");
    let titles = column(&books, 3);
    assert!(titles.contains(&"a-one".to_owned()), "{titles:?}");
    assert!(!titles.contains(&"a-two".to_owned()), "{titles:?}");
    assert!(
        !titles.contains(&"zz-book".to_owned()),
        "another tenant's: {titles:?}"
    );

    // `sales`: `real_sales` hides the zero-unit row, and the tenant hides one
    // more, leaving three of the five.
    let sales = rows(&mut client, &APP, query("sales"))
        .await
        .expect("sales");
    assert_eq!(sales.len(), 3);

    // `users`: the caller's own row in the caller's own tenant, and no other.
    let users = rows(&mut client, &APP, query("users"))
        .await
        .expect("users");
    assert_eq!(column(&users, 3), vec!["mine@example.com"]);
}

#[tokio::test]
async fn the_ungranted_table_is_denied_rather_than_empty() {
    // The distinction `testserver` says it declares `secrets` for: a policy
    // returning no rows and a role check refusing outright must not look the
    // same to a client, or an error-mapping test proves nothing.
    let serving = serving();
    let mut client = connect(&serving).await;

    let status = rows(&mut client, &APP, query("secrets"))
        .await
        .expect_err("`secrets` has no grant");
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status}");
}

#[tokio::test]
async fn the_unique_index_in_the_fixture_refuses_a_duplicate() {
    let serving = serving();
    let mut client = connect(&serving).await;

    let transaction = client
        .begin(APP.on(proto::BeginRequest {}))
        .await
        .expect("begin")
        .into_inner()
        .transaction;

    let refused = client
        .insert(APP.on(proto::InsertRequest {
            transaction: transaction.clone(),
            table: "users".to_owned(),
            rows: vec![harness::row(vec![
                harness::u64_value(1),
                harness::u64_value(99),
                harness::u64_value(1),
                harness::str_value("mine@example.com"),
            ])],
            ..Default::default()
        }))
        .await;

    let status = refused.expect_err("`by_email` is unique");
    assert!(
        status.message().contains("by_email") || status.message().contains("unique"),
        "the refusal should name the constraint: {status}"
    );
}
