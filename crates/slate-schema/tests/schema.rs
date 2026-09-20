//! Validation rules and the row body codec.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_schema::{
    Catalog, CheckDef, ForeignKeyDef, IndexDef, IndexId, Ordinal, PartialRow, Predicate,
    ReferentialAction, Row, SchemaError, TableDef, TableId, decode_row, encode_body,
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

// --- defaults ---------------------------------------------------------------

#[test]
fn an_unset_column_takes_its_default_and_a_set_one_does_not() {
    let table = TableDef::builder("items", TableId(9))
        .column("id", ValueType::U64)
        .column("status", ValueType::Str)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .default_for("status", Value::Str("new".into()))
        .build()
        .unwrap();
    let id = table.ordinal_of("id").unwrap();
    let status = table.ordinal_of("status").unwrap();
    let note = table.ordinal_of("note").unwrap();

    let defaulted = PartialRow::for_table(&table)
        .set(id, Value::U64(1))
        .into_row(&table)
        .unwrap();
    assert_eq!(defaulted.get(status), Some(&Value::Str("new".into())));
    // A column with neither a value nor a default is null, not an error, as
    // long as it is nullable.
    assert_eq!(defaulted.get(note), Some(&Value::Null));

    let supplied = PartialRow::for_table(&table)
        .set(id, Value::U64(2))
        .set(status, Value::Str("done".into()))
        .into_row(&table)
        .unwrap();
    assert_eq!(supplied.get(status), Some(&Value::Str("done".into())));
}

/// The reason `PartialRow` exists rather than reading a null as "unset".
#[test]
fn an_explicit_null_beats_a_default() {
    let table = TableDef::builder("items", TableId(9))
        .column("id", ValueType::U64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .default_for("note", Value::Str("none".into()))
        .build()
        .unwrap();
    let id = table.ordinal_of("id").unwrap();
    let note = table.ordinal_of("note").unwrap();

    let row = PartialRow::for_table(&table)
        .set(id, Value::U64(1))
        .set(note, Value::Null)
        .into_row(&table)
        .unwrap();
    assert_eq!(
        row.get(note),
        Some(&Value::Null),
        "a null the caller wrote is a value, not an omission"
    );
}

#[test]
fn an_unset_column_with_no_default_still_has_to_be_nullable() {
    let table = TableDef::builder("items", TableId(9))
        .column("id", ValueType::U64)
        .column("status", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap();
    let err = PartialRow::for_table(&table)
        .set(table.ordinal_of("id").unwrap(), Value::U64(1))
        .into_row(&table)
        .unwrap_err();
    assert!(matches!(err, SchemaError::UnexpectedNull { .. }), "{err:?}");
}

#[test]
fn a_default_must_match_its_column() {
    let wrong_type = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("n", ValueType::I64)
        .primary_key(["id"])
        .default_for("n", Value::Str("nope".into()))
        .build()
        .unwrap_err();
    assert!(matches!(wrong_type, SchemaError::ValueTypeMismatch { .. }));

    let null_default = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .nullable_column("n", ValueType::I64)
        .primary_key(["id"])
        .default_for("n", Value::Null)
        .build()
        .unwrap_err();
    assert!(matches!(null_default, SchemaError::NullDefault { .. }));

    let unknown = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .default_for("nope", Value::U64(1))
        .build()
        .unwrap_err();
    assert!(matches!(unknown, SchemaError::UnknownColumn { .. }));
}

/// The migration a default exists for: a `NOT NULL` column added to a table
/// that already has rows, without rewriting one of them.
#[test]
fn a_column_added_with_a_default_reads_back_on_older_rows() {
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
        .added_column_with_default("tier", ValueType::Str, 2, Value::Str("free".into()))
        .column("trailing", ValueType::I64)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();

    // A column added *before* the trailing one, so a decoder that read the new
    // column out of the old row's bytes would also mangle `trailing` — which is
    // the failure that would otherwise be silent.
    let v1_with_trailing = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .column("trailing", ValueType::I64)
        .primary_key(["id"])
        .schema_version(1)
        .build()
        .unwrap();

    let old = Row::new(vec![
        Value::U64(1),
        Value::Str("a@b.c".into()),
        Value::I64(-7),
    ]);
    let body = encode_body(&v1_with_trailing, &old);
    let read_back = decode_row(&v2, &[Value::U64(1)], &body).unwrap();
    assert_eq!(
        read_back,
        Row::new(vec![
            Value::U64(1),
            Value::Str("a@b.c".into()),
            Value::Str("free".into()),
            Value::I64(-7),
        ])
    );

    // Nothing was rewritten: a v2 row that supplies the same value is longer
    // than the v1 row it reads identically to, because the v1 row never carried
    // the column at all.
    let v2_row = Row::new(vec![
        Value::U64(1),
        Value::Str("a@b.c".into()),
        Value::Str("free".into()),
        Value::I64(-7),
    ]);
    assert!(encode_body(&v2, &v2_row).len() > body.len());
    assert_eq!(v1.schema_version(), 1);
}

// --- dropping ---------------------------------------------------------------

/// The load-bearing property: the *following* column still decodes.
///
/// A dropped column's bytes are still in every row written before the drop. A
/// decoder that stopped walking past them would read the next column out of
/// them and return a plausible wrong value rather than an error.
#[test]
fn a_dropped_column_is_skipped_in_old_rows_and_absent_from_new_ones() {
    let v1 = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("legacy", ValueType::Str)
        .column("after", ValueType::I64)
        .primary_key(["id"])
        .schema_version(1)
        .build()
        .unwrap();
    let v2 = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("legacy", ValueType::Str)
        .column("after", ValueType::I64)
        .primary_key(["id"])
        .schema_version(2)
        .drop_column("legacy", 2)
        .build()
        .unwrap();

    let old = Row::new(vec![
        Value::U64(1),
        Value::Str("junk".into()),
        Value::I64(42),
    ]);
    let read_back = decode_row(&v2, &[Value::U64(1)], &encode_body(&v1, &old)).unwrap();
    assert_eq!(
        read_back,
        Row::new(vec![Value::U64(1), Value::Null, Value::I64(42)]),
        "the dropped column reads as null and `after` survives"
    );

    // A row written at v2 carries no bytes for the dropped column at all, so it
    // is strictly shorter than the same row written at v1.
    let new = Row::new(vec![Value::U64(1), Value::Null, Value::I64(42)]);
    assert!(
        encode_body(&v2, &new).len() < encode_body(&v1, &old).len(),
        "a dropped column is still being written"
    );
    assert_eq!(
        decode_row(&v2, &[Value::U64(1)], &encode_body(&v2, &new)).unwrap(),
        new
    );

    // Ordinals do not move. `after` is still ordinal 2, which is what stops the
    // drop from silently renumbering every caller's column constants.
    assert_eq!(v2.ordinal_of("after"), Some(Ordinal(2)));
    assert_eq!(v2.columns().len(), 3);
}

#[test]
fn a_row_may_not_carry_a_value_for_a_dropped_column() {
    let table = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("legacy", ValueType::Str)
        .primary_key(["id"])
        .schema_version(2)
        .drop_column("legacy", 2)
        .build()
        .unwrap();

    // Refused rather than discarded: a value that vanishes between write and
    // read looks exactly like one that was stored.
    let err = Row::new(vec![Value::U64(1), Value::Str("x".into())])
        .validate(&table)
        .unwrap_err();
    assert!(
        matches!(err, SchemaError::DroppedColumnValue { .. }),
        "{err:?}"
    );
    Row::new(vec![Value::U64(1), Value::Null])
        .validate(&table)
        .expect("a null in the dropped slot is fine");
}

#[test]
fn a_drop_is_refused_where_it_would_break_something() {
    let in_key = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .schema_version(2)
        .drop_column("id", 2)
        .build()
        .unwrap_err();
    assert!(matches!(in_key, SchemaError::DroppedColumnInKey { .. }));

    let in_index = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("n", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_n", IndexId(1)).column("n"))
        .schema_version(2)
        .drop_column("n", 2)
        .build()
        .unwrap_err();
    assert!(matches!(in_index, SchemaError::DroppedColumnInKey { .. }));

    let before_added = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .added_column("n", ValueType::I64, 3)
        .primary_key(["id"])
        .schema_version(3)
        .drop_column("n", 3)
        .build()
        .unwrap_err();
    assert!(matches!(
        before_added,
        SchemaError::ColumnDroppedBeforeAdded { .. }
    ));

    // A drop the table has not reached would leave the encoder still writing
    // the column while the schema says it is gone.
    let ahead = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("n", ValueType::I64)
        .primary_key(["id"])
        .schema_version(1)
        .drop_column("n", 2)
        .build()
        .unwrap_err();
    assert!(matches!(
        ahead,
        SchemaError::ColumnDroppedInFutureVersion { .. }
    ));

    let pointless = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("n", ValueType::I64)
        .primary_key(["id"])
        .schema_version(2)
        .default_for("n", Value::I64(0))
        .drop_column("n", 2)
        .build()
        .unwrap_err();
    assert!(matches!(
        pointless,
        SchemaError::DroppedColumnHasDefault { .. }
    ));
}

// --- renaming ---------------------------------------------------------------

#[test]
fn a_renamed_column_answers_to_both_names_and_moves_no_bytes() {
    let before = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["id"])
        .schema_version(1)
        .build()
        .unwrap();
    let after = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("full_name", ValueType::Str)
        .primary_key(["id"])
        .schema_version(2)
        .renamed_column("full_name", "name")
        .build()
        .unwrap();

    assert_eq!(after.ordinal_of("full_name"), Some(Ordinal(1)));
    assert_eq!(after.ordinal_of("name"), Some(Ordinal(1)));
    assert_eq!(after.ordinal_of("nope"), None);

    // Nothing on disk carries a name, so a row written before the rename reads
    // back unchanged — and identically byte for byte.
    //
    // The bodies differ only in the schema version they are stamped with; the
    // payload after that five-byte header is identical, which is the claim.
    let row = Row::new(vec![Value::U64(1), Value::Str("Ada".into())]);
    let old_body = encode_body(&before, &row);
    let new_body = encode_body(&after, &row);
    assert_eq!(old_body[5..], new_body[5..]);
    assert_eq!(
        decode_row(&after, &[Value::U64(1)], &old_body).unwrap(),
        row
    );
}

#[test]
fn a_name_that_would_resolve_two_ways_is_refused() {
    // Renaming `a` to `b` and reusing `a` for something else makes
    // `ordinal_of("a")` a coin toss decided by declaration order.
    let err = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("b", ValueType::Str)
        .column("a", ValueType::I64)
        .primary_key(["id"])
        .renamed_column("b", "a")
        .build()
        .unwrap_err();
    assert!(
        matches!(err, SchemaError::AmbiguousColumnName { .. }),
        "{err:?}"
    );

    let two_columns_one_old_name = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .column("b", ValueType::Str)
        .column("c", ValueType::Str)
        .primary_key(["id"])
        .renamed_column("b", "a")
        .renamed_column("c", "a")
        .build()
        .unwrap_err();
    assert!(matches!(
        two_columns_one_old_name,
        SchemaError::AmbiguousColumnName { .. }
    ));
}

// --- checks -----------------------------------------------------------------

/// A stand-in for the kernel's `Expr`, which this crate cannot name.
///
/// Only enough to prove a check is stored, asked, and given SQL's answer when
/// the predicate is unknown. The expression language itself is tested where it
/// lives.
#[derive(Debug)]
struct NonNegative(Ordinal);

impl Predicate for NonNegative {
    fn truth(&self, row: &Row) -> Option<bool> {
        match row.get(self.0) {
            Some(Value::I64(n)) => Some(*n >= 0),
            // A null makes the comparison unknown, exactly as `Expr` does.
            _ => None,
        }
    }
}

#[test]
fn a_check_accepts_an_unknown_predicate_and_refuses_a_false_one() {
    let table = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .nullable_column("n", ValueType::I64)
        .primary_key(["id"])
        .check(CheckDef::new("n_non_negative", NonNegative(Ordinal(1))))
        .build()
        .unwrap();
    let check = table.checks().first().unwrap();

    assert!(check.satisfied_by(&Row::new(vec![Value::U64(1), Value::I64(0)])));
    assert!(!check.satisfied_by(&Row::new(vec![Value::U64(1), Value::I64(-1)])));
    assert!(
        check.satisfied_by(&Row::new(vec![Value::U64(1), Value::Null])),
        "SQL: a check passes when its predicate is unknown, unlike a WHERE"
    );
}

#[test]
fn two_checks_may_not_share_a_name() {
    let err = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .nullable_column("n", ValueType::I64)
        .primary_key(["id"])
        .check(CheckDef::new("c", NonNegative(Ordinal(1))))
        .check(CheckDef::new("c", NonNegative(Ordinal(1))))
        .build()
        .unwrap_err();
    assert!(matches!(err, SchemaError::DuplicateCheck { .. }));
}

#[test]
fn a_check_may_not_carry_an_empty_message() {
    // `Some("")` rather than `None`. The two are not the same thing: a check
    // with no message is ordinary, and one *given* a message and given nothing
    // is an author who meant to write a sentence. It reaches a form as a blank
    // error beside the field it is supposed to explain, and renders the
    // refusal with a dangling colon.
    let err = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .nullable_column("n", ValueType::I64)
        .primary_key(["id"])
        .check(CheckDef::new("c", NonNegative(Ordinal(1))).with_message(""))
        .build()
        .unwrap_err();
    assert!(
        matches!(err, SchemaError::EmptyCheckMessage { .. }),
        "got {err:?}"
    );
}

#[test]
fn a_check_message_of_only_whitespace_is_empty_too() {
    // Trimmed, because a message of three spaces renders exactly as badly as
    // one of none and is harder to spot in a config file.
    let err = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .nullable_column("n", ValueType::I64)
        .primary_key(["id"])
        .check(CheckDef::new("c", NonNegative(Ordinal(1))).with_message("  \n "))
        .build()
        .unwrap_err();
    assert!(
        matches!(err, SchemaError::EmptyCheckMessage { .. }),
        "got {err:?}"
    );
}

#[test]
fn a_check_with_no_message_is_fine() {
    // The control, and the reason this is not simply "messages are required":
    // most checks have none, and demanding one would be a different and much
    // larger decision than refusing a blank.
    let table = TableDef::builder("t", TableId(1))
        .column("id", ValueType::U64)
        .nullable_column("n", ValueType::I64)
        .primary_key(["id"])
        .check(CheckDef::new("c", NonNegative(Ordinal(1))))
        .build()
        .expect("a check with no message is ordinary");
    assert_eq!(table.checks()[0].message(), None);
}

// --- foreign keys -----------------------------------------------------------

fn parent() -> TableDef {
    TableDef::builder("parent", TableId(1))
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .unwrap()
}

fn child_referencing(columns: &[&str]) -> TableDef {
    let mut key = ForeignKeyDef::builder("by_parent", TableId(1));
    for column in columns {
        key = key.column(*column);
    }
    TableDef::builder("child", TableId(2))
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .nullable_column("parent_id", ValueType::U64)
        .nullable_column("wrong_type", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .foreign_key(key.on_delete(ReferentialAction::Cascade))
        .build()
        .unwrap()
}

#[test]
fn a_foreign_key_must_match_the_parents_whole_primary_key() {
    Catalog::from_tables([parent(), child_referencing(&["tenant_id", "parent_id"])])
        .expect("a reference to the whole key is fine");

    let too_narrow =
        Catalog::from_tables([parent(), child_referencing(&["parent_id"])]).unwrap_err();
    assert!(matches!(
        too_narrow,
        SchemaError::ForeignKeyWidthMismatch { .. }
    ));

    let wrong_type =
        Catalog::from_tables([parent(), child_referencing(&["tenant_id", "wrong_type"])])
            .unwrap_err();
    assert!(matches!(
        wrong_type,
        SchemaError::ForeignKeyTypeMismatch { .. }
    ));

    let no_parent =
        Catalog::from_tables([child_referencing(&["tenant_id", "parent_id"])]).unwrap_err();
    assert!(matches!(
        no_parent,
        SchemaError::UnknownForeignKeyParent { .. }
    ));
}

#[test]
fn a_null_anywhere_in_a_reference_means_it_references_nothing() {
    let child = child_referencing(&["tenant_id", "parent_id"]);
    let key = child.foreign_keys().first().unwrap();
    let tenant = Value::Uuid(Uuid::from_u128(1));

    let referencing = Row::new(vec![
        tenant.clone(),
        Value::U64(1),
        Value::U64(7),
        Value::Null,
    ]);
    assert_eq!(
        key.parent_key(&referencing),
        Some(vec![tenant.clone(), Value::U64(7)])
    );

    let dangling = Row::new(vec![tenant, Value::U64(1), Value::Null, Value::Null]);
    assert_eq!(
        key.parent_key(&dangling),
        None,
        "SQL MATCH SIMPLE: a null referencing column references nothing"
    );
}

#[test]
fn the_catalog_finds_who_references_a_table() {
    let catalog =
        Catalog::from_tables([parent(), child_referencing(&["tenant_id", "parent_id"])]).unwrap();
    let referencing = catalog.referencing(TableId(1));
    assert_eq!(referencing.len(), 1);
    let (table, key) = referencing.first().unwrap();
    assert_eq!(table.name(), "child");
    assert_eq!(key.on_delete(), ReferentialAction::Cascade);
    assert!(catalog.referencing(TableId(2)).is_empty());
}

// --- index ids are global, so the catalog has to say so --------------------

/// Two tables cannot share an index id, because the keyspace is global.
///
/// Measured before this check existed: `people` (3 rows) and `widgets` (5
/// rows) both declaring `IndexId(1)`, and an index scan of `people` returned
/// 5 — the other table's entries, through the same key range, because
/// `slate_kernel::keys::index_entry` writes the index space and the id and no
/// table id. `TableBuilder` refuses a duplicate *within* a table, which reads
/// like the check exists; it is the wrong scope for a global keyspace.
///
/// It is silent in the worst way: both tables build, the catalog validates,
/// and the wrong rows come back from a query that looks correct. The head
/// node's own test fixture had five tables on `IndexId(1)` when this was
/// written, so its suite had been running against overlapping index keyspaces
/// without anything noticing.
///
/// Refused here rather than fixed by putting the table id in the key. That is
/// the deeper fix and it is a storage format change — every index entry ever
/// written moves — where this costs one startup check and makes the collision
/// unrepresentable.
#[test]
fn two_tables_cannot_share_an_index_id() {
    let people = TableDef::builder("people", TableId(1))
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("people_by_kind", IndexId(1)).column("kind"))
        .build()
        .expect("valid");
    let widgets = TableDef::builder("widgets", TableId(2))
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("widgets_by_kind", IndexId(1)).column("kind"))
        .build()
        .expect("valid");

    let refused = Catalog::from_tables([people, widgets]);
    let Err(SchemaError::DuplicateIndexId {
        id,
        index,
        existing,
        ..
    }) = refused
    else {
        panic!("two tables shared an index id: {refused:?}");
    };
    assert_eq!(id, 1);
    // Both sides named: "duplicate index id 1" would send the reader looking
    // through every table in the catalog for the other one.
    assert_eq!(index, "widgets_by_kind");
    assert_eq!(existing, "people_by_kind");
}

/// The control: distinct ids across tables are fine, and so are several
/// indexes on one table.
#[test]
fn distinct_index_ids_across_tables_are_accepted() {
    let people = TableDef::builder("people", TableId(1))
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("people_by_kind", IndexId(1)).column("kind"))
        .index(IndexDef::builder("people_by_id", IndexId(2)).column("id"))
        .build()
        .expect("valid");
    let widgets = TableDef::builder("widgets", TableId(2))
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("widgets_by_kind", IndexId(3)).column("kind"))
        .build()
        .expect("valid");
    assert!(Catalog::from_tables([people, widgets]).is_ok());
}
