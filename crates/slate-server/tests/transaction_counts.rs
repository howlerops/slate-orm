//! What a transaction wrote, told to a [`WriteObserver`] only if it committed.
//!
//! The observer shipped counting *standalone* writes, because `autocommit` is
//! one funnel every one of them passes through. Everything a caller did inside
//! its own transaction was invisible, and so was an atomic batch: a deployment
//! that wrapped its writes in `Begin`/`Commit` — which is most of the reason to
//! run a database with transactions — reported zero rows written forever. The
//! metric said "nothing is happening" about a node under load, which is worse
//! than having no metric, because an operator believes it.
//!
//! The rule the counting has to obey is the one `autocommit` already obeys and
//! is harder here: tell the observer what *landed*. A standalone write can be
//! retried and applied twice while committing once; a caller's transaction can
//! be rolled back, abandoned, or fenced after it has written a great deal. So
//! the counts accumulate in the task and are reported on `Commit` and nowhere
//! else, which is what every test below is checking one face of.
//!
//! These are Rust tests and not conformance cases on purpose: the conformance
//! runner compares three clients against each other and cannot see a server
//! that counted nothing, because all three would agree about the answer to the
//! write. Only something that knows the expected count can.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, doc, docs, serving_leader_observed};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{CmpOp, Expr};
use slate_schema::Ordinal;
use slate_server::WriteObserver;
use slate_server::convert::{Space, expr_to_proto};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::sync::Arc;
use tonic::transport::Channel;

const KIND: Ordinal = Ordinal(1);

/// The kind-equals filter these tests delete by, on the wire.
fn kind_is(kind: &str) -> pb::Expr {
    expr_to_proto(
        &Space::input(&docs(), 0, 0),
        &Expr::compare(KIND, CmpOp::Eq, Value::Str(kind.to_owned())),
    )
}

/// Records what a [`WriteObserver`] is told.
///
/// The same double `purge_wire.rs` uses, duplicated rather than shared: a test
/// helper that two files depend on is a third thing to keep in step, and this
/// is nine lines. If a third file wants it, that is the moment to move it.
#[derive(Default)]
struct Recorded(std::sync::Mutex<Vec<(&'static str, String, u64)>>);

impl WriteObserver for Recorded {
    fn wrote(&self, kind: &'static str, table: &str, affected: u64) {
        self.0
            .lock()
            .expect("no panic holds this")
            .push((kind, table.to_owned(), affected));
    }
}

impl Recorded {
    /// Everything the observer has been told, in the order it was told.
    fn calls(&self) -> Vec<(&'static str, String, u64)> {
        self.0.lock().expect("no panic holds this").clone()
    }
}

/// A serving leader and the observer attached to it.
async fn watched() -> (common::Serving, Arc<Recorded>) {
    let seen = Arc::new(Recorded::default());
    let serving = serving_leader_observed(
        Arc::new(MemoryStore::new()),
        Arc::clone(&seen) as Arc<dyn WriteObserver>,
    )
    .await;
    (serving, seen)
}

/// Open a transaction and answer its handle.
async fn begin(client: &mut RecordsClient<Channel>) -> String {
    client
        .begin(app(pb::BeginRequest {}))
        .await
        .expect("begin")
        .into_inner()
        .transaction
}

fn insert(transaction: &str, ids: &[u64]) -> pb::InsertRequest {
    pb::InsertRequest {
        transaction: transaction.to_owned(),
        table: "docs".to_owned(),
        rows: ids
            .iter()
            .map(|id| slate_server::convert::row_to_proto(&doc(*id, "written", *id as i64, None)))
            .collect(),
        upsert: false,
        schema: Some(common::claim("docs")),
    }
}

#[tokio::test]
async fn a_committed_transactions_writes_are_counted() {
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;

    client
        .insert(app(insert(&txn, &[1, 2, 3])))
        .await
        .expect("insert");
    // Nothing yet: the rows exist only in this transaction's buffer, and a
    // counter that moved here would have to move back if the caller rolled
    // back, which a counter cannot do.
    assert!(
        seen.calls().is_empty(),
        "counted before the commit: {:?}",
        seen.calls()
    );

    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");
    assert_eq!(
        seen.calls(),
        vec![("insert", "docs".to_owned(), 3)],
        "one call, naming the statement, the table and the count"
    );
}

#[tokio::test]
async fn a_rolled_back_transactions_writes_are_not() {
    // The reason the counting waits. Three rows were written and none of them
    // landed; a series that included them would be reporting rows that never
    // existed, which is precisely the question "how many rows did this node
    // write" is asked to answer.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;

    client
        .insert(app(insert(&txn, &[1, 2, 3])))
        .await
        .expect("insert");
    client
        .rollback(app(pb::RollbackRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("rollback");
    assert!(seen.calls().is_empty(), "{:?}", seen.calls());
}

#[tokio::test]
async fn a_refused_statement_does_not_flush_what_came_before_it() {
    // Found by a mutation that survived. Reporting the tally when a command
    // *fails* looks harmless — a failed statement adds nothing to it — but the
    // tally is the whole transaction's, so flushing it there publishes every
    // row the transaction wrote before the failure, and then the caller rolls
    // back and none of them exist. The rollback test above cannot see it,
    // because it has no failing statement; this one is that test with a
    // refusal wedged in the middle.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;

    client
        .insert(app(insert(&txn, &[1, 2, 3])))
        .await
        .expect("insert");
    // The same keys again, without `upsert`: the kernel refuses it.
    client
        .insert(app(insert(&txn, &[1])))
        .await
        .expect_err("a duplicate key should be refused");
    assert!(
        seen.calls().is_empty(),
        "the refusal published the transaction's tally: {:?}",
        seen.calls()
    );

    client
        .rollback(app(pb::RollbackRequest { transaction: txn }))
        .await
        .expect("rollback");
    assert!(seen.calls().is_empty(), "{:?}", seen.calls());
}

#[tokio::test]
async fn two_writes_to_one_table_are_summed_not_repeated() {
    // A counter is a sum. Two calls of two would also add to four in
    // Prometheus, so this is not about the arithmetic — it is about the
    // observer being told once per (statement, table) per transaction rather
    // than once per statement, which is what keeps a transaction of ten
    // thousand small inserts from being ten thousand observer calls.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;

    client.insert(app(insert(&txn, &[1, 2]))).await.expect("a");
    client.insert(app(insert(&txn, &[3]))).await.expect("b");
    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");
    assert_eq!(seen.calls(), vec![("insert", "docs".to_owned(), 3)]);
}

#[tokio::test]
async fn each_statement_is_its_own_series() {
    // Summing is per (statement, table), not per table: an operator watching a
    // retention sweep needs `delete_where` on its own, and a node whose deletes
    // were folded into its inserts would report a plausible total and nothing
    // usable.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;

    client
        .insert(app(insert(&txn, &[1, 2, 3])))
        .await
        .expect("insert");
    client
        .delete_where(app(pb::DeleteWhereRequest {
            transaction: txn.clone(),
            table: "docs".to_owned(),
            filter: Some(kind_is("written")),
            returning: false,
            schema: Some(common::claim("docs")),
        }))
        .await
        .expect("delete_where");
    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");

    let mut calls = seen.calls();
    // The tally is a map, so its order is the key's rather than the caller's.
    // Sorting says the set is right without pinning an order nothing promises.
    calls.sort();
    assert_eq!(
        calls,
        vec![
            ("delete_where", "docs".to_owned(), 3),
            ("insert", "docs".to_owned(), 3),
        ]
    );
}

#[tokio::test]
async fn an_atomic_batch_in_its_own_transaction_is_counted() {
    // A batch with no `transaction` opens one, applies everything and commits,
    // all inside the one call. It never touched `autocommit`, so it was as
    // invisible as a caller's transaction and for a different reason.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    client
        .batch(app(pb::BatchRequest {
            operations: vec![
                pb::BatchOperation {
                    of: Some(pb::batch_operation::Of::Insert(insert("", &[1, 2]))),
                },
                pb::BatchOperation {
                    of: Some(pb::batch_operation::Of::Insert(insert("", &[3]))),
                },
            ],
            atomicity: pb::Atomicity::AllOrNothing as i32,
            transaction: String::new(),
        }))
        .await
        .expect("batch");

    // Two operations, so two calls: this path has no tally, because the whole
    // batch is known before any of it is reported and a `Vec` says the same
    // thing. Summing here would hide an operation, which is the opposite of
    // what the session's map is for.
    assert_eq!(
        seen.calls(),
        vec![
            ("insert", "docs".to_owned(), 2),
            ("insert", "docs".to_owned(), 1),
        ]
    );
}

#[tokio::test]
async fn an_atomic_batch_inside_a_transaction_waits_for_the_commit() {
    // The third path, and the one that looks most like the second: a batch
    // that names a transaction does not commit, so it goes through the session
    // and is counted with everything else that transaction did.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;

    client
        .batch(app(pb::BatchRequest {
            // The operation's own `transaction` stays empty and the batch's
            // carries the handle; the server refuses it the other way round,
            // because two places to say it is two places to disagree.
            operations: vec![pb::BatchOperation {
                of: Some(pb::batch_operation::Of::Insert(insert("", &[1, 2]))),
            }],
            atomicity: pb::Atomicity::AllOrNothing as i32,
            transaction: txn.clone(),
        }))
        .await
        .expect("batch");
    assert!(seen.calls().is_empty(), "{:?}", seen.calls());

    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");
    assert_eq!(seen.calls(), vec![("insert", "docs".to_owned(), 2)]);
}

#[tokio::test]
async fn a_transaction_that_wrote_nothing_reports_nothing() {
    // Zero is reported for a *statement* that touched no rows — a purge that
    // erased nothing is worth a series — but a transaction that ran no
    // statement has no statement to label, so there is nothing to say. The
    // distinction matters because the opposite implementation, one empty call
    // per commit, would need a kind and a table it does not have.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;
    client
        .commit(app(pb::CommitRequest { transaction: txn }))
        .await
        .expect("commit");
    assert!(seen.calls().is_empty(), "{:?}", seen.calls());
}

#[tokio::test]
async fn a_statement_that_matched_nothing_still_reports_its_zero() {
    // And the other half of that distinction: the statement ran, so the series
    // exists. A `delete_where` that stopped matching is the failure a retention
    // sweep's operator is watching for, and it looks identical to a sweep
    // nobody scheduled if the zero is dropped.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;
    client
        .delete_where(app(pb::DeleteWhereRequest {
            transaction: txn.clone(),
            table: "docs".to_owned(),
            filter: Some(kind_is("absent")),
            returning: false,
            schema: Some(common::claim("docs")),
        }))
        .await
        .expect("delete_where");
    client
        .commit(app(pb::CommitRequest { transaction: txn }))
        .await
        .expect("commit");
    assert_eq!(seen.calls(), vec![("delete_where", "docs".to_owned(), 0)]);
}
