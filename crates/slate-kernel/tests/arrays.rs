//! An array column, end to end: declared, written, read back, filtered, sorted.
//!
//! The element type is declared on the column rather than carried in
//! `ValueType::Array`, which is the same arrangement a decimal's scale has and
//! is argued in `docs/arrays.md` §1. Everything here is a consequence of that
//! choice: the column is the only thing that knows what an element should be,
//! so the column is where an element is checked, and the element type is
//! hashed into the schema fingerprint the way a scale is.
//!
//! An array cannot be a key or an index column, and the refusal says why in
//! terms of containment rather than of ordering — an array's order is
//! perfectly meaningful, which is what makes it a *different* refusal from the
//! vector's.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, CmpOp, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext,
    SortKey, migrate,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, SchemaError, TableDef, TableId};
use slate_tuple::{Value, ValueType, decode, encode};

const T: TableId = TableId(1);

fn table() -> TableDef {
    TableDef::builder("posts", T)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .array_column("tags", ValueType::Str)
        .nullable_array_column("scores", ValueType::I64)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    table().ordinal_of(name).expect("column exists")
}

fn tags(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| Value::Str((*s).to_owned())).collect())
}

fn scores(items: &[i64]) -> Value {
    Value::Array(items.iter().copied().map(Value::I64).collect())
}

fn row(id: u64, title: &str, t: &[&str], s: Option<&[i64]>) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(title.to_owned()),
        tags(t),
        s.map_or(Value::Null, scores),
    ])
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn rows() -> Vec<Row> {
    vec![
        row(1, "first", &["rust", "db"], Some(&[3, 1])),
        row(2, "second", &["db"], None),
        row(3, "third", &[], Some(&[])),
        row(4, "fourth", &["rust", "db", "wal"], Some(&[3, 1, 4])),
    ]
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([table()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", T, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &table(), &rows()).await.unwrap();
    txn.commit().await.unwrap();
    store
}

fn tags_of(row: &Row) -> Vec<String> {
    match row.get(col("tags")) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.clone(),
                other => panic!("a tag came back as {other:?}"),
            })
            .collect(),
        other => panic!("tags came back as {other:?}"),
    }
}

/// A row with array columns survives a write and a read, elements intact.
///
/// Including the two cases a list type gets wrong: an empty array, which must
/// not read back as null, and a null array, which must not read back as empty.
/// They encode differently — `0x24 0x00` against `0x01` — and a store that
/// confused them would lose the distinction between "no tags" and "tags
/// unknown".
#[tokio::test]
async fn an_array_column_round_trips_through_the_store() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let back = txn
        .execute(
            &root(),
            &table,
            &Query::all().sort_by([SortKey::asc(col("id"))]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(back.len(), 4);
    assert_eq!(tags_of(&back[0]), ["rust", "db"]);
    assert_eq!(tags_of(&back[1]), ["db"]);
    assert_eq!(tags_of(&back[2]), Vec::<String>::new());
    assert_eq!(tags_of(&back[3]), ["rust", "db", "wal"]);

    // Empty and absent are different values and stay different.
    assert_eq!(back[2].get(col("scores")), Some(&Value::Array(Vec::new())));
    assert_eq!(back[1].get(col("scores")), Some(&Value::Null));
}

/// An array compares as a whole value, so an ordinary equality filter works.
///
/// This is the surface the first version offers in place of containment: you
/// can ask for rows whose array *is* this one. Asking which rows *contain* an
/// element is the thing that needs an inverted index and is not built.
#[tokio::test]
async fn an_array_can_be_compared_for_equality() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let matched = txn
        .execute(
            &root(),
            &table,
            &Query::all().filter(Expr::compare(col("tags"), CmpOp::Eq, tags(&["db"]))),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].get(col("id")), Some(&Value::U64(2)));

    // And the ordering is list ordering: `["db"]` sorts below `["rust", ...]`
    // because "db" < "rust", not because it is shorter.
    let ordered = txn
        .execute(
            &root(),
            &table,
            &Query::all().sort_by([SortKey::asc(col("tags"))]),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let seen: Vec<Vec<String>> = ordered.iter().map(tags_of).collect();
    assert_eq!(
        seen,
        vec![
            Vec::<String>::new(),
            vec!["db".to_owned()],
            vec!["rust".to_owned(), "db".to_owned()],
            vec!["rust".to_owned(), "db".to_owned(), "wal".to_owned()],
        ],
        "arrays must sort element-wise with a shorter prefix first"
    );
}

/// An array cannot be a primary key or an index column, and the refusal says
/// what would be needed instead.
///
/// A *different* error from the vector's, deliberately. A vector is refused
/// because its order says nothing; an array's order says plenty, and what is
/// missing is the index cardinality that containment needs. One message would
/// have had to be wrong about one of them.
#[test]
fn an_array_cannot_be_a_key_or_an_index() {
    let as_key = TableDef::builder("bad", TableId(9))
        .array_column("tags", ValueType::Str)
        .primary_key(["tags"])
        .build();
    assert!(
        matches!(as_key, Err(SchemaError::ArrayInKey { .. })),
        "got {as_key:?}"
    );

    let as_index = TableDef::builder("bad", TableId(9))
        .column("id", ValueType::U64)
        .array_column("tags", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_tags", IndexId(10)).column("tags"))
        .build();
    assert!(
        matches!(as_index, Err(SchemaError::ArrayInKey { .. })),
        "got {as_index:?}"
    );

    let message = format!("{}", as_index.unwrap_err());
    assert!(
        message.contains("containment"),
        "the refusal should name what an index would have to do: {message}"
    );

    // An ordinary column of one is fine, which is what makes the two cases
    // above about keys rather than about arrays.
    assert!(table().column(col("tags")).is_some());
}

/// An array column must say what it holds, and must not say "another array".
#[test]
fn an_array_column_declares_its_element_type_and_cannot_nest() {
    let unstated = TableDef::builder("bad", TableId(9))
        .column("id", ValueType::U64)
        // Deliberately not `array_column`: this is the shape a caller reaches
        // through `column`, and through `slate-serverd`'s TOML, where the type
        // is a string and the element type is a separate key.
        .column("tags", ValueType::Array)
        .primary_key(["id"])
        .build();
    assert!(
        matches!(unstated, Err(SchemaError::ArrayWithoutElementType { .. })),
        "got {unstated:?}"
    );

    let nested = TableDef::builder("bad", TableId(9))
        .column("id", ValueType::U64)
        .array_column("tags", ValueType::Array)
        .primary_key(["id"])
        .build();
    assert!(
        matches!(nested, Err(SchemaError::NestedArrayColumn { .. })),
        "got {nested:?}"
    );
}

/// An element of the wrong type is refused at the write, not at the read.
///
/// The codec is perfectly happy to hold a heterogeneous array — nothing in the
/// encoding says the elements share a type — so if this check did not exist
/// the row would store, encode, and read back as a value the schema says
/// cannot exist.
#[tokio::test]
async fn an_element_of_the_wrong_type_is_refused() {
    let store = seeded().await;
    let table = table();

    let wrong = Row::new(vec![
        Value::U64(5),
        Value::Str("fifth".to_owned()),
        Value::Array(vec![Value::Str("ok".to_owned()), Value::I64(7)]),
        Value::Null,
    ]);
    let txn = store.begin().await.unwrap();
    // The rendered message, not the `Debug`: a caller sees the `Display`, and
    // the thing worth asserting is that it names *which* element rather than
    // saying the row is bad.
    let message = txn
        .insert(&root(), &table, &wrong)
        .await
        .expect_err("an i64 in an array of strings must be refused")
        .to_string();
    assert!(
        message.contains("element 1") && message.contains("array of string"),
        "the refusal should name the element and the declared type: {message}"
    );

    // A null element is refused too, and that is a decision rather than a
    // consequence — see `docs/arrays.md`, which leaves nullable elements open.
    // Refusing is the reversible half of an open question.
    let with_null = Row::new(vec![
        Value::U64(6),
        Value::Str("sixth".to_owned()),
        Value::Array(vec![Value::Str("ok".to_owned()), Value::Null]),
        Value::Null,
    ]);
    let txn = store.begin().await.unwrap();
    assert!(txn.insert(&root(), &table, &with_null).await.is_err());

    // The column's own nullability is a different knob and still works: the
    // whole `scores` value may be absent.
    let null_column = Row::new(vec![
        Value::U64(7),
        Value::Str("seventh".to_owned()),
        tags(&["fine"]),
        Value::Null,
    ]);
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &null_column).await.unwrap();
    txn.commit().await.unwrap();
}

/// Changing a column's element type changes the schema fingerprint.
///
/// For the reason a decimal's scale does: it decides how every element of
/// every stored value is read, so a silent change reinterprets rows already
/// written. The element type is *not* folded into the column's type code —
/// that would make an array of strings indistinguishable from some future type
/// whose code happened to match — it is hashed beside it.
#[test]
fn the_element_type_is_part_of_the_fingerprint() {
    let of = |element: ValueType| {
        TableDef::builder("posts", T)
            .column("id", ValueType::U64)
            .array_column("tags", element)
            .primary_key(["id"])
            .build()
            .expect("valid schema")
    };

    assert_ne!(
        migrate::fingerprint(&of(ValueType::Str)),
        migrate::fingerprint(&of(ValueType::I64)),
        "an array of strings and an array of integers are different layouts"
    );
    assert_eq!(
        migrate::fingerprint(&of(ValueType::Str)),
        migrate::fingerprint(&of(ValueType::Str)),
        "and the same declaration hashes the same"
    );

    // A non-array column has no element type, so an array column must not
    // fingerprint like a plain column of its element type either.
    let plain = TableDef::builder("posts", T)
        .column("id", ValueType::U64)
        .column("tags", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_ne!(
        migrate::fingerprint(&of(ValueType::Str)),
        migrate::fingerprint(&plain)
    );
}

/// The element type is readable from the column, and only from an array.
///
/// The last case is the one with teeth, and the first version of this test did
/// not have it: asserting `None` for a column that was never *given* an element
/// type proves nothing, because the field is `None` anyway. `element_for` can
/// set one on any column — as `scale_for` can set a scale on any column — and
/// what must hold is that reading it back through a non-array type still says
/// `None`. A mutation returning the field directly survived until this case
/// existed.
#[test]
fn only_an_array_column_reports_an_element_type() {
    let table = table();
    assert_eq!(
        table.column(col("tags")).unwrap().element_type(),
        Some(ValueType::Str)
    );
    assert_eq!(
        table.column(col("scores")).unwrap().element_type(),
        Some(ValueType::I64)
    );
    assert_eq!(table.column(col("title")).unwrap().element_type(), None);

    let stray = TableDef::builder("posts", T)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .element_for("title", ValueType::I64)
        .primary_key(["id"])
        .build()
        .expect("a stray element type is ignored, not refused");
    let ordinal = stray.ordinal_of("title").expect("column exists");
    assert_eq!(
        stray.column(ordinal).unwrap().element_type(),
        None,
        "a string column has no element type however the builder was called"
    );

    // And the stray value does not reach the fingerprint either, which is the
    // reason ignoring it is safe rather than merely tidy: two schemas that
    // differ only in a setting no type reads must not look like a migration.
    let plain = TableDef::builder("posts", T)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_eq!(migrate::fingerprint(&stray), migrate::fingerprint(&plain));
}

/// `SUM` over an array column refuses; `COUNT` counts it.
///
/// `docs/arrays.md` left this open and guessed that an array would reach
/// `Total::add`'s wildcard and be refused "very likely" the way a vector is.
/// "Very likely" is how three of this session's findings started, so it is run
/// rather than reasoned. It does refuse, and it names the type — which is the
/// part worth pinning, because the wildcard could just as easily have produced
/// a zero.
///
/// `COUNT` is the other half and is *not* a refusal: counting rows that have a
/// value is meaningful for any type, arrays included, and a count that skipped
/// them would be a wrong number rather than an error.
#[tokio::test]
async fn an_array_cannot_be_summed_but_can_be_counted() {
    let store = seeded().await;
    let table = table();
    let txn = store.begin().await.unwrap();

    let summed = txn
        .aggregate(
            &root(),
            &table,
            &Query::all(),
            &[Aggregate::Sum(col("tags"))],
        )
        .await
        .expect_err("an array has no sum")
        .to_string();
    assert!(
        summed.contains("array"),
        "the refusal should name the type it cannot add: {summed}"
    );

    let counted = txn
        .aggregate(&root(), &table, &Query::all(), &[Aggregate::Count])
        .await
        .unwrap();
    assert_eq!(counted[0], Value::U64(4));
}

/// The values themselves survive the codec and sort as lists sort.
///
/// `slate-tuple`'s own suites prove this as a property over generated arrays;
/// this is the same claim stated in the types a column actually holds, so that
/// a change to the schema layer that quietly encoded arrays some other way
/// would fail here rather than only there.
#[test]
fn array_values_round_trip_and_encode_in_order() {
    let cases = vec![
        Value::Array(Vec::new()),
        scores(&[0]),
        scores(&[0, 1]),
        scores(&[1]),
        scores(&[1, i64::MIN]),
        scores(&[i64::MAX]),
    ];

    for value in &cases {
        let bytes = encode(core::slice::from_ref(value));
        let back = decode(&bytes, &[ValueType::Array]).expect("decodes");
        assert_eq!(back.len(), 1);
        assert_eq!(&back[0], value, "{value:?} did not round trip");
    }

    let mut sorted = cases.clone();
    sorted.sort();
    let mut by_bytes = cases;
    by_bytes.sort_by_key(|v| encode(core::slice::from_ref(v)));
    assert_eq!(sorted, by_bytes, "encoding disagreed with ordering");
}
