//! The ceiling on a predicate write that asks for its rows back.
//!
//! # The defect this exists for
//!
//! `WriteResponse` is one message and `returning` puts every matched row in
//! it. Nothing bounded that, and a default gRPC client refuses to decode a
//! message over 4 MiB — so a large enough predicate write **committed and then
//! failed to deliver**. Reproduced before the fix, at 8,000 rows of about a
//! kilobyte each:
//!
//! ```text
//! ERR: code OutOfRange, decoded message length too large:
//!      found 8423749 bytes, the limit is: 4194304 bytes
//! rows left in the table: 0
//! ```
//!
//! Every row was destroyed and the caller got an error naming a decode limit,
//! which says nothing about the write having happened. That is a known outcome
//! converted into an unknown one, and it is the case the error taxonomy tells
//! callers not to retry — so a caller doing the right thing with that error
//! learns nothing and can do nothing.
//!
//! The fix is a ceiling checked *before the write*, which is the only ordering
//! that leaves the caller's world consistent. `matching_rows` in the kernel is
//! where it lives, because that is the single place both predicate writes
//! decide which rows they touch, and it is upstream of every write.
//!
//! # Why the cap is on the answer and not on the write
//!
//! A `DELETE … WHERE` over a million rows is a legitimate delete, and it is
//! uncapped. Only asking for those million rows *back* is refused. `at_most`
//! is therefore `None` unless the request set `returning`, which is what
//! `Head::returnable` decides.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, doc, docs};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{CmpOp, Expr, Scalar};
use slate_schema::Ordinal;
use slate_server::convert::{Space, column_ref, expr_to_proto, scalar_to_proto};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;

const SIZE: Ordinal = Ordinal(2);

/// A cap low enough to reach in a test. The shipped default is 10,000; sending
/// ten thousand and one rows would pin nothing extra and cost seconds.
const CAP: usize = 3;

/// `rows` documents, ids `0..rows`, size = id.
async fn seeded_with(rows: u64, cap: Option<usize>) -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = docs();
        let seed: Vec<_> = (0..rows)
            .map(|id| doc(id, "kind-0", id as i64, None))
            .collect();
        let txn = store.begin().await.unwrap();
        txn.insert_many(&context, &table, &seed).await.unwrap();
        txn.commit().await.unwrap();
    }
    let leadership = common::leader().await;
    common::serve(common::head_with(
        backing,
        Vec::new(),
        leadership,
        common::config().with_limits(slate_server::Limits {
            max_returned_rows: cap,
            ..slate_server::Limits::default()
        }),
    ))
    .await
}

fn everything() -> pb::Expr {
    expr_to_proto(
        &Space::input(&docs(), 0, 0),
        &Expr::compare(SIZE, CmpOp::Ge, Value::I64(0)),
    )
}

fn delete_where(returning: bool) -> pb::DeleteWhereRequest {
    pb::DeleteWhereRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        filter: Some(everything()),
        returning,
        schema: Some(common::claim("docs")),
    }
}

fn update_where(returning: bool) -> pb::UpdateWhereRequest {
    let bump = Scalar::Add(
        Box::new(Scalar::column(SIZE)),
        Box::new(Scalar::literal(Value::I64(1))),
    );
    pb::UpdateWhereRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        filter: Some(everything()),
        assignments: vec![pb::Assignment {
            column: Some(column_ref(0, SIZE.0)),
            value: Some(scalar_to_proto(&Space::input(&docs(), 0, 0), &bump)),
        }],
        returning,
        schema: Some(common::claim("docs")),
    }
}

async fn how_many_left(client: &mut RecordsClient<Channel>) -> usize {
    let mut stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(common::docs_query()),
            freshness: None,
        }))
        .await
        .expect("a query")
        .into_inner();
    let mut rows = 0;
    while let Some(message) = stream.message().await.expect("a query message") {
        rows += message.rows.len();
    }
    rows
}

/// The property the whole item is about: a refused `RETURNING` deleted nothing.
///
/// The count afterwards is the test. A version that only asserted the status
/// code would pass against a server that refused *after* committing, which is
/// precisely the defect.
#[tokio::test]
async fn a_refused_returning_delete_leaves_every_row() {
    let serving = seeded_with(10, Some(CAP)).await;
    let mut client = serving.client().await;

    let status = client
        .delete_where(app(delete_where(true)))
        .await
        .expect_err("ten rows against a cap of three");

    assert_eq!(status.code(), Code::ResourceExhausted);
    assert!(
        status.message().contains("more than 3 rows"),
        "the refusal should name the cap: {}",
        status.message()
    );
    assert_eq!(
        how_many_left(&mut client).await,
        10,
        "the refusal must land before the write, not after it"
    );
}

#[tokio::test]
async fn a_refused_returning_update_changes_nothing() {
    let serving = seeded_with(10, Some(CAP)).await;
    let mut client = serving.client().await;

    let status = client
        .update_where(app(update_where(true)))
        .await
        .expect_err("ten rows against a cap of three");

    assert_eq!(status.code(), Code::ResourceExhausted);
    // Every row still has `size == id`, which the bump would have broken.
    let mut stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(common::docs_query()),
            freshness: None,
        }))
        .await
        .expect("a query")
        .into_inner();
    let mut seen = 0;
    while let Some(message) = stream.message().await.expect("a query message") {
        for row in &message.rows {
            let id = match row.values[0].kind.as_ref() {
                Some(pb::value::Kind::Uint64Value(id)) => *id as i64,
                other => panic!("the first column was {other:?}"),
            };
            let size = match row.values[2].kind.as_ref() {
                Some(pb::value::Kind::Int64Value(size)) => *size,
                other => panic!("the third column was {other:?}"),
            };
            assert_eq!(size, id, "the refused update must not have applied");
            seen += 1;
        }
    }
    assert_eq!(seen, 10);
}

/// The cap is on the answer, not on the write.
///
/// The same ten rows the test above refuses are deleted here without
/// complaint, because this caller did not ask to see them. Deleting a million
/// expired sessions is a thing people do; reading a million rows back into one
/// message is not.
#[tokio::test]
async fn the_same_write_without_returning_is_allowed() {
    let serving = seeded_with(10, Some(CAP)).await;
    let mut client = serving.client().await;

    let response = client
        .delete_where(app(delete_where(false)))
        .await
        .expect("a delete that did not ask for its rows")
        .into_inner();

    assert_eq!(response.affected, 10);
    assert!(response.rows.is_empty());
    assert_eq!(how_many_left(&mut client).await, 0);
}

/// At the cap exactly, not one under it.
///
/// An off-by-one here refuses a request that fits, which is the kind of
/// mistake a "well under" and a "well over" test both miss.
#[tokio::test]
async fn a_match_exactly_at_the_cap_is_allowed() {
    let serving = seeded_with(CAP as u64, Some(CAP)).await;
    let mut client = serving.client().await;

    let response = client
        .delete_where(app(delete_where(true)))
        .await
        .expect("three rows against a cap of three")
        .into_inner();

    assert_eq!(response.affected, CAP as u64);
    assert_eq!(response.rows.len(), CAP);
}

/// One over the cap is refused — the other half of the boundary.
///
/// Written because a mutation demanded it: `rows.len() >= limit` weakened to
/// `> limit` allows one row too many, and every other test here passes with
/// that in place. The "exactly at the cap" test alone pins one side of a
/// boundary, which is half a boundary.
#[tokio::test]
async fn a_match_one_over_the_cap_is_refused() {
    let serving = seeded_with(CAP as u64 + 1, Some(CAP)).await;
    let mut client = serving.client().await;

    let status = client
        .delete_where(app(delete_where(true)))
        .await
        .expect_err("four rows against a cap of three");

    assert_eq!(status.code(), Code::ResourceExhausted);
    assert_eq!(how_many_left(&mut client).await, CAP + 1);
}

/// `None` means no cap, and it has to be spelled rather than arrived at.
#[tokio::test]
async fn an_unset_cap_refuses_nothing() {
    let serving = seeded_with(10, None).await;
    let mut client = serving.client().await;

    let response = client
        .delete_where(app(delete_where(true)))
        .await
        .expect("no cap, no refusal")
        .into_inner();

    assert_eq!(response.rows.len(), 10);
}

/// Inside a caller's transaction, the refusal lands the same way.
///
/// This path does not go through `Write::apply` — it goes through the session
/// actor — so it is a second place the ceiling has to be passed, and a second
/// place it can be forgotten. The transaction stays usable afterwards, which
/// is what says the operation was refused rather than half-applied.
#[tokio::test]
async fn the_ceiling_holds_inside_a_transaction() {
    let serving = seeded_with(10, Some(CAP)).await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest::default()))
        .await
        .expect("begin")
        .into_inner()
        .transaction;

    let status = client
        .delete_where(app(pb::DeleteWhereRequest {
            transaction: transaction.clone(),
            ..delete_where(true)
        }))
        .await
        .expect_err("ten rows against a cap of three, in a transaction");
    assert_eq!(status.code(), Code::ResourceExhausted);

    // The transaction is still alive and has nothing in it: commit, then
    // count. A write that had applied would show up here.
    client
        .commit(app(pb::CommitRequest { transaction }))
        .await
        .expect("the transaction survived a refused operation");
    assert_eq!(how_many_left(&mut client).await, 10);
}

/// A batch inside a caller's transaction is bounded too.
///
/// Written because a mutation survived: the batch handler has *two* paths —
/// one through `Write::apply` for a batch that commits itself, and one through
/// the session actor for a batch joining an open transaction — and only the
/// first had a test. Setting the second path's ceiling to `None` changed no
/// result until this existed.
///
/// An atomic batch discards the rows it would have returned, so the ceiling
/// here bounds nothing the caller sees. It is enforced anyway, and that is the
/// point: a limit that holds on one path and not the other is a limit whose
/// behaviour depends on how the caller happened to spell the request.
#[tokio::test]
async fn a_batched_write_inside_a_transaction_is_bounded() {
    let serving = seeded_with(10, Some(CAP)).await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest::default()))
        .await
        .expect("begin")
        .into_inner()
        .transaction;

    let status = client
        .batch(app(pb::BatchRequest {
            transaction: transaction.clone(),
            atomicity: pb::Atomicity::AllOrNothing as i32,
            operations: vec![pb::BatchOperation {
                of: Some(pb::batch_operation::Of::DeleteWhere(pb::DeleteWhereRequest {
                    transaction: String::new(),
                    ..delete_where(true)
                })),
            }],
        }))
        .await
        .expect_err("ten rows against a cap of three, batched in a transaction");
    assert_eq!(status.code(), Code::ResourceExhausted);

    client
        .rollback(app(pb::RollbackRequest { transaction }))
        .await
        .expect("rollback");
    assert_eq!(how_many_left(&mut client).await, 10);
}

/// A batched predicate write is bounded too.
///
/// An independent batch reports its failures inside a successful response, so
/// this is the one path where the refusal is a `BatchError` rather than a
/// status — and the rows must still be there.
#[tokio::test]
async fn a_batched_returning_write_is_bounded() {
    let serving = seeded_with(10, Some(CAP)).await;
    let mut client = serving.client().await;

    let response = client
        .batch(app(pb::BatchRequest {
            transaction: String::new(),
            atomicity: pb::Atomicity::Independent as i32,
            operations: vec![pb::BatchOperation {
                of: Some(pb::batch_operation::Of::DeleteWhere(delete_where(true))),
            }],
        }))
        .await
        .expect("the batch itself is fine; the operation is not")
        .into_inner();

    let [result] = &response.results[..] else {
        panic!("one operation, one result: {:?}", response.results)
    };
    let Some(pb::batch_result::Of::Error(error)) = result.of.as_ref() else {
        panic!("the operation should have been refused: {result:?}")
    };
    assert_eq!(error.code, Code::ResourceExhausted as i32);
    assert_eq!(error.reason, "PREDICATE_WRITE_TOO_LARGE");
    assert_eq!(how_many_left(&mut client).await, 10);
}
