//! An exact decimal, and a conditional update, over the wire.
//!
//! Both existed in the kernel and reached no client. This file used to pin the
//! *absence* of the first — `value_to_proto` turned a `Value::Decimal` into a
//! `<unrepresentable decimal>` string, and the tests asserted that it did,
//! with a comment saying they should fail when the proto grew the field. It
//! did, and they did, and this is the rewrite they asked for.
//!
//! # What is actually at stake
//!
//! A decimal on this wire is a count of the column's smallest unit and nothing
//! else. `1250` in a column declared `scale = 2` is 12.50; the identical
//! integer in a `scale = 0` column is 1250. So the risk is not that the number
//! arrives corrupted — it is that the number arrives *bare*, and something
//! downstream renders it against the wrong scale, or against none. The tests
//! below are written to fail if a decimal ever comes back as an `i64`, because
//! that is the confusion that loses two decimal places silently.
//!
//! # And why `expected` is in the same file
//!
//! Not a coincidence of scheduling. A conditional update is the mechanism for
//! a read-modify-write on exactly the kind of column a decimal is — a balance,
//! a price, a running total — and the lost update it detects is the one that
//! costs money. They shipped together because they are used together.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, price, prices, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_server::convert::{value_from_proto, value_to_proto};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;

// --- the conversion, in isolation -----------------------------------------

#[test]
fn a_decimal_is_its_units_and_nothing_else() {
    let rendered = value_to_proto(&Value::Decimal(1250));
    assert_eq!(
        rendered.kind,
        Some(pb::value::Kind::DecimalValue(1250)),
        "a decimal is `decimal_value`, not a string and not a double"
    );
}

#[test]
fn a_decimal_is_not_an_integer_on_the_wire() {
    // The whole point of a field of its own. If a decimal were sent as
    // `int64_value` it would decode back as `Value::I64`, and the kernel's
    // ordering is type-first: the round-tripped value would not compare equal
    // to the stored one, and a predicate built from a row read back would
    // select nothing.
    let decimal = value_to_proto(&Value::Decimal(1250));
    let integer = value_to_proto(&Value::I64(1250));
    assert_ne!(decimal.kind, integer.kind);
    assert_eq!(value_from_proto(&decimal).unwrap(), Value::Decimal(1250));
    assert_eq!(value_from_proto(&integer).unwrap(), Value::I64(1250));
}

#[test]
fn the_extremes_survive_the_round_trip() {
    // `int64` rather than `sint64` was a deliberate choice (see the proto), so
    // the negative end is the one worth pinning: a zigzag/varint mix-up would
    // show up here and nowhere else.
    for units in [0, 1, -1, i64::MAX, i64::MIN, -1250, 999_999_999_999] {
        let back = value_from_proto(&value_to_proto(&Value::Decimal(units))).unwrap();
        assert_eq!(back, Value::Decimal(units), "units {units}");
    }
}

#[test]
fn the_marker_is_still_there_for_a_type_the_wire_cannot_carry() {
    // `value_to_proto`'s `_` arm is what caught the decimal's absence in the
    // first place. Removing it along with the gap it named would leave the
    // next added variant arriving as a null. So: the types the wire does carry
    // are unaffected, and the arm stays.
    let integer = format!("{:?}", value_to_proto(&Value::I64(1250)));
    assert!(integer.contains("Int64Value"), "{integer}");
    assert!(!integer.contains("unrepresentable"), "{integer}");
    let decimal = format!("{:?}", value_to_proto(&Value::Decimal(1250)));
    assert!(!decimal.contains("unrepresentable"), "{decimal}");
}

// --- and over a real socket -----------------------------------------------

/// Three prices: `(1, "tea", 250)`, `(2, "coffee", 1250)`, `(3, "free", 0)`.
async fn seeded() -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let txn = store.begin().await.unwrap();
        txn.insert_many(
            &context,
            &prices(),
            &[
                price(1, "tea", 250),
                price(2, "coffee", 1250),
                price(3, "free", 0),
            ],
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
    }
    serving_leader(backing).await
}

fn units(n: i64) -> pb::Value {
    pb::Value {
        kind: Some(pb::value::Kind::DecimalValue(n)),
    }
}

fn u64_value(n: u64) -> pb::Value {
    pb::Value {
        kind: Some(pb::value::Kind::Uint64Value(n)),
    }
}

fn text(s: &str) -> pb::Value {
    pb::Value {
        kind: Some(pb::value::Kind::StringValue(s.to_owned())),
    }
}

fn price_row(id: u64, label: &str, amount: i64) -> pb::Row {
    common::wire_row(vec![u64_value(id), text(label), units(amount)])
}

async fn get(client: &mut RecordsClient<Channel>, id: u64) -> Vec<pb::Value> {
    let response = client
        .get(app(pb::GetRequest {
            transaction: String::new(),
            table: "prices".to_owned(),
            primary_key: Some(common::wire_row(vec![u64_value(id)])),
            freshness: None,
            schema: Some(common::claim("prices")),
        }))
        .await
        .expect("get")
        .into_inner();
    response.row.expect("the row is there").values
}

#[tokio::test]
async fn a_decimal_column_reads_back_as_a_decimal() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let row = get(&mut client, 2).await;
    assert_eq!(
        row[2].kind,
        Some(pb::value::Kind::DecimalValue(1250)),
        "12.50 at scale 2 is 1250 units, in `decimal_value`"
    );
}

#[tokio::test]
async fn a_decimal_can_be_written_from_a_client() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "prices".to_owned(),
            rows: vec![price_row(4, "cake", -75)],
            upsert: false,
            schema: Some(common::claim("prices")),
        }))
        .await
        .expect("insert");
    // Negative on purpose: a refund is the value most likely to be mangled by
    // an encoding that assumed a price is positive.
    assert_eq!(
        get(&mut client, 4).await[2].kind,
        Some(pb::value::Kind::DecimalValue(-75))
    );
}

#[tokio::test]
async fn an_integer_in_a_decimal_column_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let status = client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "prices".to_owned(),
            // `int64_value`, not `decimal_value` — the mistake a client that
            // skipped the new field would make. Accepting it would store a
            // value that sorts in a different order from its neighbours.
            rows: vec![common::wire_row(vec![
                u64_value(5),
                text("wrong"),
                pb::Value {
                    kind: Some(pb::value::Kind::Int64Value(100)),
                },
            ])],
            upsert: false,
            schema: Some(common::claim("prices")),
        }))
        .await
        .expect_err("a type mismatch is refused");
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
}

// --- the conditional update ------------------------------------------------

fn update(rows: Vec<pb::Row>, expected: Vec<pb::Row>) -> pb::UpdateRequest {
    pb::UpdateRequest {
        transaction: String::new(),
        table: "prices".to_owned(),
        rows,
        expected,
        schema: Some(common::claim("prices")),
    }
}

#[tokio::test]
async fn an_update_naming_the_row_it_read_is_applied() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let response = client
        .update(app(update(
            vec![price_row(2, "coffee", 1400)],
            vec![price_row(2, "coffee", 1250)],
        )))
        .await
        .expect("the row is unchanged")
        .into_inner();
    assert_eq!(response.affected, 1);
    assert_eq!(
        get(&mut client, 2).await[2].kind,
        Some(pb::value::Kind::DecimalValue(1400))
    );
}

#[tokio::test]
async fn an_update_naming_a_row_that_moved_is_refused_and_writes_nothing() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    // Somebody else's write lands first.
    client
        .update(app(update(vec![price_row(2, "coffee", 1300)], Vec::new())))
        .await
        .expect("an unconditional update still works");

    let status = client
        .update(app(update(
            vec![price_row(2, "coffee", 1400)],
            // Stale: this caller read 1250 and never saw the 1300.
            vec![price_row(2, "coffee", 1250)],
        )))
        .await
        .expect_err("the row moved");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert!(status.message().contains("prices"), "{status:?}");

    // And the refusal is total: the write it guarded did not land.
    assert_eq!(
        get(&mut client, 2).await[2].kind,
        Some(pb::value::Kind::DecimalValue(1300)),
        "a refused conditional update leaves the row as it was"
    );
}

#[tokio::test]
async fn one_stale_row_refuses_the_whole_batch() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .update(app(update(vec![price_row(3, "free", 5)], Vec::new())))
        .await
        .expect("move row 3 out from under the caller");

    let status = client
        .update(app(update(
            vec![price_row(1, "tea", 300), price_row(3, "free", 99)],
            vec![price_row(1, "tea", 250), price_row(3, "free", 0)],
        )))
        .await
        .expect_err("the second row moved");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    // The first row's write is applied and rolled back with the statement:
    // this is what makes a conditional update usable for a transfer, where a
    // half-applied pair is worse than a refused one.
    assert_eq!(
        get(&mut client, 1).await[2].kind,
        Some(pb::value::Kind::DecimalValue(250)),
        "a refused row takes its batch with it"
    );
}

#[tokio::test]
async fn expected_must_name_one_row_per_update() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let status = client
        .update(app(update(
            vec![price_row(1, "tea", 300), price_row(2, "coffee", 1400)],
            vec![price_row(1, "tea", 250)],
        )))
        .await
        .expect_err("two rows and one expectation");
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    // Not a silent prefix: the alternative implementation zips and stops at
    // the shorter, which would have updated `coffee` unguarded — precisely the
    // lost update the field exists to catch.
    assert!(
        status.message().contains("2 row(s) and 1 expected"),
        "{status:?}"
    );
    assert_eq!(
        get(&mut client, 2).await[2].kind,
        Some(pb::value::Kind::DecimalValue(1250))
    );
}

#[tokio::test]
async fn an_expectation_for_a_different_row_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let status = client
        .update(app(update(
            vec![price_row(1, "tea", 300)],
            // Row 2's key, guarding a write to row 1: "checked one row, wrote
            // another". The kernel refuses this before it reads anything.
            vec![price_row(2, "coffee", 1250)],
        )))
        .await
        .expect_err("the expectation names another row");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert_eq!(
        get(&mut client, 1).await[2].kind,
        Some(pb::value::Kind::DecimalValue(250))
    );
}

#[tokio::test]
async fn an_empty_expected_is_an_ordinary_update() {
    // The field is `repeated`, so "unset" and "empty" are the same thing on
    // the wire and there is nowhere to put a third state. This pins that an
    // old client — which sends no `expected` at all — keeps the behaviour it
    // had.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let response = client
        .update(app(update(vec![price_row(1, "tea", 275)], Vec::new())))
        .await
        .expect("no expectation, no condition")
        .into_inner();
    assert_eq!(response.affected, 1);
    assert_eq!(
        get(&mut client, 1).await[2].kind,
        Some(pb::value::Kind::DecimalValue(275))
    );
}

#[tokio::test]
async fn a_conditional_update_works_inside_a_transaction() {
    // The autocommit path and the session path are different code — `apply`
    // and the actor's `Command::Update` arm — and a feature wired into one and
    // not the other is this repository's recurring shape.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let begun = client
        .begin(app(pb::BeginRequest::default()))
        .await
        .expect("begin")
        .into_inner();

    client
        .update(app(pb::UpdateRequest {
            transaction: begun.transaction.clone(),
            ..update(
                vec![price_row(2, "coffee", 1500)],
                vec![price_row(2, "coffee", 1250)],
            )
        }))
        .await
        .expect("unchanged inside the transaction too");

    let status = client
        .update(app(pb::UpdateRequest {
            transaction: begun.transaction.clone(),
            ..update(
                vec![price_row(2, "coffee", 1600)],
                // 1250 was true before this transaction's own write, and is
                // not true now: a transaction sees its own writes, so the
                // check is against what the transaction would read.
                vec![price_row(2, "coffee", 1250)],
            )
        }))
        .await
        .expect_err("the transaction's own write moved the row");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");

    client
        .rollback(app(pb::RollbackRequest {
            transaction: begun.transaction,
        }))
        .await
        .expect("rollback");
    assert_eq!(
        get(&mut client, 2).await[2].kind,
        Some(pb::value::Kind::DecimalValue(1250))
    );
}

#[tokio::test]
async fn a_stale_first_row_refuses_the_rest_inside_a_transaction() {
    // Written because a mutation survived without it. The session actor loops
    // over the rows and replies with an outcome; removing its `break` left
    // every test green, because the only multi-row case was on the autocommit
    // path and the only transaction case had one row. Without the break the
    // *last* row's outcome is what the caller hears, so a stale first row and
    // a fine second row report success — which is the lost update the whole
    // field exists to report.
    let serving = seeded().await;
    let mut client = serving.client().await;
    client
        .update(app(update(vec![price_row(1, "tea", 251)], Vec::new())))
        .await
        .expect("somebody else moves row 1");

    let begun = client
        .begin(app(pb::BeginRequest::default()))
        .await
        .expect("begin")
        .into_inner();
    let status = client
        .update(app(pb::UpdateRequest {
            transaction: begun.transaction.clone(),
            ..update(
                vec![price_row(1, "tea", 300), price_row(2, "coffee", 1400)],
                // Stale, then current: the order matters, and it is the order
                // a loop that does not stop gets wrong.
                vec![price_row(1, "tea", 250), price_row(2, "coffee", 1250)],
            )
        }))
        .await
        .expect_err("the first row moved");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");

    client
        .rollback(app(pb::RollbackRequest {
            transaction: begun.transaction,
        }))
        .await
        .expect("rollback");
    assert_eq!(
        get(&mut client, 2).await[2].kind,
        Some(pb::value::Kind::DecimalValue(1250)),
        "the second row is not written by a refused statement"
    );
}

#[tokio::test]
async fn a_conditional_update_works_inside_a_batch() {
    // The third path: `Decoded::Update` and `Write::apply`.
    let serving = seeded().await;
    let mut client = serving.client().await;
    let response = client
        .batch(app(pb::BatchRequest {
            transaction: String::new(),
            atomicity: pb::Atomicity::AllOrNothing as i32,
            operations: vec![pb::BatchOperation {
                of: Some(pb::batch_operation::Of::Update(update(
                    vec![price_row(1, "tea", 260)],
                    vec![price_row(1, "tea", 250)],
                ))),
            }],
        }))
        .await
        .expect("batch")
        .into_inner();
    assert!(response.results.is_empty(), "all-or-nothing reports none");
    assert_eq!(
        get(&mut client, 1).await[2].kind,
        Some(pb::value::Kind::DecimalValue(260))
    );

    let status = client
        .batch(app(pb::BatchRequest {
            transaction: String::new(),
            atomicity: pb::Atomicity::AllOrNothing as i32,
            operations: vec![pb::BatchOperation {
                of: Some(pb::batch_operation::Of::Update(update(
                    vec![price_row(1, "tea", 999)],
                    vec![price_row(1, "tea", 250)],
                ))),
            }],
        }))
        .await
        .expect_err("stale inside a batch too");
    assert_eq!(status.code(), Code::Aborted, "{status:?}");
    assert_eq!(
        get(&mut client, 1).await[2].kind,
        Some(pb::value::Kind::DecimalValue(260))
    );
}
