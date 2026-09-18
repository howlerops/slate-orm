//! A conditional delete over the wire.
//!
//! `update_if_unchanged` shipped first because overwriting somebody else's
//! edit is the loss everybody recognises. Deleting a row somebody else just
//! edited is the same mistake: the caller read the row, decided *from what it
//! said* that it should go, and by the time the delete lands it says something
//! else. A moderator deleting a post because it had no views deletes one with
//! five thousand, and the count comes back `1`.
//!
//! # The one place this is not just "the update's twin"
//!
//! A plain delete reports an absent key as `affected: 0`, because "make sure
//! this is gone" is idempotent and a caller asking that wants no error. A
//! *conditional* delete refuses it. The caller said what it expected to find;
//! "it was already gone" is an answer it wants to hear rather than a smaller
//! number it will read as success. `affected` is therefore always the number
//! of keys sent — anything less would be reporting a state this call refused.
//!
//! The tests below are written around that asymmetry, because it is the part
//! a reader coming from the update will not expect.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, doc, docs, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;

/// Four documents: ids 0..4, `kind-n`, size = id, no note.
async fn seeded() -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let rows: Vec<_> = (0..4_u64)
            .map(|id| doc(id, &format!("kind-{id}"), id as i64, None))
            .collect();
        let txn = store.begin().await.unwrap();
        txn.insert_many(&context, &docs(), &rows).await.unwrap();
        txn.commit().await.unwrap();
    }
    serving_leader(backing).await
}

fn u64_value(n: u64) -> pb::Value {
    pb::Value {
        kind: Some(pb::value::Kind::Uint64Value(n)),
    }
}

fn i64_value(n: i64) -> pb::Value {
    pb::Value {
        kind: Some(pb::value::Kind::Int64Value(n)),
    }
}

fn text(s: &str) -> pb::Value {
    pb::Value {
        kind: Some(pb::value::Kind::StringValue(s.to_owned())),
    }
}

fn null() -> pb::Value {
    pb::Value {
        kind: Some(pb::value::Kind::NullValue(pb::NullValue::NullValue as i32)),
    }
}

/// A whole `docs` row, as the caller would have read it.
fn doc_row(id: u64, size: i64) -> pb::Row {
    common::wire_row(vec![
        u64_value(id),
        text(&format!("kind-{id}")),
        i64_value(size),
        null(),
    ])
}

fn key(id: u64) -> pb::Row {
    common::wire_row(vec![u64_value(id)])
}

fn delete(keys: Vec<pb::Row>, expected: Vec<pb::Row>) -> pb::DeleteRequest {
    pb::DeleteRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        primary_keys: keys,
        schema: Some(common::claim("docs")),
        expected,
    }
}

async fn present(client: &mut RecordsClient<Channel>, id: u64) -> bool {
    client
        .get(app(pb::GetRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            primary_key: Some(key(id)),
            freshness: None,
            schema: Some(common::claim("docs")),
        }))
        .await
        .expect("get")
        .into_inner()
        .found
}

#[tokio::test]
async fn a_delete_naming_the_row_it_read_is_applied() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let response = client
        .delete(app(delete(vec![key(2)], vec![doc_row(2, 2)])))
        .await
        .expect("the row is unchanged")
        .into_inner();
    assert_eq!(response.affected, 1);
    assert!(!present(&mut client, 2).await);
}

#[tokio::test]
async fn a_delete_naming_a_row_that_moved_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    // Somebody else's edit lands first.
    client
        .update(app(pb::UpdateRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![doc_row(2, 5_000)],
            schema: Some(common::claim("docs")),
            expected: Vec::new(),
        }))
        .await
        .expect("the edit");

    let status = client
        .delete(app(delete(vec![key(2)], vec![doc_row(2, 2)])))
        .await
        .expect_err("the row moved");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert!(
        present(&mut client, 2).await,
        "a refused conditional delete removed the row anyway"
    );
}

#[tokio::test]
async fn a_row_already_gone_is_refused_rather_than_counted_as_absent() {
    // The asymmetry with a plain delete, and the reason this file exists
    // separately from the update's. Both halves in one test, because the
    // refusal only means something beside the `affected: 0` it replaces.
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .delete(app(delete(vec![key(3)], Vec::new())))
        .await
        .expect("a plain delete");

    let plain = client
        .delete(app(delete(vec![key(3)], Vec::new())))
        .await
        .expect("a plain delete of an absent key is not an error")
        .into_inner();
    assert_eq!(plain.affected, 0);

    let status = client
        .delete(app(delete(vec![key(3)], vec![doc_row(3, 3)])))
        .await
        .expect_err("the row is gone");
    // `NotFound`, not `Aborted`, and the distinction is worth the assertion:
    // `Aborted` is "you raced somebody, re-read and try again" and this is not
    // that. The row is gone and re-reading will not bring it back, so a caller
    // retrying on `Aborted` would loop. The stable reason token says which.
    assert_eq!(status.code(), Code::NotFound, "{status:?}");
    assert!(
        String::from_utf8_lossy(status.details()).contains("ROW_NOT_FOUND"),
        "{status:?}"
    );
}

#[tokio::test]
async fn an_expectation_for_a_different_row_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let status = client
        .delete(app(delete(vec![key(1)], vec![doc_row(2, 2)])))
        .await
        .expect_err("the expectation names another row");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert!(present(&mut client, 1).await);
    assert!(present(&mut client, 2).await);
}

#[tokio::test]
async fn one_stale_row_refuses_the_whole_statement() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .update(app(pb::UpdateRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![doc_row(1, 99)],
            schema: Some(common::claim("docs")),
            expected: Vec::new(),
        }))
        .await
        .expect("move row 1");

    // Stale row *first*, so a loop that reported the last outcome would answer
    // with a success.
    let status = client
        .delete(app(delete(
            vec![key(1), key(2)],
            vec![doc_row(1, 1), doc_row(2, 2)],
        )))
        .await
        .expect_err("the first row moved");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert!(present(&mut client, 1).await);
    assert!(
        present(&mut client, 2).await,
        "the second row was deleted by a refused statement"
    );
}

#[tokio::test]
async fn expected_must_name_one_row_per_key() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let status = client
        .delete(app(delete(vec![key(0), key(1)], vec![doc_row(0, 0)])))
        .await
        .expect_err("two keys and one expectation");
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    assert!(
        status.message().contains("2 key(s) and 1 expected"),
        "{status:?}"
    );
    // Not a silent prefix: zipping and stopping at the shorter would have
    // deleted key 1 unguarded.
    assert!(present(&mut client, 1).await);
}

#[tokio::test]
async fn an_empty_expected_is_an_ordinary_delete() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let response = client
        .delete(app(delete(vec![key(0), key(1)], Vec::new())))
        .await
        .expect("no expectation, no condition")
        .into_inner();
    assert_eq!(response.affected, 2);
    assert!(!present(&mut client, 0).await);
    assert!(!present(&mut client, 1).await);
}

#[tokio::test]
async fn a_conditional_delete_works_inside_a_transaction() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let begun = client
        .begin(app(pb::BeginRequest::default()))
        .await
        .expect("begin")
        .into_inner();

    client
        .delete(app(pb::DeleteRequest {
            transaction: begun.transaction.clone(),
            ..delete(vec![key(2)], vec![doc_row(2, 2)])
        }))
        .await
        .expect("unchanged inside the transaction too");

    client
        .rollback(app(pb::RollbackRequest {
            transaction: begun.transaction,
        }))
        .await
        .expect("rollback");
    assert!(
        present(&mut client, 2).await,
        "the rollback did not restore it"
    );
}

#[tokio::test]
async fn a_stale_conditional_delete_inside_a_transaction_is_refused() {
    // The session path is separate code from the autocommit one, and only a
    // *stale* row tells a conditional delete apart from a plain one — an
    // unchanged row is removed identically either way. This is the test the
    // equivalent mutation on the update path survived without.
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .update(app(pb::UpdateRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![doc_row(2, 77)],
            schema: Some(common::claim("docs")),
            expected: Vec::new(),
        }))
        .await
        .expect("move the row");

    let begun = client
        .begin(app(pb::BeginRequest::default()))
        .await
        .expect("begin")
        .into_inner();
    let status = client
        .delete(app(pb::DeleteRequest {
            transaction: begun.transaction.clone(),
            ..delete(vec![key(2)], vec![doc_row(2, 2)])
        }))
        .await
        .expect_err("the row moved");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");

    client
        .rollback(app(pb::RollbackRequest {
            transaction: begun.transaction,
        }))
        .await
        .expect("rollback");
    assert!(present(&mut client, 2).await);
}

#[tokio::test]
async fn a_mismatched_pair_is_reported_as_such_and_not_as_a_missing_row() {
    // Written because a mutation survived without it. Removing the kernel's
    // key-comparison leaves every other case answering `ROW_CHANGED` anyway —
    // the read finds the row at the key, the contents differ, and the outcome
    // is the same. This is the one case where it is not: the key is *absent*,
    // so without the comparison the caller is told "no such row" and goes
    // looking for a row that was never the problem. The mismatched pair is
    // their bug and the error should say so.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let status = client
        .delete(app(delete(vec![key(9_999)], vec![doc_row(2, 2)])))
        .await
        .expect_err("the key is absent and the expectation names another row");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert!(
        String::from_utf8_lossy(status.details()).contains("ROW_CHANGED"),
        "a mismatched pair should be reported as such, not as a missing row: {status:?}"
    );
}

#[tokio::test]
async fn a_stale_first_key_refuses_the_rest_inside_a_transaction() {
    // The session path's version of `one_stale_row_refuses_the_whole_statement`,
    // written because the actor's loop `break` survived a mutation without it:
    // the transaction case above deletes one key, and with one key the loop
    // cannot be observed to continue.
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .update(app(pb::UpdateRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![doc_row(1, 42)],
            schema: Some(common::claim("docs")),
            expected: Vec::new(),
        }))
        .await
        .expect("move row 1");

    let begun = client
        .begin(app(pb::BeginRequest::default()))
        .await
        .expect("begin")
        .into_inner();
    let status = client
        .delete(app(pb::DeleteRequest {
            transaction: begun.transaction.clone(),
            ..delete(vec![key(1), key(2)], vec![doc_row(1, 1), doc_row(2, 2)])
        }))
        .await
        .expect_err("the first key moved");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");

    client
        .rollback(app(pb::RollbackRequest {
            transaction: begun.transaction,
        }))
        .await
        .expect("rollback");
    assert!(
        present(&mut client, 2).await,
        "the second key was deleted by a refused statement"
    );
}

#[tokio::test]
async fn a_conditional_delete_works_inside_a_batch() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .batch(app(pb::BatchRequest {
            transaction: String::new(),
            atomicity: pb::Atomicity::AllOrNothing as i32,
            operations: vec![pb::BatchOperation {
                of: Some(pb::batch_operation::Of::Delete(delete(
                    vec![key(0)],
                    vec![doc_row(0, 0)],
                ))),
            }],
        }))
        .await
        .expect("batch");
    assert!(!present(&mut client, 0).await);

    let status = client
        .batch(app(pb::BatchRequest {
            transaction: String::new(),
            atomicity: pb::Atomicity::AllOrNothing as i32,
            operations: vec![pb::BatchOperation {
                of: Some(pb::batch_operation::Of::Delete(delete(
                    vec![key(1)],
                    vec![doc_row(1, 99)],
                ))),
            }],
        }))
        .await
        .expect_err("stale inside a batch too");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert!(present(&mut client, 1).await);
}
