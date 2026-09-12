//! Vectors, and nearest-neighbour search over them.
//!
//! There is no vector index. A k-NN search here computes every row's distance
//! and keeps the best `k`, which is exactly what pgvector does before an
//! `ivfflat` or `hnsw` index is built. What makes it bearable is the bounded
//! top-N sort: searching a million embeddings for the nearest ten holds ten of
//! them, not a million.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Expr, Grant, Metric, Query, RecordStore, Scalar, SecurityCatalog, SecurityContext,
    SortKey,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, SchemaError, TableDef, TableId};
use slate_tuple::{Value, ValueType, decode, encode};

const T: TableId = TableId(1);
const ROWS: u64 = 500;
const DIMS: usize = 8;

fn table() -> TableDef {
    TableDef::builder("documents", T)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .column("embedding", ValueType::Vector)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

/// Points spread along a line through the space, so the nearest neighbours of
/// any query are known in advance.
fn embedding(id: u64) -> Vec<f32> {
    let t = id as f32 / ROWS as f32;
    (0..DIMS).map(|d| t + d as f32).collect()
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("doc-{id}")),
        Value::Vector(embedding(id)),
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let rows: Vec<Row> = (0..ROWS).map(row).collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows).await.unwrap();
    txn.commit().await.unwrap();
    store
}

/// A vector survives the codec, and the encoding orders as the values do —
/// the invariant the whole keyspace rests on, which a new type must not break.
#[test]
fn vectors_round_trip_and_encode_in_order() {
    let cases = vec![
        Value::Vector(Vec::new()),
        Value::Vector(vec![0.0]),
        Value::Vector(vec![-0.0]),
        Value::Vector(vec![1.5, -2.5, 0.0]),
        Value::Vector(vec![f32::MIN, f32::MAX]),
        Value::Vector(vec![f32::NEG_INFINITY, f32::INFINITY]),
        Value::Vector(vec![f32::NAN]),
        Value::Vector(vec![1.0, 2.0, 3.0, 4.0]),
    ];

    for value in &cases {
        let bytes = encode(core::slice::from_ref(value));
        let back = decode(&bytes, &[ValueType::Vector]).expect("decodes");
        assert_eq!(back.len(), 1);
        assert_eq!(&back[0], value, "{value:?} did not round trip");
    }

    // Byte order equals value order, which is what makes the codec sortable.
    let mut sorted = cases.clone();
    sorted.sort();
    let mut by_bytes = cases;
    by_bytes.sort_by_key(|v| encode(core::slice::from_ref(v)));
    assert_eq!(sorted, by_bytes, "encoding disagreed with ordering");
}

/// A vector alongside other types still sorts into its own class, so mixing
/// one into a tuple cannot reorder anything else.
#[test]
fn a_vector_sorts_after_every_scalar() {
    let mut values = [
        Value::Vector(vec![0.0]),
        Value::Null,
        Value::I64(9),
        Value::Str("z".to_owned()),
        Value::F64(1.0),
    ];
    values.sort();
    assert_eq!(values.last(), Some(&Value::Vector(vec![0.0])));
    assert_eq!(values.first(), Some(&Value::Null));
}

/// A vector's order is not its similarity, so it cannot be a key. Refused when
/// the table is defined, not when a query turns out to be meaningless.
#[test]
fn a_vector_cannot_be_a_key_or_an_index() {
    let as_key = TableDef::builder("bad", TableId(9))
        .column("embedding", ValueType::Vector)
        .primary_key(["embedding"])
        .build();
    assert!(
        matches!(as_key, Err(SchemaError::VectorInKey { .. })),
        "got {as_key:?}"
    );

    let as_index = TableDef::builder("bad", TableId(9))
        .column("id", ValueType::U64)
        .column("embedding", ValueType::Vector)
        .primary_key(["id"])
        .index(IndexDef::builder("by_embedding", IndexId(10)).column("embedding"))
        .build();
    assert!(
        matches!(as_index, Err(SchemaError::VectorInKey { .. })),
        "got {as_index:?}"
    );

    // But an ordinary column of one is fine.
    assert!(table().column(col("embedding")).is_some());
}

/// Each metric measures what it says it does.
#[test]
fn the_metrics_measure_what_they_claim() {
    let a = [3.0f32, 4.0];
    let same = [3.0f32, 4.0];
    let orthogonal = [-4.0f32, 3.0];
    let opposite = [-3.0f32, -4.0];

    // Distance to itself is zero, for every metric that says so.
    assert_eq!(Metric::L2.between(&a, &same), Some(0.0));
    assert_eq!(Metric::L2Squared.between(&a, &same), Some(0.0));
    assert!(Metric::Cosine.between(&a, &same).unwrap().abs() < 1e-9);

    // L2 is the straight-line distance; squared is its square, and they rank
    // the same way, which is why the squared one is the right default.
    let far = [0.0f32, 0.0];
    assert_eq!(Metric::L2.between(&a, &far), Some(5.0));
    assert_eq!(Metric::L2Squared.between(&a, &far), Some(25.0));

    // Cosine is about direction: orthogonal is one, opposite is two.
    let cos = |x: &[f32; 2]| Metric::Cosine.between(&a, x).unwrap();
    assert!((cos(&orthogonal) - 1.0).abs() < 1e-9);
    assert!((cos(&opposite) - 2.0).abs() < 1e-9);
    // And magnitude does not matter to it.
    assert!(cos(&[30.0, 40.0]).abs() < 1e-9);

    // Inner product is negated so that, like the others, smaller is nearer.
    assert_eq!(Metric::NegativeInnerProduct.between(&a, &same), Some(-25.0));
    assert!(
        Metric::NegativeInnerProduct.between(&a, &opposite).unwrap()
            > Metric::NegativeInnerProduct.between(&a, &same).unwrap()
    );

    // Mismatched dimensions are not a distance, and a zero vector has no
    // direction so its cosine is undefined rather than zero.
    assert_eq!(Metric::L2.between(&a, &[1.0]), None);
    assert_eq!(Metric::Cosine.between(&a, &[0.0, 0.0]), None);
}

/// The point of all of it: nearest-neighbour search as an ordinary query.
#[tokio::test]
async fn nearest_neighbours_are_ordered_by_distance() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    // Query near the embedding of row 100.
    let query = embedding(100);
    let distance = Query::computed(&table, 0);

    for metric in [Metric::L2, Metric::L2Squared, Metric::NegativeInnerProduct] {
        let rows = txn
            .execute(
                &root(),
                &table,
                &Query::all()
                    .computing([Scalar::column(col("embedding")).distance(query.clone(), metric)])
                    .sort_by([SortKey::asc(distance)])
                    .limit(5),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();

        assert_eq!(rows.len(), 5, "{metric:?}");
        let ids: Vec<u64> = rows
            .iter()
            .map(|r| match r.get(col("id")) {
                Some(Value::U64(n)) => *n,
                other => panic!("expected an id, got {other:?}"),
            })
            .collect();

        // Distances must be non-decreasing, whatever the metric.
        let distances: Vec<f64> = rows
            .iter()
            .map(|r| match r.get(distance) {
                Some(Value::F64(d)) => *d,
                other => panic!("expected a distance, got {other:?}"),
            })
            .collect();
        assert!(
            distances.windows(2).all(|w| w[0] <= w[1]),
            "{metric:?}: {distances:?}"
        );

        // For the distance metrics the answer is known: the points sit on a
        // line, so the nearest to row 100 is row 100 itself.
        if matches!(metric, Metric::L2 | Metric::L2Squared) {
            assert_eq!(ids[0], 100, "{metric:?}");
            assert!(distances[0].abs() < 1e-9);
            // And its neighbours are the adjacent rows.
            let mut nearby = ids[1..].to_vec();
            nearby.sort_unstable();
            assert_eq!(nearby, vec![98, 99, 101, 102], "{metric:?}");
        }
    }
}

/// A k-NN search returns the same rows a full sort would, which is what makes
/// the bounded heap an optimisation rather than an approximation.
///
/// The rows sit on a line, so every distance but the query point's own is a tie
/// between the neighbour on either side. `ORDER BY distance LIMIT k` is then
/// genuinely ambiguous, and the heap is free to break those ties differently
/// from a full sort — so the id is a second sort key, which makes the two
/// answers comparable without weakening what is being tested.
#[tokio::test]
async fn a_bounded_search_agrees_with_an_unbounded_one() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();
    let distance = Query::computed(&table, 0);
    let query = embedding(250);

    let search = |limit: Option<usize>| {
        let mut q = Query::all()
            .computing(
                [Scalar::column(col("embedding")).distance(query.clone(), Metric::L2Squared)],
            )
            .sort_by([SortKey::asc(distance), SortKey::asc(col("id"))]);
        if let Some(limit) = limit {
            q = q.limit(limit);
        }
        q
    };

    let bounded = txn
        .execute(&root(), &table, &search(Some(10)))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let unbounded = txn
        .execute(&root(), &table, &search(None))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(bounded.len(), 10);
    assert_eq!(unbounded.len(), ROWS as usize);
    let ids = |rows: &[Row]| -> Vec<u64> {
        rows.iter()
            .filter_map(|r| match r.get(col("id")) {
                Some(Value::U64(n)) => Some(*n),
                _ => None,
            })
            .collect()
    };
    assert_eq!(ids(&bounded), ids(&unbounded)[..10], "the heap lost a row");
}

/// Search restricted by an ordinary predicate: the filter runs first, so the
/// neighbours come from the rows the caller is allowed and asked for.
#[tokio::test]
async fn a_search_can_be_filtered() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();
    let distance = Query::computed(&table, 0);

    let rows = txn
        .execute(
            &root(),
            &table,
            &Query::all()
                .computing([
                    Scalar::column(col("embedding")).distance(embedding(100), Metric::L2Squared)
                ])
                // Only even ids, expressed as an ordinary filter.
                .filter(Expr::compare(
                    col("id"),
                    slate_kernel::CmpOp::Ge,
                    Value::U64(200),
                ))
                .sort_by([SortKey::asc(distance)])
                .limit(3),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let ids: Vec<u64> = rows
        .iter()
        .filter_map(|r| match r.get(col("id")) {
            Some(Value::U64(n)) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![200, 201, 202], "nearest among the admitted rows");
}

/// Comparing embeddings of different sizes is a mistake, not a distance — so
/// it is null, and a null sorts rather than crashing the query.
#[tokio::test]
async fn a_dimension_mismatch_is_null() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();
    let distance = Query::computed(&table, 0);

    let rows = txn
        .execute(
            &root(),
            &table,
            &Query::all()
                .computing([
                    Scalar::column(col("embedding")).distance(vec![1.0f32, 2.0], Metric::L2)
                ])
                .limit(3),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|r| r.get(distance) == Some(&Value::Null)),
        "a 2-dimension query against 8-dimension rows is not a distance"
    );
}
