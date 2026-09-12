//! Validation rules and the row body codec.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_schema::{
    Catalog, IndexDef, IndexId, Row, SchemaError, TableDef, TableId, decode_row, encode_body,
};
use slate_tuple::{Direction, Value, ValueType};
use uuid::Uuid;

fn users() -> TableDef {
    TableDef::builder("users", TableId(1))
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .nullable_column("display_name", ValueType::Str)
        .column("age", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            IndexDef::builder("by_email", IndexId(1))
                .column("email")
                .unique(),
        )
        .index(
            IndexDef::builder("by_age_desc", IndexId(2))
                .column_with("age", Direction::Desc)
                .column("email"),
        )
        .schema_version(3)
        .build()
        .expect("valid schema")
}

fn sample_row() -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(7)),
        Value::U64(42),
        Value::Str("a@example.com".into()),
        Value::Null,
        Value::I64(-3),
    ])
}

#[test]
fn body_omits_primary_key_and_round_trips() {
    let table = users();
    let row = sample_row();
    row.validate(&table).expect("row matches schema");

    let pk = row.primary_key_values(&table);
    let body = encode_body(&table, &row);
    let decoded = decode_row(&table, &pk, &body).expect("decode");

    assert_eq!(decoded, row);

    // The key columns really are absent from the body: a body carrying them
    // would be strictly longer than one carrying only the other three columns.
    let body_only = TableDef::builder("body_only", TableId(2))
        .column("k", ValueType::U64)
        .column("email", ValueType::Str)
        .nullable_column("display_name", ValueType::Str)
        .column("age", ValueType::I64)
        .primary_key(["k"])
        .schema_version(3)
        .build()
        .unwrap();
    let equivalent = Row::new(vec![
        Value::U64(42),
        Value::Str("a@example.com".into()),
        Value::Null,
        Value::I64(-3),
    ]);
    assert_eq!(
        encode_body(&table, &row).len(),
        encode_body(&body_only, &equivalent).len()
    );
}

#[test]
fn index_values_follow_index_column_order() {
    let table = users();
    let row = sample_row();
    let index = table.index_by_name("by_age_desc").unwrap();
    assert_eq!(
        row.index_values(index),
        vec![Value::I64(-3), Value::Str("a@example.com".into())]
    );
    assert_eq!(index.directions(), vec![Direction::Desc, Direction::Asc]);
}

#[test]
fn row_validation_catches_shape_errors() {
    let table = users();

    let wrong_width = Row::new(vec![Value::U64(1)]);
    assert!(matches!(
        wrong_width.validate(&table).unwrap_err(),
        SchemaError::ColumnCountMismatch { .. }
    ));

    let mut values = sample_row().into_values();
    values[2] = Value::I64(1);
    assert!(matches!(
        Row::new(values).validate(&table).unwrap_err(),
        SchemaError::ValueTypeMismatch { .. }
    ));

    let mut values = sample_row().into_values();
    values[4] = Value::Null;
    assert!(matches!(
        Row::new(values).validate(&table).unwrap_err(),
        SchemaError::UnexpectedNull { .. }
    ));
}

#[test]
fn nullable_primary_key_is_rejected() {
    // A null key component would let two rows share an identity.
    let err = TableDef::builder("t", TableId(1))
        .nullable_column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap_err();
    assert!(matches!(err, SchemaError::NullablePrimaryKeyColumn { .. }));
}

#[test]
fn tenant_column_must_lead_the_primary_key() {
    // Tenant scoping is a key prefix, so a trailing tenant column would silently
    // demote physical isolation to a filter.
    let err = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("tenant_id", ValueType::Uuid)
        .primary_key(["id", "tenant_id"])
        .tenant_column("tenant_id")
        .build()
        .unwrap_err();
    assert!(matches!(err, SchemaError::TenantColumnNotKeyPrefix { .. }));
}

#[test]
fn definition_errors_are_caught_at_build_time() {
    let dup_column = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("id", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap_err();
    assert!(matches!(dup_column, SchemaError::DuplicateColumn { .. }));

    let no_pk = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .build()
        .unwrap_err();
    assert!(matches!(no_pk, SchemaError::MissingPrimaryKey { .. }));

    let unknown = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["nope"])
        .build()
        .unwrap_err();
    assert!(matches!(unknown, SchemaError::UnknownColumn { .. }));

    let dup_key = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["id", "id"])
        .build()
        .unwrap_err();
    assert!(matches!(dup_key, SchemaError::DuplicateKeyColumn { .. }));

    let empty_index = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .index(IndexDef::builder("empty", IndexId(1)))
        .build()
        .unwrap_err();
    assert!(matches!(empty_index, SchemaError::EmptyIndex { .. }));

    let dup_index = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .index(IndexDef::builder("i", IndexId(1)).column("id"))
        .index(IndexDef::builder("i", IndexId(2)).column("id"))
        .build()
        .unwrap_err();
    assert!(matches!(dup_index, SchemaError::DuplicateIndex { .. }));
}

#[test]
fn catalog_rejects_colliding_tables() {
    let a = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap();
    let same_id = TableDef::builder("other", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap();
    let same_name = TableDef::builder("t", TableId(2))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap();

    assert!(matches!(
        Catalog::from_tables([a.clone(), same_id]).unwrap_err(),
        SchemaError::DuplicateTable { .. }
    ));
    assert!(matches!(
        Catalog::from_tables([a.clone(), same_name]).unwrap_err(),
        SchemaError::DuplicateTable { .. }
    ));

    let catalog = Catalog::from_tables([a]).unwrap();
    assert!(catalog.table(TableId(1)).is_some());
    assert!(catalog.table_by_name("t").is_some());
    assert!(catalog.table_by_name("missing").is_none());
}

/// A row written by an older build must still read back under a newer schema.
#[test]
fn columns_added_later_read_back_as_null() {
    let v1 = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id"])
        .schema_version(1)
        .build()
        .unwrap();

    let v2 = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .added_column("nickname", ValueType::Str, 2)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();

    let old_row = Row::new(vec![Value::U64(1), Value::Str("a@b.c".into())]);
    let pk = old_row.primary_key_values(&v1);
    let old_body = encode_body(&v1, &old_row);

    let read_back = decode_row(&v2, &pk, &old_body).expect("old row reads under new schema");
    assert_eq!(
        read_back,
        Row::new(vec![Value::U64(1), Value::Str("a@b.c".into()), Value::Null])
    );

    // And a row written at v2 round-trips with the new column populated.
    let new_row = Row::new(vec![
        Value::U64(2),
        Value::Str("d@e.f".into()),
        Value::Str("dee".into()),
    ]);
    let body = encode_body(&v2, &new_row);
    assert_eq!(
        decode_row(&v2, &new_row.primary_key_values(&v2), &body).unwrap(),
        new_row
    );
}

/// Reading a row from a schema this build does not know is an error, not a
/// silent truncation.
#[test]
fn rows_from_a_future_schema_are_rejected() {
    let v2 = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .added_column("nickname", ValueType::Str, 2)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();
    let v1 = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id"])
        .schema_version(1)
        .build()
        .unwrap();

    let row = Row::new(vec![
        Value::U64(1),
        Value::Str("a@b.c".into()),
        Value::Str("nick".into()),
    ]);
    let body = encode_body(&v2, &row);

    let err = decode_row(&v1, &[Value::U64(1)], &body).unwrap_err();
    assert!(
        matches!(err, SchemaError::RowFromFutureSchema { .. }),
        "got {err:?}"
    );
}

#[test]
fn malformed_bodies_are_rejected() {
    let table = users();
    let pk = sample_row().primary_key_values(&table);

    assert!(matches!(
        decode_row(&table, &pk, &[]).unwrap_err(),
        SchemaError::RowDecode { .. }
    ));
    assert!(matches!(
        decode_row(&table, &pk, &[99, 0, 0, 0, 1]).unwrap_err(),
        SchemaError::UnsupportedRowFormat { .. }
    ));

    let mut truncated = encode_body(&table, &sample_row());
    truncated.pop();
    assert!(matches!(
        decode_row(&table, &pk, &truncated).unwrap_err(),
        SchemaError::RowDecode { .. }
    ));

    let mut extended = encode_body(&table, &sample_row());
    extended.extend_from_slice(&slate_tuple::encode(&[Value::I64(1)]));
    assert!(matches!(
        decode_row(&table, &pk, &extended).unwrap_err(),
        SchemaError::RowDecode { .. }
    ));
}
