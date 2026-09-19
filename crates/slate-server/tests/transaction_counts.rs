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

// --- what a refused statement contributes ------------------------------------
//
// Found by re-reading this file's own source rather than by a failure: `insert`
// skipped the tally when it failed and `update` recorded a zero, and every test
// above passed under either spelling. Two arms of one enum disagreeing about
// the same question is the kind of thing that stays true until somebody reads
// it, so the rule is now one function — `Tally::applied` — and these are what
// pin it.
//
// The rule: a statement contributes unless it *both* failed and applied
// nothing. It is the rule the standalone path already follows, which
// `purge_wire.rs::a_refused_purge_reports_nothing` pins from the other side.

#[tokio::test]
async fn a_statement_that_failed_having_applied_nothing_contributes_no_series() {
    // Not "contributes a zero". A counter that moved on a refusal would make a
    // permission problem look like data loss, which is the argument the
    // standalone path was built on; a transaction is the same claim, delayed.
    //
    // The commit is what makes this observable at all: a rollback drops the
    // tally whatever is in it, so the difference between "no series" and "a
    // zero series" can only be seen by a transaction that failed a statement
    // and committed anyway — which a caller may do, because a refused
    // statement does not end the transaction.
    //
    // The refusal is the *only* statement on purpose. The first draft of this
    // test put a successful insert of key 1 in front of it and expected a
    // total of one; a mutation dropping the `ok` clause from `Tally::applied`
    // passed it, because the tally sums by (statement, table) and a zero folded
    // into that one is still one. A series that must not exist has to be tested
    // where nothing else can hide it.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;

    // Key 1, committed, so the refusal below is a duplicate rather than the
    // first thing this transaction does.
    let setup = begin(&mut client).await;
    client
        .insert(app(insert(&setup, &[1])))
        .await
        .expect("setup");
    client
        .commit(app(pb::CommitRequest {
            transaction: setup.clone(),
        }))
        .await
        .expect("setup commit");
    let before = seen.calls();

    let txn = begin(&mut client).await;
    client
        .insert(app(insert(&txn, &[2, 1])))
        .await
        .expect_err("a batch naming a taken key is refused");
    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");

    // `insert_many` validates the whole batch before writing any of it, so the
    // refusal applied nothing — not the two rows it named, and not the one that
    // was free. Checked rather than assumed: key 2 is absent below.
    assert_eq!(
        seen.calls(),
        before,
        "the refused transaction contributed a series"
    );

    let read = begin(&mut client).await;
    let got = client
        .get(app(pb::GetRequest {
            transaction: read.clone(),
            table: "docs".to_owned(),
            primary_key: Some(pb::Row {
                values: vec![slate_server::convert::value_to_proto(&Value::U64(2))],
                computed: Vec::new(),
            }),
            freshness: None,
            schema: Some(common::claim("docs")),
        }))
        .await
        .expect("get")
        .into_inner();
    assert!(!got.found, "the free half of the refused batch was written");
}

#[tokio::test]
async fn a_statement_that_failed_having_applied_some_contributes_those() {
    // The other clause, and the one that is not obvious. A conditional update
    // applies rows one at a time and stops at the first refusal, so a batch of
    // two where the second has moved leaves *one* row in the transaction's
    // buffer. The caller may still commit, and that row lands.
    //
    // Dropping it because the statement failed would under-report a write that
    // happened, which is the mirror of the test above and the reason the rule
    // has two clauses rather than "count successes".
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;

    // Two rows to update conditionally, committed first so the transaction
    // below has something to read.
    let setup = begin(&mut client).await;
    client
        .insert(app(insert(&setup, &[1, 2])))
        .await
        .expect("setup");
    client
        .commit(app(pb::CommitRequest {
            transaction: setup.clone(),
        }))
        .await
        .expect("setup commit");

    let txn = begin(&mut client).await;
    let moved = doc(2, "not what the caller expects", 2, None);
    let outcome = client
        .update(app(pb::UpdateRequest {
            transaction: txn.clone(),
            table: "docs".to_owned(),
            rows: vec![
                slate_server::convert::row_to_proto(&doc(1, "edited", 1, None)),
                slate_server::convert::row_to_proto(&doc(2, "edited", 2, None)),
            ],
            // Row 1's expectation is what is there; row 2's is not, so the
            // loop applies the first and is refused on the second.
            expected: vec![
                slate_server::convert::row_to_proto(&doc(1, "written", 1, None)),
                slate_server::convert::row_to_proto(&moved),
            ],
            schema: Some(common::claim("docs")),
        }))
        .await;
    assert!(outcome.is_err(), "the second row's expectation should fail");

    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");

    let mut calls = seen.calls();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            ("insert", "docs".to_owned(), 2),
            // One, not two and not zero: the row the loop got through before
            // it stopped.
            ("update", "docs".to_owned(), 1),
        ]
    );
}

// The two arms below exist because a mutation survived. Turning `update` and
// `delete` back to the unconditional `Tally::add` they used before `applied`
// existed left all eleven tests green: the insert test above pins the rule for
// its own arm and says nothing about the other two, which is the same
// arm-by-arm blindness that let the arms disagree in the first place.

#[tokio::test]
async fn a_refused_update_that_applied_nothing_contributes_no_series() {
    // `update_many` is all-or-nothing and fails before writing, so a batch
    // naming a row that is not there applies zero. Under the old spelling this
    // reported `("update", "docs", 0)` — a series that says "a write ran and
    // touched nothing", which is what an operator watches a retention sweep
    // for, raised here by a request that was refused outright.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;
    let txn = begin(&mut client).await;

    client
        .update(app(pb::UpdateRequest {
            transaction: txn.clone(),
            table: "docs".to_owned(),
            rows: vec![slate_server::convert::row_to_proto(&doc(
                7, "edited", 7, None,
            ))],
            expected: Vec::new(),
            schema: Some(common::claim("docs")),
        }))
        .await
        .expect_err("a row that is not there cannot be updated");
    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");

    assert!(
        seen.calls().is_empty(),
        "a refused update reported: {:?}",
        seen.calls()
    );
}

#[tokio::test]
async fn a_refused_conditional_delete_that_applied_nothing_contributes_no_series() {
    // The conditional delete's own loop, refused on its first key. It is the
    // arm that *can* apply a prefix, which is exactly why "applied nothing"
    // has to be tested separately from "applied some": the count is a
    // variable here rather than the all-or-nothing arms' constant.
    let (serving, seen) = watched().await;
    let mut client = serving.client().await;

    let setup = begin(&mut client).await;
    client
        .insert(app(insert(&setup, &[1])))
        .await
        .expect("setup");
    client
        .commit(app(pb::CommitRequest {
            transaction: setup.clone(),
        }))
        .await
        .expect("setup commit");
    let before = seen.calls();

    let txn = begin(&mut client).await;
    client
        .delete(app(pb::DeleteRequest {
            transaction: txn.clone(),
            table: "docs".to_owned(),
            primary_keys: vec![pb::Row {
                values: vec![slate_server::convert::value_to_proto(&Value::U64(1))],
                computed: Vec::new(),
            }],
            // Not what is stored, so the first and only key is refused.
            expected: vec![slate_server::convert::row_to_proto(&doc(
                1,
                "not what the caller expects",
                1,
                None,
            ))],
            schema: Some(common::claim("docs")),
        }))
        .await
        .expect_err("the expectation does not match");
    client
        .commit(app(pb::CommitRequest {
            transaction: txn.clone(),
        }))
        .await
        .expect("commit");

    assert_eq!(
        seen.calls(),
        before,
        "a refused conditional delete reported a series"
    );
}

// --- the arm with no test, and the two routes that do not reach it -----------
//
// `Command::Delete`'s plain (non-conditional) branch shares the rule above
// through the same `tally.applied` call, but its "failed having applied
// nothing" case has no test, because nothing in this fixture can produce it
// over the wire. Written down rather than left as an absence, since an absence
// reads as "nobody thought of it":
//
//   - **An absent key is not a failure.** `transaction.delete` answers
//     `Ok(false)`, deliberately, so a caller cannot use the count to learn
//     whether a row the policy hides was there. The loop keeps going.
//   - **An unauthorized delete never reaches the arm.** Measured, not assumed:
//     a principal holding a role with no grant on `docs` gets
//     `PermissionDenied`, and turning this arm back to the unconditional `add`
//     leaves that case green — the refusal is raised before the session task is
//     dispatched to at all, so the tally is never touched.
//
// What is left is a genuine kernel error mid-loop: a foreign-key restriction,
// a storage failure, a fence. This fixture declares no foreign keys, and adding
// one to a `TableDef` every test in the crate shares is a larger change than
// the hole is worth. The conditional branch's test one screen up covers the
// identical call two lines away, which is the reason to stop here rather than
// the reason there is nothing missing.
