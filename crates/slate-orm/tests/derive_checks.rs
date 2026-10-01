//! `#[record(check(...))]` through the ORM.
//!
//! `CheckDef` has carried `column` and `message` since
//! `ledger/2026-09-19-a-check-that-names-its-field.md`, and the TOML loader
//! has been able to declare both since the same day. The derive could declare
//! **no check at all**, which that entry's caveat understated as "cannot
//! declare either field" — so a table written in Rust had to be built through
//! `TableDef::builder` to carry a check, and the surface most likely to be
//! read as "how you declare a table in Rust" could not express the feature.
//!
//! The ways this goes wrong quietly are what the tests are arranged around,
//! not the feature:
//!
//! - A check parsed and then dropped on the way to the builder. Every
//!   behavioural test below catches that, but only as "the bad row went in",
//!   so the first test compares the whole derived table against a hand-written
//!   one.
//! - A `column` that reaches a form as a field name nothing renders beside.
//!   That is the failure the field exists to prevent, so the field-level
//!   default is checked against the **renamed** column rather than the field
//!   ident, and a `column` naming nothing is a compile error (documented on
//!   the derive itself, in `crates/slate-orm/src/lib.rs`).
//! - A predicate whose ordinals are off by one. `CheckDef`'s `PartialEq`
//!   compares **the name and nothing else** — its own comment says so, and a
//!   mutation run proved it: dropping `with_column` from the macro's output
//!   entirely left `the_derived_checks_match_the_hand_written_ones` green.
//!   So the equality test is a check *roster* and is written here as one, and
//!   every column, message and predicate is asserted field by field below.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, CheckDef, CmpOp, Expr, Grant, Ordinal, Principal, Record, RecordStore,
    Records, SchemaError, SecurityCatalog, SecurityContext, TableDef, TableId, Value, ValueType,
    memory::MemoryStore,
};

const ITEMS: TableId = TableId(1);

/// A priced item with three checks, one of each shape the attribute allows.
///
/// `priced` sits on the struct and names its column explicitly, because that
/// is the only position a struct-level check has. `named` sits on the field
/// and names none, so the default fills it in — and the field is `rename`d, to
/// pin that the default resolves to the column rather than to the field ident.
/// `discounted` sits on a field and names a *different* column, which is how a
/// two-column rule says which field a form should blame.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "items", id = 1)]
#[record(check(
    name = "priced",
    predicate(Expr::compare(price, CmpOp::Ge, Value::I64(0))),
    column = "price",
    message = "Price cannot be negative."
))]
#[record(check(
    name = "discounted",
    predicate(Expr::compare(discount, CmpOp::Le, Value::I64(100))),
    // About `discount` and `price` both; it names `discount` because that is
    // the field a person edits to fix it.
    column = "discount",
    message = "Discount cannot exceed 100."
))]
struct Item {
    #[record(pk)]
    id: u64,
    price: i64,
    discount: i64,
    #[record(rename = "label")]
    #[record(check(
        name = "named",
        predicate(Expr::matches(title, "^.{1,8}$")),
        message = "A label is 1 to 8 characters."
    ))]
    title: String,
}

/// The table a person would have written by hand.
///
/// Written by **ordinal**, because `CheckDef::new` takes a predicate and a
/// predicate names columns by `Ordinal` — the same asymmetry the partial index
/// suite pins, and the same reason the attribute is worth having. Struct-level
/// checks are emitted before field-level ones, and `TableDef` equality
/// compares the check list in order.
fn hand_written() -> TableDef {
    TableDef::builder("items", ITEMS)
        .column("id", ValueType::U64)
        .column("price", ValueType::I64)
        .column("discount", ValueType::I64)
        .column("label", ValueType::Str)
        .primary_key(["id"])
        .check(
            CheckDef::new(
                "priced",
                Expr::compare(Ordinal(1), CmpOp::Ge, Value::I64(0)),
            )
            .with_column("price")
            .with_message("Price cannot be negative."),
        )
        .check(
            CheckDef::new(
                "discounted",
                Expr::compare(Ordinal(2), CmpOp::Le, Value::I64(100)),
            )
            .with_column("discount")
            .with_message("Discount cannot exceed 100."),
        )
        .check(
            CheckDef::new("named", Expr::matches(Ordinal(3), "^.{1,8}$"))
                .with_column("label")
                .with_message("A label is 1 to 8 characters."),
        )
        .build()
        .expect("hand-written schema is valid")
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

fn store() -> RecordStore<MemoryStore> {
    RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([Item::table().clone()]).unwrap(),
        SecurityCatalog::new().grant(Grant::new("app", ITEMS, Action::EVERYTHING)),
    )
}

fn item(id: u64, price: i64, discount: i64, title: &str) -> Item {
    Item {
        id,
        price,
        discount,
        title: title.to_owned(),
    }
}

/// The derived table is the hand-written one — columns, key, and the check
/// roster in order.
///
/// Equality alone would be much weaker than it reads: `CheckDef::eq` compares
/// names only, on the argument that a predicate is a Rust function and
/// functions do not compare. So this asserts the names *and* the fields
/// equality drops, and the two assertions are written separately so a failure
/// says which half moved. Found by mutation: with `with_column` deleted from
/// the macro's output, `assert_eq!(*Item::table(), hand_written())` was still
/// green.
#[test]
fn the_derived_checks_match_the_hand_written_ones() {
    let derived = Item::table();
    let expected = hand_written();
    assert_eq!(*derived, expected);

    let fields = |table: &TableDef| -> Vec<(String, Option<String>, Option<String>)> {
        table
            .checks()
            .iter()
            .map(|c| {
                (
                    c.name().to_owned(),
                    c.column().map(str::to_owned),
                    c.message().map(str::to_owned),
                )
            })
            .collect()
    };
    assert_eq!(fields(derived), fields(&expected));
}

/// The field-level default resolves to the **column**, not the field ident.
///
/// This is the assertion that would catch the default being filled in before
/// `rename` is applied: the check would then carry `column = "title"`, a name
/// no client ever sees, and every behavioural test below would still pass. A
/// form would render the message beside nothing, which is the failure
/// `column` exists to prevent, arriving by the one route the feature itself
/// opened.
#[test]
fn a_field_level_check_names_the_renamed_column() {
    let table = Item::table();
    let named = table
        .checks()
        .iter()
        .find(|c| c.name() == "named")
        .expect("the field-level check");
    assert_eq!(named.column(), Some("label"));
    assert_eq!(named.message(), Some("A label is 1 to 8 characters."));

    // And an explicit `column` on a field-level check still wins, which is how
    // a rule about two columns sits beside one of them and blames the other.
    let discounted = table
        .checks()
        .iter()
        .find(|c| c.name() == "discounted")
        .expect("the check written on `discount`");
    assert_eq!(discounted.column(), Some("discount"));
}

/// The ordinals the macro computed are the ones the predicate needed.
///
/// Nothing above reaches the predicate at all — not `TableDef` equality, not
/// the field-by-field comparison, because `CheckDef` exposes no predicate to
/// compare and `PartialEq` would not use it if it did. This is the only
/// assertion in the file that reads one, and it reads it the one way a
/// predicate can be read: by running it. A macro that resolved `price` to
/// `Ordinal(2)` would refuse on `discount` instead, and both columns are
/// `I64`, so nothing but a value would notice.
#[test]
fn the_predicates_read_the_columns_they_name() {
    let table = Item::table();
    let good = item(1, 10, 50, "ok").to_row();
    assert!(
        table.checks().iter().all(|c| c.satisfied_by(&good)),
        "a good row satisfies every check"
    );

    for (name, row) in [
        ("priced", item(2, -1, 50, "ok")),
        ("discounted", item(3, 10, 101, "ok")),
        ("named", item(4, 10, 50, "far too long a label")),
    ] {
        let row = row.to_row();
        let refused: Vec<&str> = table
            .checks()
            .iter()
            .filter(|c| !c.satisfied_by(&row))
            .map(slate_orm::CheckDef::name)
            .collect();
        assert_eq!(
            refused,
            vec![name],
            "exactly `{name}` should refuse this row"
        );
    }
}

/// The whole point, end to end: a write the check forbids is refused, and the
/// refusal carries the column and the message a form needs.
#[tokio::test]
async fn a_write_a_derived_check_forbids_is_refused_with_its_column_and_message() {
    let store = store();
    let txn = store.begin().await.unwrap();
    txn.insert_record(&context(), &item(1, 10, 50, "fine"))
        .await
        .expect("a row every check admits");
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let error = txn
        .insert_record(&context(), &item(2, -1, 50, "fine"))
        .await
        .expect_err("a negative price is refused");

    // Asserted on the typed failure rather than on the rendered sentence: the
    // sentence is what a person reads and the fields are what a form uses, and
    // only one of those is a contract. An earlier version of this test
    // asserted `is_err()`, which passes if the row is refused for having the
    // wrong number of columns.
    let message = error.to_string();
    let SchemaError::CheckViolation { violations, .. } = check_violation(&error) else {
        panic!("expected a check violation, got: {message}");
    };
    assert_eq!(violations.len(), 1, "one check failed: {message}");
    assert_eq!(violations[0].check, "priced");
    assert_eq!(violations[0].column.as_deref(), Some("price"));
    assert_eq!(
        violations[0].message.as_deref(),
        Some("Price cannot be negative.")
    );
}

/// Every failing check at once, which is what makes the list worth carrying.
///
/// The derive emits the checks in a fixed order — struct-level first, then
/// field-level in field order — and the write path reports them in declaration
/// order, so this also pins that the macro's order is the order a form sees.
#[tokio::test]
async fn three_bad_fields_are_three_violations_in_declaration_order() {
    let store = store();
    let txn = store.begin().await.unwrap();
    let error = txn
        .insert_record(&context(), &item(1, -1, 101, "far too long a label"))
        .await
        .expect_err("every check refuses this row");

    let message = error.to_string();
    let SchemaError::CheckViolation { violations, .. } = check_violation(&error) else {
        panic!("expected a check violation, got: {message}");
    };
    let names: Vec<&str> = violations.iter().map(|v| v.check.as_str()).collect();
    assert_eq!(names, vec!["priced", "discounted", "named"]);
    let columns: Vec<Option<&str>> = violations.iter().map(|v| v.column.as_deref()).collect();
    assert_eq!(
        columns,
        vec![Some("price"), Some("discount"), Some("label")]
    );
}

/// Dig the `SchemaError` out of whatever the ORM wrapped it in.
///
/// Written as a helper rather than matched inline because the two tests above
/// would otherwise each carry the unwrapping, and a change to how the ORM
/// wraps a kernel error would then be two edits — one of which could be
/// forgotten in a way that turns an assertion into a `panic!` arm nobody
/// reaches.
fn check_violation(error: &slate_orm::OrmError) -> &SchemaError {
    match error {
        slate_orm::OrmError::Kernel(slate_orm::KernelError::Schema(schema)) => schema,
        other => panic!("expected a schema error, got: {other:?}"),
    }
}
