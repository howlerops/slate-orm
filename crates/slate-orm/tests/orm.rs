//! The typed surface: what the derive produces, and what it does at runtime.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, Direction, Expr, Field, FieldError, Grant, IndexDef, IndexId, Record,
    RecordError, RecordStore, Records, Row, ScanOrder, SecurityCatalog, SecurityContext, TableDef,
    TableId, Value, ValueType, memory::MemoryStore,
};
use uuid::Uuid;

const USERS: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "users", id = 1, version = 2, tenant = "tenant_id")]
#[record(index(name = "by_role_and_age", id = 12, columns("role", desc("age"))))]
struct User {
    #[record(pk)]
    tenant_id: Uuid,
    #[record(pk)]
    id: u64,
    #[record(index(name = "by_email", id = 10, unique))]
    #[record(rename = "email_address")]
    email: String,
    nickname: Option<String>,
    #[record(index(name = "by_age", id = 11, desc))]
    age: i64,
    role: String,
    #[record(added_in = 2)]
    bio: Option<String>,
}

fn alice(tenant: u128, id: u64) -> User {
    User {
        tenant_id: Uuid::from_u128(tenant),
        id,
        email: format!("u{id}@example.com"),
        nickname: None,
        age: 30,
        role: "member".to_owned(),
        bio: Some("hello".to_owned()),
    }
}

/// The derive must produce exactly the table a person would have written by
/// hand. If these ever diverge, the macro is doing something of its own.
#[test]
fn the_derived_table_matches_a_hand_written_one() {
    let expected = TableDef::builder("users", USERS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("email_address", ValueType::Str)
        .nullable_column("nickname", ValueType::Str)
        .column("age", ValueType::I64)
        .column("role", ValueType::Str)
        .added_column("bio", ValueType::Str, 2)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(
            IndexDef::builder("by_role_and_age", IndexId(12))
                .column("role")
                .column_with("age", Direction::Desc),
        )
        .index(
            IndexDef::builder("by_email", IndexId(10))
                .column("email_address")
                .unique(),
        )
        .index(IndexDef::builder("by_age", IndexId(11)).column_with("age", Direction::Desc))
        .schema_version(2)
        .build()
        .expect("hand-written schema is valid");

    assert_eq!(*User::table(), expected);
}

/// Nullability comes from `Field`, not from the macro recognising `Option`.
#[test]
fn nullability_is_taken_from_the_type() {
    let table = User::table();
    let nullable = |name: &str| {
        table
            .column(table.ordinal_of(name).unwrap())
            .unwrap()
            .is_nullable()
    };
    assert!(!nullable("email_address"));
    assert!(nullable("nickname"));
    // A later-added column is nullable because rows written before it have no
    // value for it.
    assert!(nullable("bio"));

    // The same holds through an alias, which a type-pattern-matching macro
    // would get wrong.
    type MaybeName = Option<String>;
    const { assert!(<MaybeName as Field>::NULLABLE) };
    assert_eq!(<MaybeName as Field>::VALUE_TYPE, ValueType::Str);
}

#[test]
fn rows_round_trip_through_the_derived_codec() {
    let user = alice(1, 7);
    let row = user.to_row();
    assert_eq!(row.values().len(), User::table().columns().len());
    row.validate(User::table()).expect("row matches the schema");
    assert_eq!(User::from_row(&row).unwrap(), user);
    assert_eq!(
        user.primary_key(),
        vec![Value::Uuid(Uuid::from_u128(1)), Value::U64(7)]
    );
}

#[test]
fn a_row_of_the_wrong_shape_is_reported_not_guessed() {
    let short = Row::new(vec![Value::U64(1)]);
    assert!(matches!(
        User::from_row(&short).unwrap_err(),
        RecordError::ColumnCount { .. }
    ));

    let mut values = alice(1, 1).to_row().into_values();
    values[4] = Value::Str("not a number".into());
    let err = User::from_row(&Row::new(values)).unwrap_err();
    match err {
        RecordError::Field { column, .. } => assert_eq!(column, "age"),
        other => panic!("expected a field error, got {other:?}"),
    }
}

#[test]
fn narrow_integer_fields_range_check_rather_than_wrap() {
    assert_eq!(<i32 as Field>::from_value(&Value::I64(7)).unwrap(), 7);
    assert!(matches!(
        <i32 as Field>::from_value(&Value::I64(i64::from(i32::MAX) + 1)).unwrap_err(),
        FieldError::OutOfRange { .. }
    ));
    assert!(matches!(
        <i32 as Field>::from_value(&Value::Null).unwrap_err(),
        FieldError::UnexpectedNull { .. }
    ));
    assert!(matches!(
        <i32 as Field>::from_value(&Value::Str("x".into())).unwrap_err(),
        FieldError::TypeMismatch { .. }
    ));
    assert_eq!(
        <Option<i32> as Field>::from_value(&Value::Null).unwrap(),
        None
    );
}

/// `f32` widens exactly on the way in; a `f64` that does not fit is an error
/// rather than a silent rounding, since a narrowed key would stop matching.
#[test]
fn f32_round_trips_but_refuses_to_narrow_lossily() {
    let stored = <f32 as Field>::to_value(&1.5f32);
    assert_eq!(<f32 as Field>::from_value(&stored).unwrap(), 1.5f32);
    assert!(matches!(
        <f32 as Field>::from_value(&Value::F64(1e300)).unwrap_err(),
        FieldError::OutOfRange { .. }
    ));
    assert!(
        <f32 as Field>::from_value(&Value::F64(f64::NAN))
            .unwrap()
            .is_nan()
    );
}

fn store() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([User::table().clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("member", USERS, Action::ALL));
    RecordStore::new(MemoryStore::new(), catalog, security)
}

fn context(tenant: u128) -> SecurityContext {
    SecurityContext::new(
        slate_orm::Principal::new(Value::U64(1))
            .with_tenant(Value::Uuid(Uuid::from_u128(tenant)))
            .with_role("member"),
    )
}

#[tokio::test]
async fn typed_operations_go_through_the_kernel() {
    let store = store();
    let ctx = context(1);

    let txn = store.begin().await.unwrap();
    txn.insert_record(&ctx, &alice(1, 1)).await.unwrap();
    txn.insert_record(&ctx, &alice(1, 2)).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let found: Option<User> = txn
        .get_record(&ctx, &[Value::Uuid(Uuid::from_u128(1)), Value::U64(1)])
        .await
        .unwrap();
    assert_eq!(found, Some(alice(1, 1)));

    let mut updated = alice(1, 1);
    updated.nickname = Some("ali".to_owned());
    txn.update_record(&ctx, &updated).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let all: Vec<User> = txn
        .find_records(&ctx, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].nickname.as_deref(), Some("ali"));

    let by_age: Vec<User> = txn
        .find_records(
            &ctx,
            Expr::compare(
                User::table().ordinal_of("age").unwrap(),
                slate_orm::CmpOp::Ge,
                Value::I64(30),
            ),
            ScanOrder::Ascending,
        )
        .await
        .unwrap();
    assert_eq!(by_age.len(), 2);

    assert!(
        txn.delete_record::<User>(&ctx, &[Value::Uuid(Uuid::from_u128(1)), Value::U64(2)])
            .await
            .unwrap()
    );
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let remaining: Vec<User> = txn
        .find_records(&ctx, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
}

/// The typed layer adds no enforcement of its own, so tenant scoping has to
/// come through unchanged.
#[tokio::test]
async fn the_typed_layer_does_not_widen_access() {
    let store = store();
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    txn.insert_record(&root, &alice(1, 1)).await.unwrap();
    txn.insert_record(&root, &alice(2, 1)).await.unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let ours: Vec<User> = txn
        .find_records(&context(1), Expr::True, ScanOrder::Ascending)
        .await
        .unwrap();
    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0].tenant_id, Uuid::from_u128(1));

    // And a record from another tenant is not reachable by primary key either.
    let theirs: Option<User> = txn
        .get_record(
            &context(1),
            &[Value::Uuid(Uuid::from_u128(2)), Value::U64(1)],
        )
        .await
        .unwrap();
    assert_eq!(theirs, None);
}

#[tokio::test]
async fn a_unique_index_declared_on_a_field_is_enforced() {
    let store = store();
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    txn.insert_record(&root, &alice(1, 1)).await.unwrap();
    let mut clash = alice(1, 2);
    clash.email = alice(1, 1).email;
    let err = txn.insert_record(&root, &clash).await.unwrap_err();
    assert!(
        matches!(
            err,
            slate_orm::OrmError::Kernel(slate_orm::KernelError::UniqueViolation { .. })
        ),
        "got {err:?}"
    );
}
