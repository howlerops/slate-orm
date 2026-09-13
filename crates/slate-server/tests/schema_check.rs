//! Checking a client's copy of a table against the server's, and — the part
//! that decides whether the check is worth having — what it does across a
//! migration.
//!
//! `ColumnRef` removed the arithmetic across tables and left the ordinal
//! within one. A client outside Rust knows `title` is column 3 because it
//! wrote that down, and when it writes it down wrong the server accepts the
//! reference and answers about a different column. That is the failure these
//! tests are about.
//!
//! # A check that broke on every migration would be worse than none
//!
//! That is the whole of the design risk, so most of what follows is
//! migrations. This project supports `DEFAULT`, `CHECK`, foreign keys, column
//! drop and column rename, and every one of them leaves an existing ordinal
//! naming the same column — the schema layer guarantees it, by never moving an
//! ordinal and by keeping a renamed column resolvable under its old name. The
//! fingerprint has to agree, or an operator adding a nullable column takes down
//! every client in the fleet and the second thing they do is turn the check
//! off.
//!
//! Each migration below is therefore written as *two* table definitions, v1
//! and v2, exactly as a schema evolving in a repository would be, with the
//! claim computed from v1 and checked against v2.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, docs_query, drain, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{CmpOp, Expr};
use slate_schema::{CheckDef, IndexDef, IndexId, Ordinal, TableBuilder, TableDef, TableId};
use slate_server::fingerprint;
use slate_server::proto as pb;
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use tonic::Code;

const DOCS: TableId = TableId(1);

/// The table as it stands before any of the migrations below.
fn v1() -> TableDef {
    base().build().expect("valid schema")
}

fn base() -> TableBuilder {
    TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
}

/// What a client declaring every column of `table` sends.
fn claim(table: &TableDef) -> pb::SchemaCheck {
    fingerprint::claim(table)
}

/// What a client declaring only the first `columns` of `table` sends.
fn short_claim(table: &TableDef, columns: usize) -> pb::SchemaCheck {
    pb::SchemaCheck {
        columns: columns as u32,
        fingerprint: fingerprint::of(table, columns),
    }
}

fn accepted(table: &TableDef, claim: &pb::SchemaCheck) -> Result<(), String> {
    fingerprint::check(table, Some(claim)).map_err(|status| status.message().to_owned())
}

fn assert_accepted(table: &TableDef, claim: &pb::SchemaCheck, why: &str) {
    if let Err(message) = accepted(table, claim) {
        panic!("{why}, but it was refused: {message}");
    }
}

fn assert_refused(table: &TableDef, claim: &pb::SchemaCheck, why: &str) -> String {
    match accepted(table, claim) {
        Ok(()) => panic!("{why}, but it was accepted"),
        Err(message) => message,
    }
}

// --- the property, before the migrations ----------------------------------

#[test]
fn a_client_that_agrees_is_accepted_and_no_claim_at_all_is_served() {
    assert_accepted(&v1(), &claim(&v1()), "the same declaration must agree");
    // Optional on the wire. A protocol that only works for clients that
    // adopted a new field is a protocol that broke.
    assert!(fingerprint::check(&v1(), None).is_ok());
}

/// The failure the Python client measured and pinned: a `Table` declaring
/// `category` where the server has `kind` passes a width check and then
/// filters the wrong column, with no error anywhere.
#[test]
fn a_renamed_column_the_server_never_had_is_refused() {
    let wrong = base()
        .build()
        .map(|_| ())
        .and(Ok(()))
        .and_then(|()| {
            TableDef::builder("docs", DOCS)
                .column("id", ValueType::U64)
                .column("category", ValueType::Str)
                .column("size", ValueType::I64)
                .nullable_column("note", ValueType::Str)
                .primary_key(["id"])
                .build()
        })
        .expect("valid schema");
    assert_refused(
        &v1(),
        &claim(&wrong),
        "a declaration naming a column this table does not have must be refused",
    );
}

/// Two same-typed columns swapped: the width agrees, every ordinal is in
/// range, every reference resolves, and every answer is about the wrong
/// column. This is the case a width check cannot see.
#[test]
fn two_columns_of_the_same_type_swapped_are_refused() {
    let swapped = TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("note", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("kind", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_eq!(
        swapped.columns().len(),
        v1().columns().len(),
        "the point of this case is that the widths agree"
    );
    assert_refused(
        &v1(),
        &claim(&swapped),
        "two same-typed columns swapped must be refused",
    );
}

/// A column inserted in the middle rather than appended shifts everything
/// past it — the "same query, different answer" shape.
#[test]
fn a_column_inserted_in_the_middle_is_refused() {
    let shifted = TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .nullable_column("inserted", ValueType::Str)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_refused(
        &v1(),
        &short_claim(&shifted, 4),
        "a column inserted in the middle re-points every reference past it",
    );
}

#[test]
fn a_client_declaring_more_columns_than_the_table_has_is_refused_and_told_so() {
    let wider = base()
        .schema_version(1)
        .added_column("extra", ValueType::I64, 1)
        .build()
        .expect("valid schema");
    let message = assert_refused(
        &v1(),
        &claim(&wider),
        "a client addressing columns this table does not have must be refused",
    );
    assert!(
        message.contains("5 columns") && message.contains("has 4"),
        "the commonest disagreement should be reported as a count: {message}"
    );
}

#[test]
fn the_primary_key_is_part_of_the_declaration() {
    let other_key = TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id", "kind"])
        .build()
        .expect("valid schema");
    assert_refused(
        &v1(),
        &claim(&other_key),
        "a client that would build keys of the wrong shape must be refused",
    );
}

#[test]
fn the_table_name_is_part_of_the_declaration() {
    let renamed = TableDef::builder("documents", DOCS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_refused(
        &v1(),
        &claim(&renamed),
        "a different table is a different declaration",
    );
}

/// Types are in, because a client must know them to write at all — the write
/// path refuses `i64` where `u64` is declared — and because a type is the one
/// thing a swap of two columns often does not change.
#[test]
fn a_column_of_the_wrong_type_is_refused() {
    let retyped = TableDef::builder("docs", DOCS)
        .column("id", ValueType::I64)
        .column("kind", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_refused(&v1(), &claim(&retyped), "u64 and i64 are different keys");
}

// --- the migrations -------------------------------------------------------

/// The migration that decides the design. A nullable column is appended; every
/// client written before it is still correct, because its ordinals all still
/// name the columns it thinks they do — and it must keep working through the
/// deployment, before anybody has updated it.
#[test]
fn adding_a_column_does_not_break_a_client_written_before_it() {
    let v2 = base()
        .schema_version(1)
        .added_column("owner", ValueType::Str, 1)
        .build()
        .expect("valid schema");

    assert_accepted(
        &v2,
        &claim(&v1()),
        "a client declaring the four columns that existed when it was written \
         is declaring a prefix of this table, and every one of its ordinals \
         still names the same column",
    );
    // And a client updated to the new declaration is accepted too, which is
    // the other half: the two must not be alternatives.
    assert_accepted(&v2, &claim(&v2), "the current declaration must agree");
}

/// The same, for a `NOT NULL` column with a default — the migration `DEFAULT`
/// exists for.
#[test]
fn adding_a_not_null_column_with_a_default_does_not_break_an_older_client() {
    let v2 = base()
        .schema_version(1)
        .added_column_with_default("owner", ValueType::U64, 1, Value::U64(0))
        .build()
        .expect("valid schema");
    assert_accepted(&v2, &claim(&v1()), "an appended column is a prefix");
}

/// A dropped column keeps its ordinal for ever, so nothing after it shifts and
/// no reference changes meaning. The fingerprint must not change either — a
/// client that never touched the column has nothing to update.
#[test]
fn dropping_a_column_does_not_change_the_fingerprint() {
    let v2 = base()
        .schema_version(1)
        .drop_column("note", 1)
        .build()
        .expect("valid schema");
    assert_eq!(
        fingerprint::of_table(&v2),
        fingerprint::of_table(&v1()),
        "a drop keeps the ordinal and the declaration, so it must keep the fingerprint"
    );
    assert_accepted(&v2, &claim(&v1()), "a drop moves no ordinal");
}

/// A rename records the old name and keeps resolving it, so that "code and
/// queries written against the old name keep working". A check that refused
/// the old name would contradict the feature it is checking; one that ignored
/// names entirely would not catch the misdeclaration above.
#[test]
fn a_renamed_column_is_accepted_under_either_name() {
    let v2 = TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("category", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .renamed_column("category", "kind")
        .build()
        .expect("valid schema");

    assert_accepted(
        &v2,
        &claim(&v1()),
        "the previous name still resolves in the catalog, so it must still pass",
    );
    assert_accepted(&v2, &claim(&v2), "and so must the current one");

    // A third name is not one of the two, and is still the failure this whole
    // mechanism exists for.
    let invented = TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("genre", ValueType::Str)
        .column("size", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_refused(
        &v2,
        &claim(&invented),
        "a name this column never answered to must still be refused",
    );
}

/// Constraints and indexes are not addressing. Adding either must not
/// invalidate a client: an index is added for performance, and a `CHECK` is
/// refused by name at write time, which is a better error than a fingerprint
/// mismatch on an unrelated read.
#[test]
fn constraints_and_indexes_are_not_part_of_the_fingerprint() {
    let with_check = base()
        .check(CheckDef::new(
            "positive_size",
            Expr::compare(Ordinal(2), CmpOp::Gt, Value::I64(0)),
        ))
        .build()
        .expect("valid schema");
    let with_index = base()
        .index(IndexDef::builder("by_kind", IndexId(1)).column("kind"))
        .build()
        .expect("valid schema");
    let with_default = base()
        .default_for("kind", Value::Str("unfiled".to_owned()))
        .build()
        .expect("valid schema");

    for (name, table) in [
        ("a CHECK", with_check),
        ("an index", with_index),
        ("a DEFAULT", with_default),
    ] {
        assert_eq!(
            fingerprint::of_table(&table),
            fingerprint::of_table(&v1()),
            "adding {name} addresses no column and must not change the fingerprint"
        );
    }
}

/// A prefix declaration that stops before a key column is refused with its own
/// message: such a client cannot build a primary key for this table at all, so
/// it is not the truncated-but-usable case above.
#[test]
fn a_declaration_too_short_to_hold_the_key_is_refused_by_name() {
    let keyed_late = TableDef::builder("docs", DOCS)
        .column("kind", ValueType::Str)
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    let message = assert_refused(
        &keyed_late,
        &short_claim(&keyed_late, 1),
        "a declaration that stops before the key must be refused",
    );
    assert!(
        message.contains("key column"),
        "the message should say what is missing: {message}"
    );
}

// --- and it is enforced on the wire ---------------------------------------

/// The check is only worth anything where a request goes through it, so this
/// asserts the end a client sees: a wrong declaration is `INVALID_ARGUMENT`
/// and nothing is read.
#[tokio::test]
async fn a_query_whose_declaration_disagrees_is_refused_over_grpc() {
    let writer = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&writer)).await;
    let mut client = serving.client().await;

    // The control: the same query with a correct declaration is served.
    let ok = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(docs_query()),
            freshness: None,
        }))
        .await
        .expect("a correct declaration is served");
    drain(ok.into_inner()).await;

    let wrong = pb::SchemaCheck {
        columns: 4,
        fingerprint: fingerprint::of_table(&v1()) ^ 1,
    };
    let error = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                schema: Some(wrong),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .expect_err("a declaration that disagrees must be refused");
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(
        error.message().contains("schema check"),
        "{}",
        error.message()
    );
}

/// A write is worth checking more than a read: it is positional across the
/// whole row, so a declaration that disagrees writes every value into the
/// wrong column and is only caught by luck, when two columns happen to differ
/// in type.
#[tokio::test]
async fn an_insert_whose_declaration_disagrees_is_refused_over_grpc() {
    let writer = Arc::new(MemoryStore::new());
    let serving = serving_leader(Arc::clone(&writer)).await;
    let mut client = serving.client().await;

    let error = client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![slate_server::convert::row_to_proto(&common::doc(
                1, "a", 10, None,
            ))],
            upsert: false,
            schema: Some(pb::SchemaCheck {
                columns: 4,
                fingerprint: 0,
            }),
        }))
        .await
        .expect_err("a declaration that disagrees must be refused");
    assert_eq!(error.code(), Code::InvalidArgument);

    // Nothing was written: the refusal happens before the rows are decoded.
    let (rows, _) = drain(
        client
            .query(app(pb::QueryRequest {
                transaction: String::new(),
                query: Some(docs_query()),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert!(
        rows.is_empty(),
        "the refused insert wrote {} rows",
        rows.len()
    );
}

// --- the canonical form is a wire contract --------------------------------

/// The fingerprint is only worth anything if a client in another language
/// computes the same number, so the number is pinned. A change to the
/// canonical form fails here rather than in a client somebody else maintains
/// — where it would present as every request being refused after an upgrade.
///
/// The reference implementation, which is the whole specification and is
/// twelve lines on purpose:
///
/// ```python
/// def fnv(b):
///     h = 0xcbf29ce484222325
///     for x in b:
///         h ^= x
///         h = (h * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
///     return h
///
/// def s(x): x = x.encode(); return str(len(x)).encode() + b":" + x
/// def d(n): return str(n).encode() + b";"
///
/// def fingerprint(name, columns, key_ordinals):
///     out = b"slate.v1.schema/1" + s(name)
///     for i, (cname, ctype) in enumerate(columns):
///         out += d(i) + s(cname) + s(ctype)
///     out += b"key" + d(len(key_ordinals))
///     for k in key_ordinals: out += d(k)
///     return out + b"columns" + d(len(columns))
/// ```
///
/// The values below were produced by running exactly that against the fixture
/// tables, so this asserts agreement with a second implementation rather than
/// with itself.
#[test]
fn the_canonical_form_is_pinned_against_an_implementation_in_another_language() {
    assert_eq!(
        fingerprint::of_table(&common::docs()),
        0xf8bd_ead5_a04b_5bb5,
        "the canonical form changed; every client's fingerprint has to change with it"
    );
    assert_eq!(
        fingerprint::of_table(&common::users()),
        0x193a_2476_1c50_eb32
    );
    // A prefix is its own value, and it is what an older client sends.
    assert_eq!(fingerprint::of(&common::docs(), 2), 0x4fde_f413_2267_2bf0);
}

/// Length-prefixing rather than delimiting is what stops one declaration being
/// spelled to look like another. Without it, a column called `kind` of type
/// `string` and a column called `kind:6:string` of nothing in particular could
/// hash alike.
#[test]
fn a_column_name_cannot_be_spelled_to_look_like_the_next_field() {
    let sneaky = TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("kind:6:string2:", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    let plain = TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema");
    assert_ne!(
        fingerprint::of_table(&sneaky),
        fingerprint::of_table(&plain)
    );
}
