//! Keyset pagination over the wire.
//!
//! The kernel has had `Query::after` for a while and the wire had no field for
//! it, so every remote caller paged by offset — which is correct only while
//! nothing changes. `OFFSET n` counts rows: delete one ahead of the cursor
//! between two pages and the reader silently skips a row, insert one and they
//! see a row twice, and nothing reports either. That is the property this file
//! is about, and `paging_visits_every_row_exactly_once_under_concurrent_writes`
//! is the test that would fail if the server quietly translated the cursor
//! into an offset.
//!
//! The server also *builds* the cursor, rather than leaving the client to pull
//! the key out of the last row. `Records::page_records` gives the reason —
//! doing it by hand means knowing which columns are the primary key and in what
//! order, and a cursor built from the wrong column still pages, just through
//! the wrong sequence. The cost of that choice is a refusal the caller has to
//! understand: a projection that drops a key column cannot produce a cursor, so
//! a paged read asking for one is refused by name rather than served without.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, doc, doc_ids, docs_query, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_schema::Ordinal;
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use std::collections::BTreeSet;
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;

const ID: Ordinal = Ordinal(0);
const KIND: Ordinal = Ordinal(1);

/// Forty documents, ids 0..40.
async fn seeded() -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = common::docs();
        let rows: Vec<_> = (0..40_u64)
            .map(|id| doc(id, &format!("kind-{}", id % 4), id as i64, None))
            .collect();
        let txn = store.begin().await.unwrap();
        txn.insert_many(&context, &table, &rows).await.unwrap();
        txn.commit().await.unwrap();
    }
    serving_leader(backing).await
}

/// One page: the ids it returned and the cursor to ask for the next.
///
/// `next_cursor` arrives on whichever message carries it and is empty on the
/// rest, so this reads the last non-empty one rather than the last message —
/// a reader that only looked at the final message would work by accident
/// whenever the rows did not divide evenly into batches.
async fn page(client: &mut RecordsClient<Channel>, query: pb::Query) -> (Vec<u64>, Vec<pb::Value>) {
    let mut stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(query),
            freshness: None,
        }))
        .await
        .expect("a paged query")
        .into_inner();
    let mut rows = Vec::new();
    let mut cursor = Vec::new();
    while let Some(message) = stream.message().await.expect("a query message") {
        rows.extend(message.rows);
        if !message.next_cursor.is_empty() {
            cursor = message.next_cursor;
        }
    }
    (doc_ids(&rows), cursor)
}

fn paged(limit: u64, after: Vec<pb::Value>) -> pb::Query {
    pb::Query {
        limit: Some(limit),
        paged: true,
        after,
        ..docs_query()
    }
}

#[tokio::test]
async fn a_full_page_returns_a_cursor_and_a_short_one_does_not() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let (ids, cursor) = page(&mut client, paged(10, Vec::new())).await;
    assert_eq!(ids, (0..10).collect::<Vec<_>>());
    assert!(!cursor.is_empty(), "a full page should carry a cursor");

    // 35 of the 40 remain after the first five pages' worth; ask for more than
    // is left and the page is short.
    let (ids, cursor) = page(&mut client, paged(100, Vec::new())).await;
    assert_eq!(ids.len(), 40);
    assert!(
        cursor.is_empty(),
        "a short page proves there is nothing after it: {cursor:?}"
    );
}

/// The last *full* page still returns a cursor, and the request after it is
/// empty. That extra request is the documented cost of not reading one row
/// ahead on every page — see `Page` in the record layer.
#[tokio::test]
async fn the_last_full_page_costs_one_empty_request() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    // Exactly 40 rows in four pages of ten.
    let mut cursor = Vec::new();
    for at in 0..4_usize {
        let (ids, next) = page(&mut client, paged(10, cursor)).await;
        assert_eq!(ids.len(), 10, "page {at}");
        assert!(!next.is_empty(), "page {at} was full and should say so");
        cursor = next;
    }
    let (ids, next) = page(&mut client, paged(10, cursor)).await;
    assert!(ids.is_empty(), "the fifth page should be empty: {ids:?}");
    assert!(next.is_empty(), "and should end the sequence");
}

/// The property `OFFSET` cannot offer.
///
/// Between every pair of pages, a row *behind* the cursor is deleted and a new
/// one is inserted behind it too. Under `OFFSET` both shift the window: the
/// delete makes the reader skip a row it has not seen, the insert makes it see
/// one twice. A key does not move when its neighbours change, so the sequence
/// of ids visited must be exactly the ids that were there when paging started
/// and are still ahead of the cursor — with no repeats.
#[tokio::test]
async fn paging_visits_every_row_exactly_once_under_concurrent_writes() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let mut seen: Vec<u64> = Vec::new();
    let mut cursor = Vec::new();
    let mut deleted = 0_usize;
    // Bounded, because a bug in the cursor is exactly a loop that never ends:
    // a server that returned one for a short page would page forever and this
    // test would hang rather than fail. It did — a mutation doing that hung a
    // mutation run for thirty minutes before the harness gave up, and a test
    // that hangs on a bug reports nothing. Forty rows in pages of seven is six
    // requests; twenty is room for a different fixture and not for a loop.
    for round in 0..20 {
        assert!(round < 19, "paging did not terminate: {seen:?}");
        let (ids, next) = page(&mut client, paged(7, cursor.clone())).await;
        seen.extend(&ids);
        if next.is_empty() {
            break;
        }
        cursor = next;

        // Churn *behind* the cursor, where offset-based paging would be wrong
        // and keyset paging must be unaffected. A *different* row each round,
        // all of them already in `seen`, so anything re-read shows up as a
        // duplicate below — and deleting the same row twice would refuse
        // rather than churn.
        let behind = *seen.get(deleted).expect("a row behind the cursor");
        client
            .delete(app(pb::DeleteRequest {
                transaction: String::new(),
                table: "docs".to_owned(),
                primary_keys: vec![pb::Row {
                    values: vec![pb::Value {
                        kind: Some(pb::value::Kind::Uint64Value(behind)),
                    }],
                    computed: Vec::new(),
                }],
                schema: Some(common::claim("docs")),
                expected: Vec::new(),
            }))
            .await
            .expect("deleting behind the cursor");
        deleted += 1;
    }
    assert!(
        deleted > 0,
        "the churn never happened, so this asserts nothing"
    );

    let unique: BTreeSet<u64> = seen.iter().copied().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "a row was visited twice: {seen:?}"
    );
    assert_eq!(
        seen,
        (0..40).collect::<Vec<_>>(),
        "paging should visit every row that was ahead of the cursor, in key order"
    );
}

/// A cursor from a *different* table's key shape is refused rather than
/// silently paging from nowhere. The kernel's refusal, reaching the wire.
#[tokio::test]
async fn a_cursor_that_is_not_a_whole_key_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let two = vec![
        pb::Value {
            kind: Some(pb::value::Kind::Uint64Value(1)),
        },
        pb::Value {
            kind: Some(pb::value::Kind::Uint64Value(2)),
        },
    ];
    let status = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(paged(5, two)),
            freshness: None,
        }))
        .await
        .expect_err("a two-column cursor for a one-column key should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("primary key"),
        "the refusal should say what a cursor is: {}",
        status.message()
    );
}

#[tokio::test]
async fn a_paged_read_without_a_limit_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let status = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                paged: true,
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .expect_err("a page with no size should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("limit"),
        "the refusal should name the missing field: {}",
        status.message()
    );
}

/// The refusal that pays for the server building the cursor.
///
/// Serving this without a cursor would be the silent version: the caller loops
/// until the cursor is empty, gets an empty one on the first page, and reads
/// ten rows of a forty-row table as the whole answer.
#[tokio::test]
async fn a_projection_that_drops_the_key_is_refused_when_paged() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let without_id = pb::Projection {
        all_columns: false,
        columns: vec![slate_server::convert::column_ref(0, KIND.0)],
    };
    let status = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                projection: Some(without_id.clone()),
                ..paged(10, Vec::new())
            }),
            freshness: None,
        }))
        .await
        .expect_err("a paged read cannot return a cursor it has no key for");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("id"),
        "the refusal should name the missing key column: {}",
        status.message()
    );

    // And the same projection is fine when the caller is not paging, because
    // then nothing needs a cursor.
    let mut stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                projection: Some(without_id),
                limit: Some(10),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .expect("an unpaged projection is unaffected")
        .into_inner();
    let mut rows = 0;
    while let Some(message) = stream.message().await.expect("a query message") {
        rows += message.rows.len();
    }
    assert_eq!(rows, 10);
}

/// The *first* page of a read that cannot be paged is refused, not the second.
///
/// Without this the kernel's cursor refusals only fire once a cursor is being
/// carried, so page one of a sorted read comes back happily — with a cursor —
/// and page two is where it falls over. The caller then learns on the second
/// request that the first was never resumable, which is the worst moment to
/// find out. `Query.paged` moves it to the first request, and that is the
/// whole reason the field exists rather than being inferred from `limit`.
///
/// The conformance runner found this: a case listed as a refusal was answered
/// by all three SDKs, because the server had no reason to refuse it yet.
#[tokio::test]
async fn the_first_page_of_a_sorted_read_is_refused_not_the_second() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let sorted = |cursor: Vec<pb::Value>| pb::Query {
        sort: vec![pb::SortKey {
            column: Some(slate_server::convert::column_ref(0, KIND.0)),
            direction: pb::SortDirection::Desc as i32,
            nulls: pb::NullsOrder::Last as i32,
        }],
        ..paged(5, cursor)
    };

    let status = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(sorted(Vec::new())),
            freshness: None,
        }))
        .await
        .expect_err("the first page of a sorted read cannot be resumed and should say so");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("sorted"),
        "the refusal should name the reason: {}",
        status.message()
    );

    // And the same sort is fine when nothing is paging, because then no
    // cursor is promised and no page boundary has to be anywhere.
    let (ids, cursor) = page(
        &mut client,
        pb::Query {
            sort: sorted(Vec::new()).sort,
            limit: Some(5),
            ..docs_query()
        },
    )
    .await;
    assert_eq!(ids.len(), 5);
    assert!(cursor.is_empty());
}

/// A cursor on a join *input* is refused rather than dropped.
///
/// A join input's `Query` carries a cursor field it has no meaning for — the
/// cursor names a row of the result, and an input's rows are not the result.
/// Ignoring it is how a request comes to mean something other than what was
/// written, which is the rule `refuse_unused` already applies to a side's
/// sort, limit and offset.
#[tokio::test]
async fn a_cursor_on_a_join_input_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let cursor = vec![pb::Value {
        kind: Some(pb::value::Kind::Uint64Value(1)),
    }];
    let status = client
        .join(app(pb::JoinRequest {
            transaction: String::new(),
            join: Some(pb::JoinQuery {
                after: Vec::new(),
                paged: false,
                inputs: vec![
                    pb::JoinInput {
                        query: Some(pb::Query {
                            after: cursor,
                            ..docs_query()
                        }),
                        on: Vec::new(),
                        join_type: pb::JoinType::Inner as i32,
                        having: None,
                        force: None,
                    },
                    pb::JoinInput {
                        query: Some(docs_query()),
                        on: Vec::new(),
                        join_type: pb::JoinType::Inner as i32,
                        having: None,
                        force: None,
                    },
                ],
                limit: None,
                offset: 0,
                build_limit: None,
                compute: Vec::new(),
            }),
            freshness: None,
        }))
        .await
        .expect_err("a cursor on a join side should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("cursor"),
        "the refusal should say what was wrong: {}",
        status.message()
    );
}

/// Nothing changes for a caller that does not set `paged`.
///
/// The field is new, and the whole protocol argument for it is that a request
/// without one is served exactly as before.
#[tokio::test]
async fn an_unpaged_limited_read_carries_no_cursor() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let (ids, cursor) = page(
        &mut client,
        pb::Query {
            limit: Some(10),
            ..docs_query()
        },
    )
    .await;
    assert_eq!(ids, (0..10).collect::<Vec<_>>());
    assert!(cursor.is_empty(), "no cursor was asked for: {cursor:?}");

    // And `after` alone still pages, for a caller that keeps its own key.
    let from_twenty = vec![pb::Value {
        kind: Some(pb::value::Kind::Uint64Value(19)),
    }];
    let (ids, cursor) = page(
        &mut client,
        pb::Query {
            limit: Some(3),
            after: from_twenty,
            ..docs_query()
        },
    )
    .await;
    assert_eq!(ids, vec![20, 21, 22]);
    assert!(cursor.is_empty());
}

/// `ID` and `KIND` are used above; this keeps the unused-constant warning
/// honest rather than silenced.
#[test]
fn the_fixture_ordinals_are_what_the_table_says() {
    let table = common::docs();
    assert_eq!(common::at(&table, "id"), ID);
    assert_eq!(common::at(&table, "kind"), KIND);
}
