//! Decoding a row in two passes must not change what it decodes to.
//!
//! Filtering on a partially decoded row is the kind of optimisation that works
//! until a predicate reads a column the first pass skipped, at which point it
//! reads null and silently returns the wrong rows. These tests compare against
//! the same queries run with the optimisation disabled.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, Projection, Query, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext, SortKey,
};
use slate_schema::{Catalog, ColumnSet, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const T: TableId = TableId(1);
const ROWS: u64 = 300;

fn table() -> TableDef {
    TableDef::builder("wide", T)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("label", ValueType::Str)
        .column("blob", ValueType::Bytes)
        .column("ratio", ValueType::F64)
        .nullable_column("extra", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 5)),
        Value::I64((id % 50) as i64),
        if id.is_multiple_of(3) {
            Value::Null
        } else {
            Value::Str(format!("label-{id}"))
        },
        Value::Bytes(bytes::Bytes::from(vec![(id % 251) as u8; 24])),
        Value::F64(id as f64 / 7.0),
        if id.is_multiple_of(7) {
            Value::Null
        } else {
            Value::Str("extra".to_owned())
        },
    ])
}

async fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table(), &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

/// What the query should return, computed without touching the executor.
fn brute_force(filter: &Expr, projection: &Projection) -> Vec<Row> {
    let table = table();
    let wanted: Option<ColumnSet> = projection.columns().map(|columns| {
        let mut set: ColumnSet = columns.iter().copied().collect();
        // The predicate's columns have to be read to evaluate it, and the
        // primary key arrives already decoded from the key, so both are present
        // whatever the projection says.
        for column in filter.columns() {
            set.insert(column);
        }
        for column in table.primary_key() {
            set.insert(*column);
        }
        set
    });

    (0..ROWS)
        .map(row)
        .filter(|r| filter.admits(r))
        .map(|r| match &wanted {
            None => r,
            // Unprojected columns read back null, so the expectation must too.
            Some(wanted) => Row::new(
                r.values()
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        if wanted.contains(Ordinal(i)) {
                            v.clone()
                        } else {
                            Value::Null
                        }
                    })
                    .collect(),
            ),
        })
        .collect()
}

/// Across a spread of predicates and projections, a two-pass decode must agree
/// with what the rows actually contain.
#[tokio::test]
async fn partial_decoding_returns_the_same_rows() {
    let store = store().await;
    let table = table();

    let cases: Vec<(&str, Expr, Projection)> = vec![
        ("no filter, all columns", Expr::True, Projection::All),
        (
            "filter on a body column, all columns",
            Expr::eq(col("kind"), Value::Str("kind-2".into())),
            Projection::All,
        ),
        (
            "filter on a key column, all columns",
            Expr::compare(col("id"), CmpOp::Lt, Value::U64(40)),
            Projection::All,
        ),
        (
            "filter on one body column, project another",
            Expr::eq(col("kind"), Value::Str("kind-3".into())),
            Projection::Columns(vec![col("ratio")]),
        ),
        (
            "filter on a nullable column",
            Expr::is_null(col("label")),
            Projection::Columns(vec![col("id"), col("blob")]),
        ),
        (
            "filter reads a column the projection does not",
            Expr::compare(col("size"), CmpOp::Ge, Value::I64(25)),
            Projection::Columns(vec![col("id")]),
        ),
        (
            "very selective: almost everything is rejected",
            Expr::eq(col("size"), Value::I64(7))
                .and(Expr::eq(col("kind"), Value::Str("kind-2".into()))),
            Projection::All,
        ),
        (
            "rejects everything",
            Expr::eq(col("kind"), Value::Str("nope".into())),
            Projection::All,
        ),
        (
            "negation over a nullable column",
            Expr::Not(Box::new(Expr::eq(col("extra"), Value::Str("extra".into())))),
            Projection::All,
        ),
        (
            "project nothing at all",
            Expr::eq(col("kind"), Value::Str("kind-1".into())),
            Projection::none(),
        ),
    ];

    for (name, filter, projection) in cases {
        let mut query = Query::all().filter(filter.clone());
        query.projection = projection.clone();

        let txn = store.begin().await.unwrap();
        let got = txn
            .execute(&root(), &table, &query)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let want = brute_force(&filter, &projection);

        assert_eq!(
            got.len(),
            want.len(),
            "`{name}` returned {} rows, expected {}",
            got.len(),
            want.len()
        );
        for (got, want) in got.iter().zip(&want) {
            assert_eq!(got, want, "`{name}` decoded a row wrongly");
        }
    }
}

/// The columns a predicate reads must be decoded even when the caller did not
/// ask for them — otherwise the filter sees nulls and admits the wrong rows.
#[tokio::test]
async fn a_filter_column_outside_the_projection_is_still_read() {
    let store = store().await;
    let table = table();

    let mut query = Query::all().filter(Expr::eq(col("kind"), Value::Str("kind-4".into())));
    query.projection = Projection::Columns(vec![col("id")]);

    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(&root(), &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), (ROWS / 5) as usize);
    for row in &rows {
        // The filter column was needed, so it is present even though the
        // projection did not name it.
        assert_eq!(row.values()[col("kind").0], Value::Str("kind-4".into()));
        // A column neither filtered nor projected is absent.
        assert_eq!(row.values()[col("blob").0], Value::Null);
    }
}

/// Sorting happens after materialisation, so a sort key must survive it.
#[tokio::test]
async fn a_sort_key_is_materialised_before_sorting() {
    let store = store().await;
    let table = table();

    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(
            &root(),
            &table,
            &Query::all()
                .filter(Expr::eq(col("kind"), Value::Str("kind-0".into())))
                .sort_by([SortKey::desc(col("size"))])
                .limit(5),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), 5);
    let sizes: Vec<i64> = rows
        .iter()
        .map(|r| match r.values()[col("size").0] {
            Value::I64(v) => v,
            ref other => panic!("size was {other:?}"),
        })
        .collect();
    assert!(
        sizes.windows(2).all(|w| w[0] >= w[1]),
        "not sorted: {sizes:?}"
    );
}

/// Whatever the plan, the answer is the same. Scan order should not interact
/// with partial decoding at all, but it is cheap to be sure.
#[tokio::test]
async fn scan_direction_does_not_affect_decoding() {
    let store = store().await;
    let table = table();
    let filter = Expr::compare(col("size"), CmpOp::Lt, Value::I64(5));

    let txn = store.begin().await.unwrap();
    let mut forwards = txn
        .execute(
            &root(),
            &table,
            &Query::all()
                .filter(filter.clone())
                .order(ScanOrder::Ascending),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let mut backwards = txn
        .execute(
            &root(),
            &table,
            &Query::all().filter(filter).order(ScanOrder::Descending),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert!(!forwards.is_empty());
    backwards.reverse();
    forwards.sort_by_key(|r| format!("{:?}", r.values()[0]));
    backwards.sort_by_key(|r| format!("{:?}", r.values()[0]));
    assert_eq!(forwards, backwards);
}
