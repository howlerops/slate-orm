//! Purging soft-deleted rows over the wire.
//!
//! These exist because the conformance runner **cannot** catch what they
//! catch, and that is worth stating plainly rather than discovering twice.
//!
//! That runner's whole design is "the three SDKs agree". A mutation on the
//! *server* changes every client's answer identically, so all three still
//! agree and the run is green. Three mutations of this RPC — reporting a count
//! it did not do, ignoring the ceiling, and authorising `Read` where `Delete`
//! was meant — survived a full 96-case conformance run with nothing to say.
//!
//! A client-side difference is what that suite is for. A server-side one needs
//! a test that knows the expected answer, which is this file.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, as_principal, retire, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_server::convert::column_ref;
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;
use tonic::{Request, Status};

/// Four rows, of which ids 1 and 2 are retired and 3 and 4 are live.
///
/// Retired by *deleting* them, because that is the only path that stamps the
/// column: a row written with `deleted_at` already set is refused, since the
/// write path checks the writer could read back what it wrote.
async fn seeded() -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = retire();
        let rows: Vec<_> = (1..=4_u64)
            .map(|id| {
                slate_schema::Row::new(vec![
                    Value::U64(id),
                    Value::Str(format!("kind-{id}")),
                    Value::Null,
                ])
            })
            .collect();
        let txn = store.begin().await.unwrap();
        txn.insert_many(&context, &table, &rows).await.unwrap();
        txn.commit().await.unwrap();

        let txn = store.begin().await.unwrap();
        for id in [1_u64, 2] {
            txn.delete(&context, &table, &[Value::U64(id)])
                .await
                .unwrap();
        }
        txn.commit().await.unwrap();
    }
    serving_leader(backing).await
}

/// A purge of everything retired before the far future.
fn purge(at_most: u64) -> pb::PurgeDeletedRequest {
    pb::PurgeDeletedRequest {
        transaction: String::new(),
        table: "retire".to_owned(),
        // Far enough ahead that every row retired by `seeded` is included, and
        // not `i64::MAX`, so a bound that was being ignored rather than
        // compared would not be hidden by an extreme value.
        before: 4_102_444_800,
        at_most,
        schema: None,
    }
}

/// Every id still stored, retired ones included.
async fn remaining(client: &mut RecordsClient<Channel>) -> Vec<u64> {
    let mut query = common::plain_query("retire");
    query.schema = None;
    query.include_deleted = true;
    let mut stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(query),
            freshness: None,
        }))
        .await
        .expect("query")
        .into_inner();
    let mut ids = Vec::new();
    while let Some(page) = stream.message().await.expect("page") {
        for row in page.rows {
            match row.values.first().and_then(|v| v.kind.as_ref()) {
                Some(pb::value::Kind::Uint64Value(id)) => ids.push(*id),
                other => panic!("id is {other:?}"),
            }
        }
    }
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn a_purge_erases_the_retired_rows_and_reports_how_many() {
    // The count is the assertion the conformance suite could not make: it
    // compares adapters to each other and holds no expected value, so a server
    // answering `0` for every purge satisfies it perfectly.
    let serving = seeded().await;
    let mut client = serving.client().await;

    let written = client
        .purge_deleted(app(purge(0)))
        .await
        .expect("purge")
        .into_inner();
    assert_eq!(written.affected, 2, "ids 1 and 2 were retired");
    assert!(written.rows.is_empty(), "a purge answers with a count");
    assert!(
        written.sequence.is_some(),
        "a standalone write commits and so has a sequence"
    );

    assert_eq!(remaining(&mut client).await, vec![3, 4], "the live rows");
}

#[tokio::test]
async fn a_second_purge_finds_nothing_and_says_so() {
    // Idempotent, and the zero is a real answer rather than an error. A cron
    // that ran twice should not page anybody.
    let serving = seeded().await;
    let mut client = serving.client().await;
    client.purge_deleted(app(purge(0))).await.expect("first");
    let again = client
        .purge_deleted(app(purge(0)))
        .await
        .expect("second")
        .into_inner();
    assert_eq!(again.affected, 0);
    assert_eq!(remaining(&mut client).await, vec![3, 4]);
}

#[tokio::test]
async fn the_ceiling_refuses_before_anything_is_erased() {
    // Nothing else in the suite passes a non-zero ceiling, so a handler that
    // dropped it entirely went unnoticed. The rows surviving is the half that
    // matters: a refusal after erasing half the table would be worse than no
    // ceiling at all.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let refused = client
        .purge_deleted(app(purge(1)))
        .await
        .expect_err("two retired rows exceed a ceiling of one");
    assert_eq!(refused.code(), Code::ResourceExhausted, "{refused:?}");
    assert_eq!(
        remaining(&mut client).await,
        vec![1, 2, 3, 4],
        "nothing was erased"
    );
}

#[tokio::test]
async fn a_ceiling_the_match_fits_under_is_no_obstacle() {
    // The other side of the boundary, so "the ceiling always refuses" is not
    // what the test above is actually pinning.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let written = client
        .purge_deleted(app(purge(2)))
        .await
        .expect("two rows, a ceiling of two")
        .into_inner();
    assert_eq!(written.affected, 2);
}

/// Holds every data action — including `delete` — and not `read_deleted`.
fn purger_blind<T>(message: T) -> Request<T> {
    as_principal(message, "u64:7", None, "purger_blind")
}

/// Holds `read` and `read_deleted`, and not `delete`.
fn watcher<T>(message: T) -> Request<T> {
    as_principal(message, "u64:8", None, "watcher")
}

#[tokio::test]
async fn a_caller_who_cannot_see_retired_rows_cannot_erase_them() {
    // `Action::ALL` is the four data actions and excludes `read_deleted`, so
    // this role can delete rows it can see and not ones the convention hid.
    // Erasing a retired row means reading it first, and this is what says the
    // kernel's second check is reached through the wire.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let refused = client
        .purge_deleted(purger_blind(purge(0)))
        .await
        .expect_err("delete without read_deleted is not enough");
    assert_eq!(refused.code(), Code::PermissionDenied, "{refused:?}");
    assert!(
        refused.message().contains("read_deleted"),
        "the refusal should name the missing action: {refused:?}"
    );
    assert_eq!(remaining(&mut client).await, vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn a_caller_who_may_see_retired_rows_still_cannot_erase_them() {
    // The complement, and the one that catches a handler authorising `Read`
    // where `Delete` was meant — a mutation that survives every other test
    // here, because every other identity holds both.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let refused = client
        .purge_deleted(watcher(purge(0)))
        .await
        .expect_err("seeing a retired row is not permission to erase it");
    assert_eq!(refused.code(), Code::PermissionDenied, "{refused:?}");
    assert_eq!(remaining(&mut client).await, vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn a_purge_of_a_table_that_does_not_soft_delete_is_refused() {
    // Not a no-op: `docs` has no retired rows by construction, so this is a
    // request aimed at the wrong table and a nightly sweep would never learn
    // that from a zero.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let refused: Status = client
        .purge_deleted(app(pb::PurgeDeletedRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            before: 4_102_444_800,
            at_most: 0,
            schema: None,
        }))
        .await
        .expect_err("docs does not soft-delete");
    assert_eq!(refused.code(), Code::FailedPrecondition, "{refused:?}");
}

// --- a join input asks for retired rows -------------------------------------

/// Holds `read` on `retire` and nothing else — not `read_deleted`.
fn plain_reader<T>(message: T) -> Request<T> {
    as_principal(message, "u64:9", None, "plain_reader")
}

/// `retire` joined to itself on `kind`, with each side's visibility chosen.
///
/// A self-join, because the fixture has one soft-deleting table and what is
/// under test is per-*input* behaviour rather than anything about two tables.
/// Every row's `kind` is unique, so each visible row pairs with itself and the
/// row count is exactly "rows both sides can see".
fn self_join(left_sees_deleted: bool, right_sees_deleted: bool) -> pb::JoinQuery {
    let table = retire();
    let kind = common::at(&table, "kind").0;
    let side = |include: bool| {
        let mut query = common::plain_query("retire");
        query.schema = None;
        query.include_deleted = include;
        Some(query)
    };
    pb::JoinQuery {
        inputs: vec![
            pb::JoinInput {
                query: side(left_sees_deleted),
                on: Vec::new(),
                join_type: pb::JoinType::Inner as i32,
                having: None,
                force: None,
            },
            pb::JoinInput {
                query: side(right_sees_deleted),
                on: vec![pb::JoinOn {
                    earlier: Some(column_ref(0, kind)),
                    own: Some(column_ref(1, kind)),
                }],
                join_type: pb::JoinType::Inner as i32,
                having: None,
                force: None,
            },
        ],
        limit: None,
        offset: 0,
        build_limit: None,
        after: Vec::new(),
        paged: false,
        compute: Vec::new(),
    }
}

async fn joined_rows(client: &mut RecordsClient<Channel>, join: pb::JoinQuery) -> usize {
    let mut stream = client
        .join(app(pb::JoinRequest {
            transaction: String::new(),
            join: Some(join),
            freshness: None,
        }))
        .await
        .expect("join")
        .into_inner();
    let mut rows = 0;
    while let Some(page) = stream.message().await.expect("page") {
        rows += page.rows.len();
    }
    rows
}

#[tokio::test]
async fn a_join_input_can_ask_for_retired_rows_on_its_own() {
    // `include_deleted` sits on the *shared* part of the query builders
    // precisely so a join input can set it, and nothing checked the flag
    // survived the trip: an input is converted by a different function from a
    // plain read, and that function refuses several fields it considers
    // meaningless on one. It could have joined them.
    //
    // Four rows, two retired, `kind` unique per row. A self-join on `kind`
    // pairs each row with itself, so the count is the number of rows *both*
    // sides admit — which makes the flag's effect a number rather than a shape.
    let serving = seeded().await;
    let mut client = serving.client().await;

    assert_eq!(
        joined_rows(&mut client, self_join(false, false)).await,
        2,
        "neither side sees a retired row"
    );
    assert_eq!(
        joined_rows(&mut client, self_join(true, true)).await,
        4,
        "both sides do, so the two retired rows pair up as well"
    );
    // The asymmetric cases are the ones that say the flag is honoured *per
    // input* rather than once for the whole join: a retired row on one side
    // has nothing to match on the other.
    assert_eq!(
        joined_rows(&mut client, self_join(true, false)).await,
        2,
        "input 0 sees four and input 1 sees two, so two pair"
    );
    assert_eq!(
        joined_rows(&mut client, self_join(false, true)).await,
        2,
        "and the other way round"
    );
}

#[tokio::test]
async fn a_join_input_asking_for_retired_rows_needs_the_grant_too() {
    // The check lives in `plan`, which every input goes through — so a caller
    // without `read_deleted` is refused for input 1 exactly as for a plain
    // read. Asserted because "it is in the shared function" is a claim about
    // code, and this is the observation of it.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let refused = client
        .join(plain_reader(pb::JoinRequest {
            transaction: String::new(),
            join: Some(self_join(false, true)),
            freshness: None,
        }))
        .await
        .expect_err("reading retired rows on an input still needs the grant");
    assert_eq!(refused.code(), Code::PermissionDenied, "{refused:?}");
    assert!(
        refused.message().contains("read_deleted"),
        "should name the action: {refused:?}"
    );
}
