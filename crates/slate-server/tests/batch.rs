//! Several writes, one round trip — and the distinction the RPC exists to make.
//!
//! A batch is a network optimisation. A transaction is an atomicity guarantee.
//! Conflating them is how people end up believing they have one when they have
//! the other, and the belief is only tested on the day a write in the middle
//! fails. So `atomicity` is required, `ATOMICITY_UNSPECIFIED` is refused by
//! name, and `an_unspecified_atomicity_is_refused` is the test that keeps a
//! zeroed request from silently meaning either.
//!
//! The measurement this file makes is in `a_batch_is_one_round_trip`, and it is
//! deliberately weak as an assertion: round trips are counted, which is exact,
//! and wall clock is reported rather than asserted, because a threshold on a
//! shared runner is a flake waiting for a slow morning.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, doc, docs, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{CmpOp, Expr, Scalar};
use slate_schema::Ordinal;
use slate_server::convert::{Space, column_ref, expr_to_proto, scalar_to_proto};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::sync::Arc;
use std::time::Instant;
use tonic::Code;
use tonic::transport::Channel;

const KIND: Ordinal = Ordinal(1);
const SIZE: Ordinal = Ordinal(2);

async fn serving() -> common::Serving {
    serving_leader(Arc::new(MemoryStore::new())).await
}

fn insert(id: u64) -> pb::BatchOperation {
    pb::BatchOperation {
        of: Some(pb::batch_operation::Of::Insert(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![slate_server::convert::row_to_proto(&doc(
                id, "batched", id as i64, None,
            ))],
            upsert: false,
            schema: Some(common::claim("docs")),
        })),
    }
}

fn batch(operations: Vec<pb::BatchOperation>, atomicity: pb::Atomicity) -> pb::BatchRequest {
    pb::BatchRequest {
        operations,
        atomicity: atomicity as i32,
        transaction: String::new(),
    }
}

/// Every id currently in `docs`.
async fn ids(client: &mut RecordsClient<Channel>) -> Vec<u64> {
    let mut stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(common::docs_query()),
            freshness: None,
        }))
        .await
        .expect("a query")
        .into_inner();
    let mut rows = Vec::new();
    while let Some(message) = stream.message().await.expect("a message") {
        rows.extend(message.rows);
    }
    common::doc_ids(&rows)
}

#[tokio::test]
async fn an_unspecified_atomicity_is_refused() {
    let serving = serving().await;
    let mut client = serving.client().await;

    let status = client
        .batch(app(pb::BatchRequest {
            operations: vec![insert(1)],
            atomicity: pb::Atomicity::Unspecified as i32,
            transaction: String::new(),
        }))
        .await
        .expect_err("a batch with no atomicity should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    // The message must name both options, because the caller's next move is to
    // pick one and the difference is the whole point.
    assert!(
        status.message().contains("INDEPENDENT"),
        "{}",
        status.message()
    );
    assert!(
        status.message().contains("ALL_OR_NOTHING"),
        "{}",
        status.message()
    );

    // And nothing was written on the way to being refused.
    assert!(ids(&mut client).await.is_empty());
}

#[tokio::test]
async fn an_empty_batch_is_refused() {
    let serving = serving().await;
    let mut client = serving.client().await;
    let status = client
        .batch(app(batch(Vec::new(), pb::Atomicity::Independent)))
        .await
        .expect_err("an empty batch should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn an_independent_batch_applies_every_operation() {
    let serving = serving().await;
    let mut client = serving.client().await;

    let response = client
        .batch(app(batch(
            (1..=5).map(insert).collect(),
            pb::Atomicity::Independent,
        )))
        .await
        .expect("a batch")
        .into_inner();

    assert_eq!(response.results.len(), 5, "one result per operation");
    for result in &response.results {
        match result.of.as_ref() {
            Some(pb::batch_result::Of::Ok(write)) => assert_eq!(write.affected, 1),
            other => panic!("expected an ok, got {other:?}"),
        }
    }
    assert!(response.sequence.is_some(), "the last commit's sequence");
    assert_eq!(ids(&mut client).await, vec![1, 2, 3, 4, 5]);
}

/// The difference, demonstrated: one failure, and the others still landed.
#[tokio::test]
async fn an_independent_batch_reports_a_failure_and_carries_on() {
    let serving = serving().await;
    let mut client = serving.client().await;
    // 3 is already there, so inserting it again is a duplicate key.
    client
        .batch(app(batch(vec![insert(3)], pb::Atomicity::Independent)))
        .await
        .expect("the setup insert");

    let response = client
        .batch(app(batch(
            vec![insert(1), insert(3), insert(5)],
            pb::Atomicity::Independent,
        )))
        .await
        .expect("the batch itself succeeds; the operation inside it did not")
        .into_inner();

    assert_eq!(response.results.len(), 3);
    assert!(matches!(
        response.results[0].of,
        Some(pb::batch_result::Of::Ok(_))
    ));
    match response.results[1].of.as_ref() {
        Some(pb::batch_result::Of::Error(error)) => {
            assert_eq!(error.code, Code::AlreadyExists as i32);
            // The stable token survives the trip through a message body, which
            // is the half of this that a lone RPC gets from its trailers.
            assert_eq!(error.reason, "DUPLICATE_PRIMARY_KEY", "{error:?}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(matches!(
        response.results[2].of,
        Some(pb::batch_result::Of::Ok(_))
    ));

    // 1 and 5 landed although 3 failed between them. That is independence, and
    // it is the thing a caller who wanted a transaction would be horrified by.
    assert_eq!(ids(&mut client).await, vec![1, 3, 5]);
}

/// And the other guarantee: one failure, nothing lands.
#[tokio::test]
async fn an_atomic_batch_rolls_back_the_operations_before_the_failure() {
    let serving = serving().await;
    let mut client = serving.client().await;
    client
        .batch(app(batch(vec![insert(3)], pb::Atomicity::Independent)))
        .await
        .expect("the setup insert");

    let status = client
        .batch(app(batch(
            vec![insert(1), insert(3), insert(5)],
            pb::Atomicity::AllOrNothing,
        )))
        .await
        .expect_err("the whole request fails");
    assert_eq!(status.code(), Code::AlreadyExists);

    // Only the setup row. 1 was applied before 3 failed and was rolled back
    // with it; 5 never ran.
    assert_eq!(ids(&mut client).await, vec![3]);
}

#[tokio::test]
async fn an_atomic_batch_reports_no_per_operation_results() {
    let serving = serving().await;
    let mut client = serving.client().await;

    let response = client
        .batch(app(batch(
            (1..=3).map(insert).collect(),
            pb::Atomicity::AllOrNothing,
        )))
        .await
        .expect("a batch")
        .into_inner();

    // Nothing to report per operation: they all happened. A list of successes
    // would invite a caller to check it, and it could only ever say "yes".
    assert!(response.results.is_empty(), "{:?}", response.results);
    assert!(response.sequence.is_some(), "one commit, one sequence");
    assert_eq!(ids(&mut client).await, vec![1, 2, 3]);
}

/// Mixed operations, including the predicate writes and their rows.
#[tokio::test]
async fn a_batch_carries_every_kind_of_write() {
    let serving = serving().await;
    let mut client = serving.client().await;
    client
        .batch(app(batch(
            (1..=6).map(insert).collect(),
            pb::Atomicity::Independent,
        )))
        .await
        .expect("the seed");

    let table = docs();
    let space = Space::input(&table, 0, 0);
    let response = client
        .batch(app(batch(
            vec![
                insert(7),
                pb::BatchOperation {
                    of: Some(pb::batch_operation::Of::DeleteWhere(
                        pb::DeleteWhereRequest {
                            transaction: String::new(),
                            table: "docs".to_owned(),
                            filter: Some(expr_to_proto(
                                &space,
                                &Expr::compare(SIZE, CmpOp::Ge, Value::I64(6)),
                            )),
                            returning: true,
                            schema: Some(common::claim("docs")),
                        },
                    )),
                },
                pb::BatchOperation {
                    of: Some(pb::batch_operation::Of::UpdateWhere(
                        pb::UpdateWhereRequest {
                            transaction: String::new(),
                            table: "docs".to_owned(),
                            filter: Some(expr_to_proto(
                                &space,
                                &Expr::eq(KIND, Value::Str("batched".to_owned())),
                            )),
                            assignments: vec![pb::Assignment {
                                column: Some(column_ref(0, SIZE.0)),
                                value: Some(scalar_to_proto(
                                    &space,
                                    &Scalar::literal(Value::I64(99)),
                                )),
                            }],
                            returning: false,
                            schema: Some(common::claim("docs")),
                        },
                    )),
                },
            ],
            pb::Atomicity::Independent,
        )))
        .await
        .expect("a mixed batch")
        .into_inner();

    assert_eq!(response.results.len(), 3);
    // The delete removed 6 and 7 and returned them, because it asked.
    match response.results[1].of.as_ref() {
        Some(pb::batch_result::Of::Ok(write)) => {
            assert_eq!(write.affected, 2);
            assert_eq!(write.rows.len(), 2, "returning rode through the batch");
        }
        other => panic!("expected an ok, got {other:?}"),
    }
    // The update touched the five that were left and returned none, because it
    // did not ask.
    match response.results[2].of.as_ref() {
        Some(pb::batch_result::Of::Ok(write)) => {
            assert_eq!(write.affected, 5);
            assert!(write.rows.is_empty());
        }
        other => panic!("expected an ok, got {other:?}"),
    }
    assert_eq!(ids(&mut client).await, vec![1, 2, 3, 4, 5]);
}

/// An operation carrying its own `transaction` is refused, not ignored.
#[tokio::test]
async fn an_operation_may_not_name_its_own_transaction() {
    let serving = serving().await;
    let mut client = serving.client().await;

    let mut operation = insert(1);
    if let Some(pb::batch_operation::Of::Insert(r)) = operation.of.as_mut() {
        r.transaction = "some-transaction".to_owned();
    }
    let status = client
        .batch(app(batch(vec![operation], pb::Atomicity::Independent)))
        .await
        .expect_err("an operation with its own transaction should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("transaction"),
        "{}",
        status.message()
    );
    assert!(ids(&mut client).await.is_empty(), "nothing was written");
}

/// Independence inside a transaction is two contradictory requests.
#[tokio::test]
async fn an_independent_batch_may_not_run_inside_a_transaction() {
    let serving = serving().await;
    let mut client = serving.client().await;
    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .expect("begin")
        .into_inner()
        .transaction;

    let status = client
        .batch(app(pb::BatchRequest {
            operations: vec![insert(1)],
            atomicity: pb::Atomicity::Independent as i32,
            transaction: transaction.clone(),
        }))
        .await
        .expect_err("independent-inside-a-transaction should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);

    client
        .rollback(app(pb::RollbackRequest { transaction }))
        .await
        .expect("rollback");
}

/// An atomic batch inside a caller's transaction joins it rather than
/// committing, and rolls back with it.
#[tokio::test]
async fn an_atomic_batch_joins_an_open_transaction() {
    let serving = serving().await;
    let mut client = serving.client().await;
    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .expect("begin")
        .into_inner()
        .transaction;

    let response = client
        .batch(app(pb::BatchRequest {
            operations: (1..=3).map(insert).collect(),
            atomicity: pb::Atomicity::AllOrNothing as i32,
            transaction: transaction.clone(),
        }))
        .await
        .expect("a batch inside a transaction")
        .into_inner();
    // No sequence: the transaction has not committed, and the batch is not
    // the thing that commits it.
    assert_eq!(response.sequence, None);

    client
        .rollback(app(pb::RollbackRequest { transaction }))
        .await
        .expect("rollback");
    assert!(ids(&mut client).await.is_empty(), "the rollback undid it");
}

/// A decode failure anywhere applies nothing, including under INDEPENDENT.
///
/// The interesting half is `INDEPENDENT`: a lazy decode would apply the first
/// operations and then fail on the ninth, handing the caller a
/// malformed-request error beside eight rows that had already landed.
#[tokio::test]
async fn a_malformed_operation_stops_the_batch_before_any_of_it_runs() {
    let serving = serving().await;
    let mut client = serving.client().await;

    let mut broken = insert(9);
    if let Some(pb::batch_operation::Of::Insert(r)) = broken.of.as_mut() {
        // A claim that is not this table's.
        let mut wrong = common::claim("docs");
        wrong.fingerprint ^= 1;
        r.schema = Some(wrong);
    }
    let status = client
        .batch(app(batch(
            vec![insert(1), insert(2), broken],
            pb::Atomicity::Independent,
        )))
        .await
        .expect_err("a malformed operation should refuse the batch");
    assert_eq!(status.code(), Code::InvalidArgument);
    // Which operation, by index. A batch of fifty whose message is "schema
    // mismatch" and nothing else costs the caller a bisection.
    assert!(
        status.message().contains("operation 2"),
        "{}",
        status.message()
    );

    assert!(
        ids(&mut client).await.is_empty(),
        "the two valid operations before it must not have been applied"
    );
}

/// The measurement: round trips, exactly, and wall clock, reported.
#[tokio::test]
async fn a_batch_is_one_round_trip() {
    const N: u64 = 50;
    let serving = serving().await;
    let mut client = serving.client().await;

    // Singly: N requests, by construction. Counted rather than inferred —
    // this loop *is* the round-trip count.
    let mut singly = 0u32;
    let started = Instant::now();
    for id in 1..=N {
        client
            .batch(app(batch(vec![insert(id)], pb::Atomicity::Independent)))
            .await
            .expect("a single insert");
        singly += 1;
    }
    let alone = started.elapsed();

    let started = Instant::now();
    client
        .batch(app(batch(
            (N + 1..=N * 2).map(insert).collect(),
            pb::Atomicity::Independent,
        )))
        .await
        .expect("one batch");
    let batched = started.elapsed();
    let batched_trips = 1u32;

    assert_eq!(singly, N as u32, "one request per row, sent singly");
    assert_eq!(batched_trips, 1, "one request for all of them, batched");
    assert_eq!(ids(&mut client).await.len(), (N * 2) as usize);

    // Reported, not asserted. This runs over a loopback socket against an
    // in-memory store, where a round trip is microseconds and the saving is
    // real but small; on a network it is the whole cost. A threshold here
    // would be measuring the runner.
    println!(
        "round trips: {singly} singly in {alone:?}, {batched_trips} batched in {batched:?} \
         ({:.1}x)",
        alone.as_secs_f64() / batched.as_secs_f64().max(f64::MIN_POSITIVE)
    );
    // The one thing worth failing on: batching must not be *slower* by an
    // order of magnitude, which would mean the server serialised something it
    // should not have.
    assert!(
        batched < alone * 10,
        "batched {batched:?} against {alone:?} singly — batching should not cost more"
    );
}

/// A batch over `max_batch_operations` is refused, and says both numbers.
///
/// The cap had no test at all when it was added — a `Limits` field, a daemon
/// config key and an `if`, none of which anything exercised. A limit nothing
/// tests is a limit that can stop working without anyone noticing, which is
/// the same class as the `protoc` guard that had never run.
#[tokio::test]
async fn a_batch_over_the_cap_is_refused() {
    let leadership = common::leader().await;
    let serving = common::serve(common::head_with(
        Arc::new(MemoryStore::new()),
        Vec::new(),
        leadership,
        common::config().with_limits(slate_server::Limits {
            // Two rather than the real thousand: a ceiling is only testable by
            // setting it low enough to reach, and sending a thousand and one
            // operations would be slow and would pin nothing extra.
            max_batch_operations: Some(2),
            ..slate_server::Limits::default()
        }),
    ))
    .await;
    let mut client = serving.client().await;

    // Two is allowed.
    client
        .batch(app(batch(
            vec![insert(1), insert(2)],
            pb::Atomicity::Independent,
        )))
        .await
        .expect("a batch at the cap");

    // Three is not, and the message names the cap and the size — a refusal
    // that says only "too many" leaves the caller guessing at both.
    let status = client
        .batch(app(batch(
            vec![insert(3), insert(4), insert(5)],
            pb::Atomicity::Independent,
        )))
        .await
        .expect_err("a batch over the cap should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(status.message().contains('2'), "{}", status.message());
    assert!(status.message().contains('3'), "{}", status.message());

    // And nothing from the refused batch landed.
    assert_eq!(ids(&mut client).await, vec![1, 2]);
}
