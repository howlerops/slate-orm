//! Values computed from a row.
//!
//! The last eight ClickBench queries needed one feature wearing eight
//! disguises: `length(url)`, a timestamp's minute, `width + 1`, `CASE WHEN`, a
//! literal as a grouping key. All of them are a value computed from a row,
//! which this layer could not express.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, CmpOp, Expr, Grant, Group, Query, RecordStore, Scalar, SecurityCatalog,
    SecurityContext, SortKey, TimeUnit,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const T: TableId = TableId(1);

fn table() -> TableDef {
    TableDef::builder("events", T)
        .column("id", ValueType::U64)
        .column("url", ValueType::Str)
        .column("width", ValueType::I64)
        // Seconds since the epoch, which is how the datasets store a time.
        .column("at", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

/// Twelve rows: urls of three lengths, widths 0..12, times ten minutes apart.
fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str("x".repeat((id % 3 + 1) as usize)),
        Value::I64(id as i64),
        // 2021-01-01T00:00:00Z plus ten minutes per row.
        Value::I64(1_609_459_200 + (id as i64) * 600),
        if id.is_multiple_of(4) {
            Value::Null
        } else {
            Value::Str("note".to_owned())
        },
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let rows: Vec<Row> = (0..12).map(row).collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();
    store
}

/// Where a computed value lands, and that everything downstream can name it.
#[tokio::test]
async fn a_computed_value_is_a_column_like_any_other() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let length = Query::computed(&table, 0);
    let query = Query::all()
        .computing([Scalar::column(col("url")).length()])
        // Filter on it,
        .filter(Expr::compare(length, CmpOp::Ge, Value::I64(2)))
        // sort by it,
        .sort_by([SortKey::desc(length)]);

    let rows = txn
        .execute(&root(), &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let lengths: Vec<i64> = rows
        .iter()
        .map(|r| match r.get(length) {
            Some(Value::I64(n)) => *n,
            other => panic!("expected a length, got {other:?}"),
        })
        .collect();
    assert_eq!(lengths.len(), 8, "urls of length 2 and 3");
    assert!(lengths.windows(2).all(|w| w[0] >= w[1]), "{lengths:?}");
    assert!(lengths.iter().all(|n| *n >= 2));

    // And aggregate over it.
    let values = txn
        .aggregate(
            &root(),
            &table,
            &Query::all().computing([Scalar::column(col("url")).length()]),
            &[Aggregate::Sum(length), Aggregate::Max(length)],
        )
        .await
        .unwrap();
    // Lengths 1,2,3 repeated four times each.
    assert_eq!(values[0], Value::I64(24));
    assert_eq!(values[1], Value::I64(3));
}

/// Arithmetic, written as arithmetic.
#[tokio::test]
async fn arithmetic_computes_what_it_says() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let plus = Query::computed(&table, 0);
    let doubled = Query::computed(&table, 1);
    let values = txn
        .aggregate(
            &root(),
            &table,
            &Query::all().computing([
                Scalar::column(col("width")) + 10,
                Scalar::column(col("width")) * Scalar::literal(Value::I64(2)),
            ]),
            &[Aggregate::Sum(plus), Aggregate::Sum(doubled)],
        )
        .await
        .unwrap();

    let widths: i64 = (0..12).sum();
    assert_eq!(values[0], Value::I64(widths + 120), "each width plus ten");
    assert_eq!(values[1], Value::I64(widths * 2));
}

/// Arithmetic on a null, or that cannot be done, is null rather than an error.
/// A query over a million rows should not fail because one held a zero.
#[test]
fn arithmetic_that_cannot_be_done_is_null() {
    let one = Row::new(vec![Value::I64(7), Value::Null, Value::Str("x".into())]);
    let n = |i: usize| Scalar::column(Ordinal(i));

    assert_eq!((n(0) + 1).evaluate(&one), Value::I64(8));
    assert_eq!((n(0) + n(1)).evaluate(&one), Value::Null, "null operand");
    assert_eq!((n(0) + n(2)).evaluate(&one), Value::Null, "not a number");
    assert_eq!(
        (n(0) / Scalar::literal(Value::I64(0))).evaluate(&one),
        Value::Null,
        "division by zero"
    );
    // Overflow saturates into null rather than wrapping to a small negative.
    let big = Row::new(vec![Value::I64(i64::MAX)]);
    assert_eq!((n(0) + 1).evaluate(&big), Value::Null);
    // An integer and a real meet as a real.
    let mixed = Row::new(vec![Value::I64(3), Value::F64(0.5)]);
    assert_eq!((n(0) + n(1)).evaluate(&mixed), Value::F64(3.5));
}

/// A timestamp's parts, and rounding one down.
#[test]
fn time_units_extract_and_truncate() {
    // 2021-01-01T02:34:56Z.
    let at = 1_609_459_200 + 2 * 3600 + 34 * 60 + 56;
    let one = Row::new(vec![Value::I64(at)]);
    let t = || Scalar::column(Ordinal(0));

    assert_eq!(t().extract(TimeUnit::Second).evaluate(&one), Value::I64(56));
    assert_eq!(t().extract(TimeUnit::Minute).evaluate(&one), Value::I64(34));
    assert_eq!(t().extract(TimeUnit::Hour).evaluate(&one), Value::I64(2));

    assert_eq!(
        t().date_trunc(TimeUnit::Minute).evaluate(&one),
        Value::I64(at - 56)
    );
    assert_eq!(
        t().date_trunc(TimeUnit::Hour).evaluate(&one),
        Value::I64(at - 34 * 60 - 56)
    );
    assert_eq!(
        t().date_trunc(TimeUnit::Day).evaluate(&one),
        Value::I64(1_609_459_200)
    );

    // Before the epoch, where truncation must still round *down* rather than
    // toward zero — the difference between `/` and a floor division.
    let before = Row::new(vec![Value::I64(-30)]);
    assert_eq!(
        t().date_trunc(TimeUnit::Minute).evaluate(&before),
        Value::I64(-60)
    );
    assert_eq!(
        t().extract(TimeUnit::Second).evaluate(&before),
        Value::I64(30)
    );
}

/// `CASE WHEN`, including that an unknown condition falls through rather than
/// being taken.
#[test]
fn case_takes_the_first_branch_that_definitely_holds() {
    let expr = Scalar::Case {
        branches: vec![
            (
                Expr::eq(Ordinal(0), Value::I64(1)),
                Scalar::literal(Value::Str("one".into())),
            ),
            (
                Expr::compare(Ordinal(1), CmpOp::Gt, Value::I64(0)),
                Scalar::literal(Value::Str("positive".into())),
            ),
        ],
        otherwise: Box::new(Scalar::literal(Value::Str("neither".into()))),
    };

    let first = Row::new(vec![Value::I64(1), Value::I64(-5)]);
    assert_eq!(expr.evaluate(&first), Value::Str("one".into()));

    let second = Row::new(vec![Value::I64(2), Value::I64(5)]);
    assert_eq!(expr.evaluate(&second), Value::Str("positive".into()));

    // A null makes both conditions unknown, and unknown is not true, so
    // neither branch is taken.
    let unknown = Row::new(vec![Value::Null, Value::Null]);
    assert_eq!(expr.evaluate(&unknown), Value::Str("neither".into()));
}

/// Grouping by something computed, including a constant — which is how
/// ClickBench's `GROUP BY 1, URL` is written.
#[tokio::test]
async fn grouping_by_a_computed_value() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let length = Query::computed(&table, 0);
    let constant = Query::computed(&table, 1);
    let groups = txn
        .group_by(
            &root(),
            &table,
            &Query::all().computing([
                Scalar::column(col("url")).length(),
                Scalar::literal(Value::I64(1)),
            ]),
            &[constant, length],
            &[Aggregate::Count],
        )
        .await
        .unwrap();

    assert_eq!(groups.len(), 3, "three url lengths, one constant");
    assert!(groups.iter().all(|g| g.key[0] == Value::I64(1)));
    let counts: Vec<u64> = groups
        .iter()
        .map(|g| match g.values[0] {
            Value::U64(n) => n,
            _ => 0,
        })
        .collect();
    assert_eq!(counts, vec![4, 4, 4]);
}

/// `HAVING` filters groups by what they computed, not by what their rows held.
#[tokio::test]
async fn having_filters_groups_by_their_aggregates() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    // Group by url length; every group has four rows.
    let length = Query::computed(&table, 0);
    let query = Query::all().computing([Scalar::column(col("url")).length()]);
    let count = Group::aggregate(1, 0);

    let all = txn
        .group_by_having(
            &root(),
            &table,
            &query,
            &[length],
            &[Aggregate::Count],
            &Expr::True,
        )
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    let none = txn
        .group_by_having(
            &root(),
            &table,
            &query,
            &[length],
            &[Aggregate::Count],
            &Expr::compare(count, CmpOp::Gt, Value::U64(4)),
        )
        .await
        .unwrap();
    assert!(none.is_empty(), "no group has more than four rows");

    let some = txn
        .group_by_having(
            &root(),
            &table,
            &query,
            &[length],
            &[Aggregate::Count],
            &Expr::compare(count, CmpOp::Ge, Value::U64(4)),
        )
        .await
        .unwrap();
    assert_eq!(some.len(), 3, "all of them have exactly four");
}

/// A computed value reads columns the caller never projected, and those still
/// have to be decoded — the same hole the sort column fell through.
#[tokio::test]
async fn a_computed_value_decodes_its_inputs() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let length = Query::computed(&table, 0);
    let rows = txn
        .execute(
            &root(),
            &table,
            &Query::all()
                .computing([Scalar::column(col("url")).length()])
                // `url` is deliberately not projected.
                .select([col("id")]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), 12);
    assert!(
        rows.iter()
            .all(|r| !matches!(r.get(length), Some(Value::Null) | None)),
        "the computed value came back null, so its input was not decoded"
    );
}
