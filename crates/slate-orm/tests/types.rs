//! Timestamps and enums: mapping Rust types onto a closed value model.
//!
//! Neither adds a value type. A timestamp is an integer of seconds and an enum
//! is its variant name in a string column, which is what makes both cheap —
//! the ordering, the encoding and the index behaviour are the ones already
//! proven for `I64` and `Str`.
//!
//! What they add is in Rust: a compiler that knows a count of seconds is not an
//! id, and a column that cannot hold a string no variant claims.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_orm::{
    Action, CalendarPart, Enum, Expr, Field, FieldError, Grant, Query, Record, RecordStore, Records,
    Scalar, SecurityCatalog, SecurityContext, Timestamp, Value, ValueType,
};
use slate_schema::{Catalog, TableId};

const RIDES: TableId = TableId(1);

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
enum Payment {
    Cash,
    #[record(rename = "credit card")]
    CreditCard,
    Wallet,
}

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "rides", id = 1)]
struct Ride {
    #[record(pk)]
    id: u64,
    #[record(index(name = "by_started", id = 10))]
    started: Timestamp,
    payment: Payment,
}

fn store() -> (RecordStore<MemoryStore>, SecurityContext) {
    let catalog = Catalog::from_tables([Ride::table().clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", RIDES, Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    (store, SecurityContext::superuser())
}

// --- the column types ------------------------------------------------------

#[test]
fn a_timestamp_is_an_integer_column_and_an_enum_is_a_string_one() {
    let table = Ride::table();
    assert_eq!(
        table.column(Ride::COLUMNS.started).unwrap().value_type(),
        ValueType::I64
    );
    assert_eq!(
        table.column(Ride::COLUMNS.payment).unwrap().value_type(),
        ValueType::Str
    );
}

// --- timestamps ------------------------------------------------------------

#[tokio::test]
async fn timestamps_round_trip_and_order_chronologically() {
    let (store, ctx) = store();
    let txn = store.begin().await.unwrap();
    // Spanning the epoch, because a negative instant is where an unsigned
    // column or an unchecked cast would go wrong and nothing else would.
    let instants = [-86_400i64, -1, 0, 1, 1_700_000_000];
    for (id, seconds) in instants.iter().enumerate() {
        txn.insert_record(
            &ctx,
            &Ride {
                id: id as u64,
                started: Timestamp::from_unix_seconds(*seconds),
                payment: Payment::Cash,
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let rides: Vec<Ride> = txn
        .query_records(
            &ctx,
            &Query::all().sort_by([slate_orm::SortKey::asc(Ride::COLUMNS.started)]),
        )
        .await
        .unwrap();
    txn.rollback();

    let got: Vec<i64> = rides.iter().map(|r| r.started.seconds()).collect();
    let mut want = instants.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "chronological order is integer order");
}

#[tokio::test]
async fn a_timestamp_answers_calendar_questions_as_a_computed_column() {
    let (store, ctx) = store();
    let txn = store.begin().await.unwrap();
    // 2023-11-14T22:13:20Z, which is 17:13 on the 14th in New York (EST) — so
    // the zone changes the *day of month* only at the edges, and the hour
    // always. Both are checked.
    txn.insert_record(
        &ctx,
        &Ride {
            id: 1,
            started: Timestamp::from_unix_seconds(1_700_000_000),
            payment: Payment::Wallet,
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let table = Ride::table();
    let query = Query::all().computing([
        Scalar::column(Ride::COLUMNS.started).calendar_part(CalendarPart::Year),
        Scalar::column(Ride::COLUMNS.started).calendar_part(CalendarPart::Month),
        Scalar::column(Ride::COLUMNS.started)
            .in_zone("America/New_York")
            .calendar_part(CalendarPart::DayOfMonth),
    ]);

    let txn = store.begin().await.unwrap();
    let mut cursor = txn.execute(&ctx, table, &query).await.unwrap();
    let row = cursor.next().await.unwrap().expect("one row");
    drop(cursor);
    txn.rollback();

    let first = Query::computed(table, 0).0;
    assert_eq!(row.values()[first], Value::I64(2023), "{row:?}");
    assert_eq!(row.values()[first + 1], Value::I64(11), "{row:?}");
    assert_eq!(row.values()[first + 2], Value::I64(14), "{row:?}");
}

/// `Option<Timestamp>` makes the column nullable, by the same mechanism that
/// makes `Option<String>` nullable.
///
/// Worth a test rather than an argument: the mechanism is `Field for Option<T>`
/// deriving `NULLABLE` from the wrapper, and a newtype that got that wrong
/// would declare a non-nullable column and fail on the first missing value.
#[tokio::test]
async fn an_optional_timestamp_is_a_nullable_column_and_round_trips_a_none() {
    #[derive(Record, Debug, Clone, PartialEq)]
    #[record(table = "drafts", id = 2)]
    struct Draft {
        #[record(pk)]
        id: u64,
        published: Option<Timestamp>,
    }

    let table = Draft::table();
    assert!(
        table
            .column(Draft::COLUMNS.published)
            .unwrap()
            .is_nullable()
    );

    let catalog = Catalog::from_tables([table.clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", TableId(2), Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let ctx = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    for draft in [
        Draft {
            id: 1,
            published: None,
        },
        Draft {
            id: 2,
            published: Some(Timestamp::from_unix_seconds(-1)),
        },
    ] {
        txn.insert_record(&ctx, &draft).await.unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let back: Vec<Draft> = txn.query_records(&ctx, &Query::all()).await.unwrap();
    txn.rollback();
    assert_eq!(back[0].published, None);
    // -1, not None: a null and "one second before the epoch" are different, and
    // a mapping that folded either into the other would pass a test using only
    // positive instants.
    assert_eq!(back[1].published, Some(Timestamp::from_unix_seconds(-1)));
}

#[test]
fn a_timestamp_refuses_a_value_of_another_type() {
    let error = Timestamp::from_value(&Value::Str("yesterday".into())).unwrap_err();
    assert!(
        matches!(error, FieldError::TypeMismatch { .. }),
        "{error:?}"
    );
    let error = Timestamp::from_value(&Value::Null).unwrap_err();
    assert!(
        matches!(error, FieldError::UnexpectedNull { .. }),
        "{error:?}"
    );
}

// --- enums -----------------------------------------------------------------

#[test]
fn an_enum_stores_its_name_and_rename_changes_which_name() {
    assert_eq!(Payment::Cash.to_value(), Value::Str("Cash".into()));
    // Not "CreditCard": the attribute is what lets the Rust spelling and the
    // stored spelling move independently.
    assert_eq!(
        Payment::CreditCard.to_value(),
        Value::Str("credit card".into())
    );
}

#[test]
fn an_enum_reads_back_every_variant_it_writes() {
    for variant in [Payment::Cash, Payment::CreditCard, Payment::Wallet] {
        assert_eq!(Payment::from_value(&variant.to_value()).unwrap(), variant);
    }
}

/// A name no variant claims is an error, not a default.
///
/// This is the case a row written by a newer build — one variant ahead — lands
/// in, and the failure mode being guarded against is not a panic: it is
/// quietly becoming the first variant, which writes back and looks correct.
#[test]
fn an_unknown_variant_name_is_an_error_and_not_the_first_variant() {
    let error = Payment::from_value(&Value::Str("crypto".into())).unwrap_err();
    match error {
        FieldError::OutOfRange { target } => assert_eq!(target, "Payment"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_enum_round_trips_through_the_store_and_filters_by_name() {
    let (store, ctx) = store();
    let txn = store.begin().await.unwrap();
    for (id, payment) in [Payment::Cash, Payment::CreditCard, Payment::CreditCard]
        .into_iter()
        .enumerate()
    {
        txn.insert_record(
            &ctx,
            &Ride {
                id: id as u64,
                started: Timestamp::EPOCH,
                payment,
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    // Filtering names the *stored* spelling, which is the renamed one. A
    // predicate written as "CreditCard" would match nothing, and that is worth
    // a test rather than a warning.
    let paid: Vec<Ride> = txn
        .query_records(
            &ctx,
            &Query::all().filter(Expr::eq(
                Ride::COLUMNS.payment,
                Value::Str("credit card".into()),
            )),
        )
        .await
        .unwrap();
    assert_eq!(paid.len(), 2, "{paid:?}");
    assert!(paid.iter().all(|r| r.payment == Payment::CreditCard));

    let none: Vec<Ride> = txn
        .query_records(
            &ctx,
            &Query::all().filter(Expr::eq(
                Ride::COLUMNS.payment,
                Value::Str("CreditCard".into()),
            )),
        )
        .await
        .unwrap();
    txn.rollback();
    assert!(none.is_empty(), "the Rust spelling is not the stored one");
}
