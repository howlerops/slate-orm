//! Adversarial probes on the expression-index covering path.
//!
//! `oracle.rs` generates seven compute lists and the queries around them. This
//! file is the shapes that generator cannot draw, enumerated rather than
//! sampled because there are few enough of them to write down and each one is
//! there for a reason:
//!
//! - **the same expression twice.** `expression_position` answers with the
//!   *first* matching position, so a list holding one expression at two
//!   positions asks whether the planner's "this position comes out of the
//!   entry" and the executor's are the same position — and what the *other*
//!   copy is credited with, since its inputs are exactly the columns the entry
//!   does not carry.
//! - **a chain three long**, and a chain whose last link also reaches for a
//!   table column, which must stop the chain rather than ride on it.
//! - **a forward reference and an ordinal naming no computed value at all**,
//!   which `covers` treats differently from each other and which must read the
//!   same on every access path either way.
//! - **a projection that names a computed value's own input**, which takes the
//!   column out of `transient` and so changes what a row contains — on both
//!   paths, or on neither.
//! - **a sort over a computed value**, which is the one thing that makes the
//!   cursor materialise and re-enter `extend` from a different direction.
//!
//! The property is the differential the whole project rests on: the same query
//! down every access path returns the same rows, in the same order once the
//! primary key is appended to the sort. Nothing is assumed about what the right
//! answer is.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, Projection, Query, RecordStore, Scalar, SecurityCatalog,
    SecurityContext, SortKey, Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};

const ITEMS: TableId = TableId(1);
const ROWS: u64 = 60;

const BY_KIND: IndexId = IndexId(10);
const BY_KIND_SIZE: IndexId = IndexId(11);
const BY_LOWER_LABEL: IndexId = IndexId(12);
const BY_LABEL_LENGTH: IndexId = IndexId(13);

const ID: Ordinal = Ordinal(0);
const KIND: Ordinal = Ordinal(1);
const SIZE: Ordinal = Ordinal(2);
const LABEL: Ordinal = Ordinal(3);
const WIDTH: usize = 4;

const fn computed(n: usize) -> Ordinal {
    Ordinal(WIDTH + n)
}

fn lower_label() -> Scalar {
    Scalar::Lower(Box::new(Scalar::Column(LABEL)))
}

fn label_length() -> Scalar {
    Scalar::Length(Box::new(Scalar::Column(LABEL)))
}

fn items() -> TableDef {
    TableDef::builder("items", ITEMS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("label", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", BY_KIND).column("kind"))
        .index(
            IndexDef::builder("by_kind_size", BY_KIND_SIZE)
                .column("kind")
                .column("size"),
        )
        .index(
            IndexDef::builder("by_lower_label", BY_LOWER_LABEL)
                .expression(lower_label(), ValueType::Str),
        )
        // Descending, because a bound for a descending expression key is built
        // by code the ascending case never runs.
        .index(
            IndexDef::builder("by_label_length", BY_LABEL_LENGTH).expression_with(
                label_length(),
                ValueType::I64,
                Direction::Desc,
            ),
        )
        .build()
        .expect("valid schema")
}

#[test]
fn the_ordinals_are_where_they_are_claimed() {
    let table = items();
    assert_eq!(ID, table.ordinal_of("id").unwrap());
    assert_eq!(KIND, table.ordinal_of("kind").unwrap());
    assert_eq!(SIZE, table.ordinal_of("size").unwrap());
    assert_eq!(LABEL, table.ordinal_of("label").unwrap());
    assert_eq!(WIDTH, table.columns().len());
}

/// `label` is null a quarter of the time, spelled in two cases so `lower` is
/// not the column back again, and comes in two lengths so `length` is not one
/// value for the whole table.
fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 4)),
        Value::I64((id % 11) as i64 - 5),
        if id.is_multiple_of(4) {
            Value::Null
        } else if id.is_multiple_of(2) {
            Value::Str(format!("Label-{}", id % 7))
        } else {
            Value::Str(format!("lbl{}", id % 7))
        },
    ])
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([items()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", ITEMS, Action::ALL));
    let mut store = RecordStore::new(MemoryStore::new(), catalog, security);
    let table = items();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..ROWS {
        txn.insert(&root, &table, &row(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let stats = txn.analyze(&root, &table).await.unwrap();
    txn.commit().await.unwrap();
    let mut statistics = Statistics::new();
    statistics.set(ITEMS, stats);
    store.set_statistics(statistics);
    store
}

/// The compute lists, each named by what it is there to attack.
fn compute_lists() -> Vec<(&'static str, Vec<Scalar>)> {
    vec![
        ("the index's own value", vec![lower_label()]),
        (
            // Two positions holding the identical expression. Only one can be
            // the one the entry supplies.
            "the same expression twice",
            vec![lower_label(), lower_label()],
        ),
        (
            // The entry's value second, behind one an ordinary index can also
            // produce.
            "the index's value second",
            vec![Scalar::Upper(Box::new(Scalar::Column(KIND))), lower_label()],
        ),
        (
            // A chain three long over the entry's value.
            "a chain of three",
            vec![
                lower_label(),
                Scalar::Length(Box::new(Scalar::Column(computed(0)))),
                Scalar::Add(
                    Box::new(Scalar::Column(computed(1))),
                    Box::new(Scalar::Literal(Value::I64(1))),
                ),
            ],
        ),
        (
            // The chain's last link also reaches for a table column the entry
            // does not carry, which must stop it rather than ride on the link
            // before it.
            "a chain that reaches back to a column",
            vec![
                lower_label(),
                Scalar::Concat(vec![
                    Scalar::Column(computed(0)),
                    Scalar::Column(LABEL),
                ]),
            ],
        ),
        (
            // A forward reference: position 0 naming position 1.
            "a forward reference",
            vec![
                Scalar::Length(Box::new(Scalar::Column(computed(1)))),
                lower_label(),
            ],
        ),
        (
            // An ordinal past the end of the compute list, naming nothing.
            "an ordinal naming no computed value",
            vec![
                lower_label(),
                Scalar::Length(Box::new(Scalar::Column(Ordinal(WIDTH + 9)))),
            ],
        ),
        (
            // Both expression indexes' values at once, so at most one can come
            // out of an entry.
            "both expressions at once",
            vec![lower_label(), label_length()],
        ),
        (
            // The descending expression index's value, and a chain on it.
            "the descending expression, chained",
            vec![
                label_length(),
                Scalar::Mul(
                    Box::new(Scalar::Column(computed(0))),
                    Box::new(Scalar::Literal(Value::I64(2))),
                ),
            ],
        ),
    ]
}

/// Projections, including one that names a computed value's own *input* —
/// which takes `label` out of the transient set and so changes what a row
/// carries on every path or on none.
fn projections() -> Vec<(&'static str, Projection)> {
    vec![
        ("all", Projection::All),
        ("id only", Projection::Columns(vec![ID])),
        (
            "id and the first computed value",
            Projection::Columns(vec![ID, computed(0)]),
        ),
        (
            "id and the computed value's own input",
            Projection::Columns(vec![ID, LABEL, computed(0)]),
        ),
        ("kind and size", Projection::Columns(vec![KIND, SIZE])),
    ]
}

fn filters() -> Vec<(&'static str, Expr)> {
    vec![
        ("no filter", Expr::True),
        (
            "on the entry's value",
            Expr::compare(computed(0), CmpOp::Ge, Value::Str("l".to_owned())),
        ),
        (
            "on a table column",
            Expr::compare(KIND, CmpOp::Eq, Value::Str("kind-1".to_owned())),
        ),
        (
            "on a chained computed value",
            Expr::compare(computed(1), CmpOp::Le, Value::I64(6)),
        ),
    ]
}

/// Every access path, forced. Hinting an index the planner cannot use falls
/// back to a scan rather than failing, which is still a plan that has to agree.
fn paths() -> Vec<(&'static str, Box<dyn Fn(Query) -> Query>)> {
    vec![
        ("planner's choice", Box::new(|q: Query| q)),
        ("table scan", Box::new(Query::using_table_scan)),
        ("by_kind", Box::new(|q: Query| q.using_index(BY_KIND))),
        (
            "by_kind_size",
            Box::new(|q: Query| q.using_index(BY_KIND_SIZE)),
        ),
        (
            "by_lower_label",
            Box::new(|q: Query| q.using_index(BY_LOWER_LABEL)),
        ),
        (
            "by_label_length",
            Box::new(|q: Query| q.using_index(BY_LABEL_LENGTH)),
        ),
    ]
}

/// Every access path returns the same rows, for every compute list, projection
/// and filter above.
///
/// Sorted on the primary key so there is one right order and the comparison is
/// of rows rather than of the order an index happened to produce.
#[tokio::test]
async fn every_access_path_agrees_on_every_compute_list() {
    let store = seeded().await;
    let table = items();
    let root = SecurityContext::superuser();
    let mut cases = 0usize;

    for (compute_name, compute) in compute_lists() {
        for (projection_name, projection) in projections() {
            for (filter_name, filter) in filters() {
                let base = Query {
                    filter: filter.clone(),
                    projection: projection.clone(),
                    sort: vec![SortKey::asc(ID)],
                    ..Query::all()
                }
                .computing(compute.clone());

                let mut expected: Option<(&str, Vec<Row>)> = None;
                for (path_name, force) in paths() {
                    let txn = store.begin().await.unwrap();
                    let rows = txn
                        .execute(&root, &table, &force(base.clone()))
                        .await
                        .unwrap()
                        .collect()
                        .await
                        .unwrap();
                    match &expected {
                        None => expected = Some((path_name, rows)),
                        Some((first_name, first)) => assert_eq!(
                            &rows, first,
                            "{path_name} disagreed with {first_name} for \
                             compute=[{compute_name}] projection=[{projection_name}] \
                             filter=[{filter_name}]"
                        ),
                    }
                }
                cases += 1;
            }
        }
    }

    assert_eq!(cases, 9 * 5 * 4);
}

/// The same, with the sort over a *computed* value rather than the key.
///
/// A sort makes the cursor materialise everything and hand it back through
/// `Source::Sorted`, which is the one arm of `next_admitted` that must not
/// extend a row a second time. The key is appended so ties have one answer.
#[tokio::test]
async fn every_access_path_agrees_when_the_sort_is_over_a_computed_value() {
    let store = seeded().await;
    let table = items();
    let root = SecurityContext::superuser();

    for (compute_name, compute) in compute_lists() {
        for descending in [false, true] {
            for window in [None, Some((3usize, 2usize))] {
                let key = if descending {
                    SortKey::desc(computed(0))
                } else {
                    SortKey::asc(computed(0))
                };
                let mut base = Query {
                    projection: Projection::Columns(vec![ID, computed(0)]),
                    sort: vec![key, SortKey::asc(ID)],
                    ..Query::all()
                }
                .computing(compute.clone());
                if let Some((limit, offset)) = window {
                    base.limit = Some(limit);
                    base.offset = offset;
                }

                let mut expected: Option<(&str, Vec<Row>)> = None;
                for (path_name, force) in paths() {
                    let txn = store.begin().await.unwrap();
                    let rows = txn
                        .execute(&root, &table, &force(base.clone()))
                        .await
                        .unwrap()
                        .collect()
                        .await
                        .unwrap();
                    match &expected {
                        None => expected = Some((path_name, rows)),
                        Some((first_name, first)) => assert_eq!(
                            &rows, first,
                            "{path_name} disagreed with {first_name} sorting \
                             [{compute_name}] descending={descending} window={window:?}"
                        ),
                    }
                }
            }
        }
    }
}

/// The corpus has to reach the path it is about.
///
/// Without this the differential above could pass because every one of its
/// queries planned as a table scan, and it would be asserting that a table scan
/// agrees with itself.
#[tokio::test]
async fn the_corpus_reaches_a_covering_scan_of_an_expression_index() {
    let store = seeded().await;
    let table = items();
    let root = SecurityContext::superuser();
    let mut covering = 0usize;
    let mut total = 0usize;

    for (_, compute) in compute_lists() {
        for (_, projection) in projections() {
            for (_, filter) in filters() {
                for index in [BY_LOWER_LABEL, BY_LABEL_LENGTH] {
                    let query = Query {
                        filter: filter.clone(),
                        projection: projection.clone(),
                        sort: vec![SortKey::asc(ID)],
                        ..Query::all()
                    }
                    .computing(compute.clone())
                    .using_index(index);
                    let txn = store.begin().await.unwrap();
                    let explained = txn.explain(&root, &table, &query).unwrap().to_string();
                    total += 1;
                    if explained.contains("Index Only") {
                        covering += 1;
                    }
                }
            }
        }
    }

    // Measured at 24 of 360 on this fixture. The bar is well under it, so this
    // guards the corpus rather than restating the cost model.
    assert!(
        covering >= 10,
        "only {covering} of {total} hinted plans was an index-only scan of an \
         expression index, which is too few for the differential above to be \
         about the covering path"
    );
}

