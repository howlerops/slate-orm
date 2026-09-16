//! Predicate writes over the wire, and `RETURNING`.
//!
//! `delete_where` and `update_where` were built in the kernel and stopped
//! there, so a remote caller had to query the keys, carry them back and write
//! one per key — N+1 by construction, and not atomic with the query that found
//! them: a row inserted in between is missed, and a row deleted in between is
//! deleted twice.
//!
//! # Why `RETURNING` is here and not on `Insert`
//!
//! `RETURNING` exists to hand back what the caller could not otherwise know.
//! Over *this* wire an insert cannot produce any: the proto's `Row` is full
//! width and its nulls are values a caller meant, so there is no unset column
//! for a `DEFAULT` to fill, and there is no auto-increment, no trigger and no
//! generated column. The row written is the row sent. Returning it would hand
//! the caller its own request back.
//!
//! A predicate write is the opposite case. The caller named a *condition*, and
//! which rows matched is a fact it does not have — and for a delete it is a
//! fact that stops existing the moment the write lands, so a read afterwards
//! cannot recover it. `a_delete_returns_the_rows_it_destroyed` is the one that
//! could not be written any other way.

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
use tonic::Code;
use tonic::transport::Channel;

const ID: Ordinal = Ordinal(0);
const KIND: Ordinal = Ordinal(1);
const SIZE: Ordinal = Ordinal(2);

/// Twelve documents: ids 0..12, `kind-0`..`kind-3` round-robin, size = id.
async fn seeded() -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = docs();
        let rows: Vec<_> = (0..12_u64)
            .map(|id| doc(id, &format!("kind-{}", id % 4), id as i64, None))
            .collect();
        let txn = store.begin().await.unwrap();
        txn.insert_many(&context, &table, &rows).await.unwrap();
        txn.commit().await.unwrap();
    }
    serving_leader(backing).await
}

fn wire(expr: &Expr) -> pb::Expr {
    expr_to_proto(&Space::input(&docs(), 0, 0), expr)
}

fn assign(column: Ordinal, value: &Scalar) -> pb::Assignment {
    pb::Assignment {
        column: Some(column_ref(0, column.0)),
        value: Some(scalar_to_proto(&Space::input(&docs(), 0, 0), value)),
    }
}

fn delete_where(filter: Option<pb::Expr>, returning: bool) -> pb::DeleteWhereRequest {
    pb::DeleteWhereRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        filter,
        returning,
        schema: Some(common::claim("docs")),
    }
}

fn update_where(
    filter: Option<pb::Expr>,
    assignments: Vec<pb::Assignment>,
    returning: bool,
) -> pb::UpdateWhereRequest {
    pb::UpdateWhereRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        filter,
        assignments,
        returning,
        schema: Some(common::claim("docs")),
    }
}

/// The ids in a response's returned rows.
fn ids(response: &pb::WriteResponse) -> Vec<u64> {
    response
        .rows
        .iter()
        .map(
            |row| match row.values.first().and_then(|v| v.kind.as_ref()) {
                Some(pb::value::Kind::Uint64Value(id)) => *id,
                other => panic!("the first column was {other:?}"),
            },
        )
        .collect()
}

/// Every row still in the table, by id.
async fn remaining(client: &mut RecordsClient<Channel>) -> Vec<u64> {
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
    while let Some(message) = stream.message().await.expect("a query message") {
        rows.extend(message.rows);
    }
    common::doc_ids(&rows)
}

#[tokio::test]
async fn a_delete_removes_every_row_the_predicate_selects() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let response = client
        .delete_where(app(delete_where(
            Some(wire(&Expr::eq(KIND, Value::Str("kind-1".to_owned())))),
            false,
        )))
        .await
        .expect("a predicate delete")
        .into_inner();

    // 1, 5, 9 are the `kind-1` rows.
    assert_eq!(response.affected, 3);
    assert!(
        response.rows.is_empty(),
        "no rows were asked for: {:?}",
        response.rows
    );
    assert_eq!(
        remaining(&mut client).await,
        vec![0, 2, 3, 4, 6, 7, 8, 10, 11]
    );
}

/// The test that could not be written any other way.
///
/// After a delete the rows are gone, so there is no read that recovers them.
/// A caller that wanted to know what it destroyed either gets it in this
/// response or does not get it.
#[tokio::test]
async fn a_delete_returns_the_rows_it_destroyed() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let response = client
        .delete_where(app(delete_where(
            Some(wire(&Expr::compare(SIZE, CmpOp::Ge, Value::I64(9)))),
            true,
        )))
        .await
        .expect("a predicate delete")
        .into_inner();

    assert_eq!(response.affected, 3);
    assert_eq!(ids(&response), vec![9, 10, 11]);
    // And they really are gone, so the rows above are the only record of them.
    assert_eq!(remaining(&mut client).await, (0..9).collect::<Vec<_>>());
}

#[tokio::test]
async fn an_update_assigns_over_the_row_as_it_was_read() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    // size = size + 100, which is the whole point: one write, not a read, a
    // decision and a write.
    let bumped = Scalar::Add(
        Box::new(Scalar::column(SIZE)),
        Box::new(Scalar::literal(Value::I64(100))),
    );
    let response = client
        .update_where(app(update_where(
            Some(wire(&Expr::eq(KIND, Value::Str("kind-2".to_owned())))),
            vec![assign(SIZE, &bumped)],
            true,
        )))
        .await
        .expect("a predicate update")
        .into_inner();

    // 2, 6, 10 are the `kind-2` rows, and their sizes are their ids.
    assert_eq!(response.affected, 3);
    assert_eq!(ids(&response), vec![2, 6, 10]);
    let sizes: Vec<i64> = response
        .rows
        .iter()
        .map(|row| match row.values[2].kind.as_ref() {
            Some(pb::value::Kind::Int64Value(size)) => *size,
            other => panic!("size was {other:?}"),
        })
        .collect();
    assert_eq!(
        sizes,
        vec![102, 106, 110],
        "the returned rows are the rows *as written*, not as they were"
    );
}

/// Assignments read the original row, so they apply together.
#[tokio::test]
async fn assignments_are_simultaneous_not_sequential() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    // size = id, id stays — expressed as `size = size` and a second column, so
    // this needs a row whose two values differ. `kind` is a string, so the
    // swap is done on `size` against a literal read of itself twice over: if
    // the assignments were applied in order, the second would see the first.
    let response = client
        .update_where(app(update_where(
            Some(wire(&Expr::eq(ID, Value::U64(3)))),
            vec![
                assign(
                    SIZE,
                    &Scalar::Add(
                        Box::new(Scalar::column(SIZE)),
                        Box::new(Scalar::literal(Value::I64(1))),
                    ),
                ),
                assign(KIND, &Scalar::column(KIND)),
            ],
            true,
        )))
        .await
        .expect("a predicate update")
        .into_inner();

    assert_eq!(response.affected, 1);
    match response.rows[0].values[2].kind.as_ref() {
        Some(pb::value::Kind::Int64Value(size)) => assert_eq!(*size, 4),
        other => panic!("size was {other:?}"),
    }
}

#[tokio::test]
async fn a_predicate_that_matches_nothing_writes_nothing() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let response = client
        .delete_where(app(delete_where(
            Some(wire(&Expr::eq(KIND, Value::Str("kind-99".to_owned())))),
            true,
        )))
        .await
        .expect("a predicate delete")
        .into_inner();
    assert_eq!(response.affected, 0);
    assert!(response.rows.is_empty());
    assert_eq!(remaining(&mut client).await.len(), 12);
}

/// No filter is `DELETE FROM docs`, which is a real statement.
#[tokio::test]
async fn an_absent_filter_is_every_row() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let response = client
        .delete_where(app(delete_where(None, false)))
        .await
        .expect("a predicate delete with no filter")
        .into_inner();
    assert_eq!(response.affected, 12);
    assert!(remaining(&mut client).await.is_empty());
}

/// An update with nothing to assign is refused rather than reported as zero.
///
/// Zero rows written is what a predicate that matched nothing reports, and the
/// two are different mistakes.
#[tokio::test]
async fn an_update_with_no_assignments_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let status = client
        .update_where(app(update_where(None, Vec::new(), false)))
        .await
        .expect_err("an update with no assignments should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("assignment"),
        "the refusal should say what is missing: {}",
        status.message()
    );
}

/// The kernel's refusal, reaching the wire.
#[tokio::test]
async fn a_column_assigned_twice_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let status = client
        .update_where(app(update_where(
            None,
            vec![
                assign(SIZE, &Scalar::literal(Value::I64(1))),
                assign(SIZE, &Scalar::literal(Value::I64(2))),
            ],
            false,
        )))
        .await
        .expect_err("a column assigned twice should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
}

/// A caller that cannot write the table is refused before anything is read.
#[tokio::test]
async fn a_reader_may_not_write_by_predicate() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let status = client
        .delete_where(common::as_principal(
            delete_where(None, false),
            "u64:2",
            Some("u64:1"),
            "reader",
        ))
        .await
        .expect_err("a reader may not delete");
    assert_eq!(status.code(), Code::PermissionDenied);
    // And nothing was removed on the way to finding out.
    assert_eq!(remaining(&mut client).await.len(), 12);
}

/// The schema claim rides on a predicate write too.
///
/// It is attached at every call site by hand, so a new one that forgets it is
/// simply not checked and nothing else notices — and here it matters more than
/// usual, because the ordinals in the *predicate* are resolved against the
/// caller's idea of the table. A claim that disagrees means the filter selects
/// rows by a different column than the caller wrote, and then deletes them.
#[tokio::test]
async fn the_schema_claim_rides_on_a_predicate_write() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    // A fingerprint that is not this table's. `SchemaCheck` is a width and a
    // fingerprint, so a wrong declaration is a wrong number rather than a
    // reordered list — the hash is what carries the column names and types.
    let mut swapped = common::claim("docs");
    swapped.fingerprint ^= 1;

    let status = client
        .delete_where(app(pb::DeleteWhereRequest {
            schema: Some(swapped),
            ..delete_where(None, false)
        }))
        .await
        .expect_err("a misdeclared table should be refused before anything is deleted");
    assert_eq!(status.code(), Code::InvalidArgument);

    let status = client
        .update_where(app(pb::UpdateWhereRequest {
            schema: Some(swapped),
            ..update_where(
                None,
                vec![assign(SIZE, &Scalar::literal(Value::I64(1)))],
                false,
            )
        }))
        .await
        .expect_err("and on an update");
    assert_eq!(status.code(), Code::InvalidArgument);

    assert_eq!(
        remaining(&mut client).await.len(),
        12,
        "nothing was written"
    );
}

/// An assignment with a column and no value is refused, not skipped.
///
/// Skipping it would drop an assignment the caller wrote and report the rows
/// as written — the row would come back in `RETURNING` without the change the
/// caller asked for, which is the most convincing way to be wrong.
#[tokio::test]
async fn an_assignment_with_no_value_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let status = client
        .update_where(app(update_where(
            None,
            vec![pb::Assignment {
                column: Some(column_ref(0, SIZE.0)),
                value: None,
            }],
            false,
        )))
        .await
        .expect_err("an assignment with no value should be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("value"),
        "the refusal should say what is missing: {}",
        status.message()
    );

    // And one valid assignment beside a valueless one does not sneak through.
    let status = client
        .update_where(app(update_where(
            None,
            vec![
                assign(SIZE, &Scalar::literal(Value::I64(1))),
                pb::Assignment {
                    column: Some(column_ref(0, KIND.0)),
                    value: None,
                },
            ],
            false,
        )))
        .await
        .expect_err("a valueless assignment refuses the whole request");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(remaining(&mut client).await.len(), 12);
}

/// Inside a transaction, and rolled back.
#[tokio::test]
async fn a_predicate_write_in_a_transaction_rolls_back_with_it() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .expect("begin")
        .into_inner()
        .transaction;

    let response = client
        .delete_where(app(pb::DeleteWhereRequest {
            transaction: transaction.clone(),
            returning: true,
            ..delete_where(
                Some(wire(&Expr::compare(SIZE, CmpOp::Lt, Value::I64(4)))),
                true,
            )
        }))
        .await
        .expect("a predicate delete in a transaction")
        .into_inner();
    assert_eq!(ids(&response), vec![0, 1, 2, 3]);
    // No sequence: a write inside a transaction has none until it commits.
    assert_eq!(response.sequence, None);

    client
        .rollback(app(pb::RollbackRequest { transaction }))
        .await
        .expect("rollback");

    assert_eq!(
        remaining(&mut client).await.len(),
        12,
        "the rollback undid it"
    );
}
