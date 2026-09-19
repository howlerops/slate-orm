//! `CHECK` constraints, foreign keys, and schema evolution through the store.
//!
//! Three claims are being tested here, and they fail in different ways:
//!
//! - A `CHECK` refuses a row **and passes one whose predicate is unknown**.
//!   That second half is SQL and is the opposite of a `WHERE`, so it is the
//!   half a reimplementation gets backwards. It is tested against the very same
//!   [`Expr`] used as a filter, so "these two disagree" is the assertion rather
//!   than a claim about one of them.
//! - A foreign key holds on write and on delete, and **cannot be used to learn
//!   about rows the caller may not see**. A constraint that reads another table
//!   is the obvious place for an existence oracle to appear, and it would be a
//!   real one: an insert that succeeds for key `k` and fails for key `j` says
//!   which of them exists.
//! - A column can be dropped or renamed and rows written before the change
//!   still read back — including the columns *after* the one that changed,
//!   which is where a body codec goes wrong silently rather than loudly.
//!
//! Cascades are also swept for atomicity, in the same shape as
//! `tests/crash.rs`: a delete and everything it cascades to must land together
//! or not at all, or there is a window in which a row points at a parent that
//! has gone.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use async_trait::async_trait;
use bytes::Bytes;
use slate_kernel::error::{KernelError, Result, StorageError};
use slate_kernel::memory::MemoryStore;
use slate_kernel::store::{KeyRange, KvIterator, KvSnapshot, KvStore, KvTransaction, ScanOrder};
use slate_kernel::{
    Action, CmpOp, Expr, Grant, Policy, Principal, Query, RecordStore, RecordTransaction,
    SecurityCatalog, SecurityContext, keys,
};
use slate_schema::{
    Catalog, CheckDef, ForeignKeyDef, IndexDef, IndexId, Ordinal, PartialRow, ReferentialAction,
    Row, SchemaError, TableDef, TableId,
};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

// --- schemas ----------------------------------------------------------------

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);
const REVIEWS: TableId = TableId(3);
const COMMENTS: TableId = TableId(4);
const DOSSIERS: TableId = TableId(5);
const NOTES: TableId = TableId(6);
const ORGS: TableId = TableId(7);
const MEMBERS: TableId = TableId(8);

/// `authors.age`, as an ordinal.
///
/// A check is defined while the table is being built, so it cannot ask the
/// table to resolve a name — the same position the derive macro's generated
/// column constants occupy.
const AGE: Ordinal = Ordinal(2);

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .nullable_column("age", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_name", IndexId(10)).column("name"))
        // Nullable on purpose: an unknown predicate is the interesting case.
        .check(
            CheckDef::new("age_positive", Expr::compare(AGE, CmpOp::Gt, Value::I64(0)))
                .with_column("age")
                .with_message("Age must be greater than zero."),
        )
        .build()
        .expect("valid schema")
}

fn books(on_delete: ReferentialAction) -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("id", ValueType::U64)
        .nullable_column("author_id", ValueType::U64)
        .column("title", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_author", IndexId(20)).column("author_id"))
        .foreign_key(
            ForeignKeyDef::builder("books_author", AUTHORS)
                .column("author_id")
                .on_delete(on_delete),
        )
        .build()
        .expect("valid schema")
}

fn reviews(on_delete: ReferentialAction) -> TableDef {
    TableDef::builder("reviews", REVIEWS)
        .column("id", ValueType::U64)
        .nullable_column("book_id", ValueType::U64)
        .column("stars", ValueType::I64)
        .primary_key(["id"])
        .foreign_key(
            ForeignKeyDef::builder("reviews_book", BOOKS)
                .column("book_id")
                .on_delete(on_delete),
        )
        .build()
        .expect("valid schema")
}

fn library(on_delete: ReferentialAction) -> Catalog {
    Catalog::from_tables([authors(), books(on_delete), reviews(on_delete)]).expect("catalog")
}

/// A table that references itself, so a cascade can meet a cycle.
fn comments() -> TableDef {
    TableDef::builder("comments", COMMENTS)
        .column("id", ValueType::U64)
        .nullable_column("reply_to", ValueType::U64)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .foreign_key(
            ForeignKeyDef::builder("comments_reply_to", COMMENTS)
                .column("reply_to")
                .on_delete(ReferentialAction::Cascade),
        )
        .build()
        .expect("valid schema")
}

/// A child with two references to the same parent, one of each action.
fn dossiers() -> TableDef {
    TableDef::builder("dossiers", DOSSIERS)
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("id", ValueType::U64)
        .nullable_column("dossier_id", ValueType::U64)
        .nullable_column("filed_under", ValueType::U64)
        .primary_key(["id"])
        .foreign_key(
            ForeignKeyDef::builder("notes_dossier", DOSSIERS)
                .column("dossier_id")
                .on_delete(ReferentialAction::Cascade),
        )
        .foreign_key(
            ForeignKeyDef::builder("notes_filed_under", DOSSIERS)
                .column("filed_under")
                .on_delete(ReferentialAction::Restrict),
        )
        .build()
        .expect("valid schema")
}

/// A tenant-scoped pair. The child's reference carries the tenant, because the
/// parent's primary key begins with it.
fn orgs() -> TableDef {
    TableDef::builder("orgs", ORGS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn members() -> TableDef {
    TableDef::builder("members", MEMBERS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("org_id", ValueType::U64)
        .column("owner_id", ValueType::Uuid)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .foreign_key(
            ForeignKeyDef::builder("members_org", ORGS)
                .column("tenant_id")
                .column("org_id")
                .on_delete(ReferentialAction::Cascade),
        )
        .build()
        .expect("valid schema")
}

// --- fixtures ---------------------------------------------------------------

fn store(catalog: Catalog) -> RecordStore<MemoryStore> {
    RecordStore::new(MemoryStore::new(), catalog, SecurityCatalog::new())
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn author(id: u64, name: &str, age: Option<i64>) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(name.to_owned()),
        age.map_or(Value::Null, Value::I64),
    ])
}

fn book(id: u64, author_id: Option<u64>, title: &str) -> Row {
    Row::new(vec![
        Value::U64(id),
        author_id.map_or(Value::Null, Value::U64),
        Value::Str(title.to_owned()),
    ])
}

fn review(id: u64, book_id: Option<u64>, stars: i64) -> Row {
    Row::new(vec![
        Value::U64(id),
        book_id.map_or(Value::Null, Value::U64),
        Value::I64(stars),
    ])
}

async fn ids(store: &RecordStore<MemoryStore>, table: &TableDef) -> Vec<u64> {
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&root(), table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let key = table.primary_key().last().copied().unwrap();
    rows.iter()
        .map(|row| match row.get(key) {
            Some(Value::U64(n)) => *n,
            other => panic!("unexpected key {other:?}"),
        })
        .collect()
}

/// How many index entries the backend physically holds for `table`.
///
/// Read straight out of the store rather than through a scan: the question is
/// what a cascade left behind, and asking the record layer would let a leaked
/// entry hide behind the same code that leaked it.
fn index_entry_count(backend: &MemoryStore, table: &TableDef) -> usize {
    backend
        .keys()
        .into_iter()
        .filter(|key| {
            table
                .indexes()
                .iter()
                .any(|index| key.starts_with(&keys::index_prefix(table, index, None)))
        })
        .count()
}

async fn seed_library(store: &RecordStore<MemoryStore>, on_delete: ReferentialAction) {
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &authors(), &author(1, "Ada", Some(36)))
        .await
        .unwrap();
    txn.insert(&root(), &authors(), &author(2, "Grace", Some(45)))
        .await
        .unwrap();
    txn.insert(&root(), &books(on_delete), &book(10, Some(1), "Notes"))
        .await
        .unwrap();
    txn.insert(&root(), &books(on_delete), &book(11, Some(1), "More"))
        .await
        .unwrap();
    txn.insert(&root(), &books(on_delete), &book(12, Some(2), "Other"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
}

// --- CHECK ------------------------------------------------------------------

#[tokio::test]
async fn a_check_refuses_a_row_and_leaves_nothing_behind() {
    let store = store(library(ReferentialAction::Restrict));
    let table = authors();

    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert(&root(), &table, &author(1, "Ada", Some(-1)))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &refused,
            KernelError::Schema(SchemaError::CheckViolation { check, .. }) if check == "age_positive"
        ),
        "got {refused:?}"
    );
    txn.commit().await.unwrap();
    assert!(ids(&store, &table).await.is_empty());

    // And the same row with an age the check admits goes in, so the refusal
    // above is about the value rather than about the write path.
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &author(1, "Ada", Some(1)))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &table).await, vec![1]);
}

/// SQL's rule, and the one a reimplementation gets backwards: a check passes
/// when its predicate is *unknown*.
///
/// Asserted against the identical [`Expr`] used as a filter, so what is pinned
/// is the disagreement between the two — a build that collapsed unknown to
/// false everywhere would fail the first half, and one that collapsed it to
/// true everywhere would fail the second.
#[tokio::test]
async fn a_check_passes_a_row_whose_predicate_is_unknown() {
    let store = store(library(ReferentialAction::Restrict));
    let table = authors();
    let predicate = Expr::compare(AGE, CmpOp::Gt, Value::I64(0));

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &author(1, "Ada", None))
        .await
        .expect("a null age makes `age > 0` unknown, and a CHECK admits unknown");
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &table).await, vec![1]);

    let txn = store.begin().await.unwrap();
    let matching = txn
        .query(&root(), &table, predicate, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(
        matching.is_empty(),
        "the same predicate as a WHERE must withhold the row it accepted as a CHECK"
    );
}

#[tokio::test]
async fn a_check_applies_to_every_way_of_writing_a_row() {
    let store = store(library(ReferentialAction::Restrict));
    let table = authors();

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &author(1, "Ada", Some(36)))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    for (what, outcome) in [
        (
            "update",
            txn.update(&root(), &table, &author(1, "Ada", Some(-1)))
                .await,
        ),
        (
            "upsert",
            txn.upsert(&root(), &table, &author(1, "Ada", Some(-1)))
                .await,
        ),
        (
            "insert_many",
            txn.insert_many(&root(), &table, &[author(2, "Grace", Some(-1))])
                .await,
        ),
        (
            "upsert_many",
            txn.upsert_many(&root(), &table, &[author(3, "Alan", Some(-1))])
                .await,
        ),
    ] {
        assert!(
            matches!(
                outcome,
                Err(KernelError::Schema(SchemaError::CheckViolation { .. }))
            ),
            "{what} accepted a row the check forbids"
        );
    }
    txn.commit().await.unwrap();

    // Nothing changed and nothing was added.
    assert_eq!(ids(&store, &table).await, vec![1]);
    let txn = store.begin().await.unwrap();
    let rows = txn
        .query(&root(), &table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows[0].get(AGE), Some(&Value::I64(36)));
}

/// A batch is one transaction, so one bad row costs the whole batch.
#[tokio::test]
async fn a_bulk_insert_with_one_failing_check_lands_nothing() {
    let store = store(library(ReferentialAction::Restrict));
    let table = authors();

    let batch: Vec<Row> = (1..=8)
        .map(|i| {
            author(
                i,
                &format!("a{i}"),
                Some(if i == 5 { -1 } else { i as i64 }),
            )
        })
        .collect();

    let txn = store.begin().await.unwrap();
    let outcome = txn.insert_many(&root(), &table, &batch).await;
    assert!(matches!(
        outcome,
        Err(KernelError::Schema(SchemaError::CheckViolation { .. }))
    ));
    txn.commit().await.unwrap();
    assert!(
        ids(&store, &table).await.is_empty(),
        "a batch with a bad row landed a prefix of itself"
    );
}

// --- foreign keys, on write -------------------------------------------------

#[tokio::test]
async fn a_reference_to_a_missing_parent_is_refused() {
    let store = store(library(ReferentialAction::Restrict));
    let table = books(ReferentialAction::Restrict);

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &authors(), &author(1, "Ada", Some(36)))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert(&root(), &table, &book(10, Some(99), "Ghost"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &refused,
            KernelError::Schema(SchemaError::ForeignKeyViolation { foreign_key, parent, .. })
                if foreign_key == "books_author" && parent == "authors"
        ),
        "got {refused:?}"
    );

    // The same insert against a parent that is there succeeds, in the same
    // transaction, so the refusal is about the reference and nothing else.
    txn.insert(&root(), &table, &book(10, Some(1), "Real"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &table).await, vec![10]);
}

#[tokio::test]
async fn a_null_reference_is_allowed() {
    let store = store(library(ReferentialAction::Restrict));
    let table = books(ReferentialAction::Restrict);

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &book(10, None, "Anonymous"))
        .await
        .expect("SQL MATCH SIMPLE: a null referencing column references nothing");
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &table).await, vec![10]);
}

#[tokio::test]
async fn moving_a_reference_is_checked_and_leaving_it_alone_is_not() {
    let store = store(library(ReferentialAction::Restrict));
    let books = books(ReferentialAction::Restrict);
    seed_library(&store, ReferentialAction::Restrict).await;

    let txn = store.begin().await.unwrap();
    let refused = txn
        .update(&root(), &books, &book(10, Some(99), "Notes"))
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        KernelError::Schema(SchemaError::ForeignKeyViolation { .. })
    ));

    // Changing everything except the reference is fine, and moving it to
    // another parent that exists is fine too.
    txn.update(&root(), &books, &book(10, Some(1), "Renamed"))
        .await
        .unwrap();
    txn.update(&root(), &books, &book(10, Some(2), "Renamed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
}

/// A batch may reference a parent written earlier in the same transaction.
///
/// The bulk path pre-reads the parents it can, and a key it did not find has to
/// fall back to a read that sees the transaction's own writes — otherwise
/// inserting a parent and its children together would be impossible.
#[tokio::test]
async fn a_bulk_insert_sees_parents_written_in_the_same_transaction() {
    let store = store(library(ReferentialAction::Restrict));
    let books_table = books(ReferentialAction::Restrict);

    let txn = store.begin().await.unwrap();
    txn.insert_many(&root(), &authors(), &[author(1, "Ada", Some(36))])
        .await
        .unwrap();
    txn.insert_many(
        &root(),
        &books_table,
        &[book(10, Some(1), "A"), book(11, Some(1), "B")],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &books_table).await, vec![10, 11]);

    // And one bad reference still costs the whole batch.
    let txn = store.begin().await.unwrap();
    let outcome = txn
        .insert_many(
            &root(),
            &books_table,
            &[book(12, Some(1), "C"), book(13, Some(99), "D")],
        )
        .await;
    assert!(matches!(
        outcome,
        Err(KernelError::Schema(SchemaError::ForeignKeyViolation { .. }))
    ));
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &books_table).await, vec![10, 11]);
}

// --- foreign keys, on delete ------------------------------------------------

#[tokio::test]
async fn restrict_refuses_the_delete_until_the_children_are_gone() {
    let store = store(library(ReferentialAction::Restrict));
    let authors_table = authors();
    let books_table = books(ReferentialAction::Restrict);
    seed_library(&store, ReferentialAction::Restrict).await;

    let txn = store.begin().await.unwrap();
    let refused = txn
        .delete(&root(), &authors_table, &[Value::U64(1)])
        .await
        .unwrap_err();
    assert!(
        matches!(
            &refused,
            KernelError::Schema(SchemaError::ForeignKeyRestricted { child, foreign_key, .. })
                if child == "books" && foreign_key == "books_author"
        ),
        "got {refused:?}"
    );
    txn.commit().await.unwrap();

    // Nothing was removed on the way to the refusal — not the row, and not its
    // index entries.
    assert_eq!(ids(&store, &authors_table).await, vec![1, 2]);
    assert_eq!(index_entry_count(store.backend(), &authors_table), 2);

    let txn = store.begin().await.unwrap();
    for id in [10u64, 11] {
        assert!(
            txn.delete(&root(), &books_table, &[Value::U64(id)])
                .await
                .unwrap()
        );
    }
    assert!(
        txn.delete(&root(), &authors_table, &[Value::U64(1)])
            .await
            .unwrap()
    );
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &authors_table).await, vec![2]);
}

#[tokio::test]
async fn cascade_removes_the_children_and_their_index_entries() {
    let store = store(library(ReferentialAction::Cascade));
    let authors_table = authors();
    let books_table = books(ReferentialAction::Cascade);
    seed_library(&store, ReferentialAction::Cascade).await;

    let txn = store.begin().await.unwrap();
    assert!(
        txn.delete(&root(), &authors_table, &[Value::U64(1)])
            .await
            .unwrap()
    );
    txn.commit().await.unwrap();

    assert_eq!(ids(&store, &authors_table).await, vec![2]);
    assert_eq!(
        ids(&store, &books_table).await,
        vec![12],
        "only the other author's book should survive"
    );
    // The load-bearing half: a cascade that deleted rows and left their index
    // entries would read as an empty table through a scan and as three books
    // through the index.
    assert_eq!(index_entry_count(store.backend(), &authors_table), 1);
    assert_eq!(index_entry_count(store.backend(), &books_table), 1);
}

#[tokio::test]
async fn a_cascade_is_transitive() {
    let store = store(library(ReferentialAction::Cascade));
    let authors_table = authors();
    let reviews_table = reviews(ReferentialAction::Cascade);
    seed_library(&store, ReferentialAction::Cascade).await;

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &reviews_table, &review(100, Some(10), 5))
        .await
        .unwrap();
    txn.insert(&root(), &reviews_table, &review(101, Some(12), 3))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.delete(&root(), &authors_table, &[Value::U64(1)])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(
        ids(&store, &reviews_table).await,
        vec![101],
        "the review two hops from the deleted author survived"
    );
}

/// A cascade must terminate on a cycle, and a self-reference is an ordinary
/// schema rather than a pathological one.
#[tokio::test]
async fn a_cascade_terminates_on_a_reference_cycle() {
    let store = store(Catalog::from_tables([comments()]).unwrap());
    let table = comments();
    let comment = |id: u64, reply_to: Option<u64>| {
        Row::new(vec![
            Value::U64(id),
            reply_to.map_or(Value::Null, Value::U64),
            Value::Str(format!("c{id}")),
        ])
    };

    // A chain first, then closed into a cycle by an update — the only way to
    // build one, since every insert must reference a row that already exists.
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &table, &comment(1, None))
        .await
        .unwrap();
    txn.insert(&root(), &table, &comment(2, Some(1)))
        .await
        .unwrap();
    txn.insert(&root(), &table, &comment(3, Some(2)))
        .await
        .unwrap();
    txn.update(&root(), &table, &comment(1, Some(3)))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &table).await, vec![1, 2, 3]);

    let txn = store.begin().await.unwrap();
    txn.delete(&root(), &table, &[Value::U64(1)]).await.unwrap();
    txn.commit().await.unwrap();
    assert!(
        ids(&store, &table).await.is_empty(),
        "the whole cycle should go, exactly once round"
    );
}

/// `RESTRICT` asks whether anything *outside the delete* still references the
/// row, not whether anything references it at all.
///
/// Both edges here point at the same parent from the same child table, so a
/// row can be scheduled by the cascade and found by the restrict check in one
/// delete. Checking them as they are met rather than in two passes would make
/// the answer depend on which edge the walk happened to take first.
#[tokio::test]
async fn restrict_ignores_a_row_the_same_delete_is_removing() {
    let catalog = Catalog::from_tables([dossiers(), notes()]).unwrap();
    let store = store(catalog);
    let dossiers_table = dossiers();
    let notes_table = notes();
    let note = |id: u64, dossier: Option<u64>, filed: Option<u64>| {
        Row::new(vec![
            Value::U64(id),
            dossier.map_or(Value::Null, Value::U64),
            filed.map_or(Value::Null, Value::U64),
        ])
    };

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &dossiers_table, &Row::new(vec![Value::U64(1)]))
        .await
        .unwrap();
    txn.insert(&root(), &dossiers_table, &Row::new(vec![Value::U64(2)]))
        .await
        .unwrap();
    // Note 100 is cascaded away by the delete of dossier 1 *and* references it
    // through the restricting edge.
    txn.insert(&root(), &notes_table, &note(100, Some(1), Some(1)))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    txn.delete(&root(), &dossiers_table, &[Value::U64(1)])
        .await
        .expect("a referencing row that is itself being deleted must not block");
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &dossiers_table).await, vec![2]);
    assert!(ids(&store, &notes_table).await.is_empty());

    // And the exemption is exactly that narrow: a note the cascade does not
    // reach still blocks.
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &notes_table, &note(200, None, Some(2)))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let refused = txn
        .delete(&root(), &dossiers_table, &[Value::U64(2)])
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            KernelError::Schema(SchemaError::ForeignKeyRestricted { .. })
        ),
        "a row outside the delete must still block it"
    );
}

// --- foreign keys versus row-level security ---------------------------------

fn secured_library() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("writer", AUTHORS, Action::ALL))
        .grant(Grant::new("writer", BOOKS, Action::ALL))
        // Only authors whose name starts with a vowel are visible. An arbitrary
        // rule, chosen so the hidden and visible sets are both non-empty and
        // neither is "everything the caller wrote".
        .policy(Policy::new(
            "vowels_only",
            AUTHORS,
            Action::ALL,
            |_: &SecurityContext| Expr::like(Ordinal(1), "A%"),
        ))
}

fn writer() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("writer"))
}

/// The leak this constraint could have been: an insert that succeeds for one
/// key and fails for another is an answer to "does this row exist?".
///
/// The parent is read through the ordinary secured path, so a row the policy
/// hides is absent *for this caller* and reports identically to one that was
/// never written. The assertion is that the two errors are the same error, not
/// merely that both are errors.
#[tokio::test]
async fn a_foreign_key_cannot_be_used_to_probe_for_hidden_rows() {
    let store = RecordStore::new(
        MemoryStore::new(),
        library(ReferentialAction::Restrict),
        secured_library(),
    );
    let authors_table = authors();
    let books_table = books(ReferentialAction::Restrict);

    // Seeded as superuser: the point is a row that exists and is hidden, which
    // the caller under test could not have created.
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &authors_table, &author(1, "Ada", Some(36)))
        .await
        .unwrap();
    txn.insert(&root(), &authors_table, &author(2, "Grace", Some(45)))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let hidden = txn
        .insert(&writer(), &books_table, &book(10, Some(2), "Hidden parent"))
        .await
        .unwrap_err();
    let absent = txn
        .insert(&writer(), &books_table, &book(11, Some(99), "No parent"))
        .await
        .unwrap_err();
    assert_eq!(
        format!("{hidden}"),
        format!("{absent}"),
        "referencing a hidden row must be indistinguishable from referencing a missing one"
    );
    assert!(matches!(
        hidden,
        KernelError::Schema(SchemaError::ForeignKeyViolation { .. })
    ));

    // Not vacuous: the visible parent is accepted by the same caller in the
    // same transaction, so the two refusals above are about visibility rather
    // than about the policy refusing everything.
    txn.insert(
        &writer(),
        &books_table,
        &book(12, Some(1), "Visible parent"),
    )
    .await
    .expect("a parent the caller can read is a parent they can reference");
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &books_table).await, vec![12]);
}

/// Referencing a table is reading it, grant included.
///
/// The alternative — a privileged read on behalf of a caller who may not read
/// the parent at all — is exactly the oracle the test above rules out.
#[tokio::test]
async fn referencing_a_table_requires_being_allowed_to_read_it() {
    let store = RecordStore::new(
        MemoryStore::new(),
        library(ReferentialAction::Restrict),
        SecurityCatalog::new().grant(Grant::new("writer", BOOKS, Action::ALL)),
    );
    let books_table = books(ReferentialAction::Restrict);

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &authors(), &author(1, "Ada", Some(36)))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert(&writer(), &books_table, &book(10, Some(1), "Notes"))
        .await
        .unwrap_err();
    assert!(
        matches!(refused, KernelError::AccessDenied { ref table, action: "read", .. }
                 if table == "authors"),
        "got {refused:?}"
    );
    // A book that references nothing needs no such grant, so the refusal is
    // about the reference rather than about the table.
    txn.insert(&writer(), &books_table, &book(11, None, "Anonymous"))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(ids(&store, &books_table).await, vec![11]);
}

/// A tenant-scoped parent's key begins with the tenant, so a reference across
/// tenants is not a policy decision — it is a key that cannot be formed.
#[tokio::test]
async fn a_reference_cannot_cross_a_tenant_boundary() {
    let security = SecurityCatalog::new()
        .grant(Grant::new("member", ORGS, Action::ALL))
        .grant(Grant::new("member", MEMBERS, Action::ALL));
    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([orgs(), members()]).unwrap(),
        security,
    );
    let orgs_table = orgs();
    let members_table = members();
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(2);

    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &orgs_table,
        &Row::new(vec![
            Value::Uuid(b),
            Value::U64(7),
            Value::Str("theirs".into()),
        ]),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let in_a = SecurityContext::new(
        Principal::new(Value::Uuid(a))
            .with_tenant(Value::Uuid(a))
            .with_role("member"),
    );
    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert(
            &in_a,
            &members_table,
            &Row::new(vec![
                Value::Uuid(a),
                Value::U64(1),
                Value::U64(7),
                Value::Uuid(a),
            ]),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            KernelError::Schema(SchemaError::ForeignKeyViolation { .. })
        ),
        "org 7 exists, but only in another tenant"
    );

    // The same org id inside the caller's own tenant is a different key, and
    // works — which is what makes the refusal above about the tenant.
    txn.insert(
        &root(),
        &orgs_table,
        &Row::new(vec![
            Value::Uuid(a),
            Value::U64(7),
            Value::Str("ours".into()),
        ]),
    )
    .await
    .unwrap();
    txn.insert(
        &in_a,
        &members_table,
        &Row::new(vec![
            Value::Uuid(a),
            Value::U64(1),
            Value::U64(7),
            Value::Uuid(a),
        ]),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
}

/// The other half of the trade, pinned rather than left to be discovered.
///
/// Finding the rows that reference a deleted row ignores row policy, because a
/// child hidden from the deleter would otherwise be left pointing at nothing.
/// So a cascade does remove rows the caller cannot see. That is the schema
/// author's declared intent, and the delete grant on the child table is what
/// bounds it.
#[tokio::test]
async fn a_cascade_reaches_rows_the_caller_cannot_see_but_only_with_a_grant() {
    let visible_only = |_: &SecurityContext| Expr::compare(Ordinal(0), CmpOp::Lt, Value::U64(11));
    let security = SecurityCatalog::new()
        .grant(Grant::new("writer", AUTHORS, Action::ALL))
        .grant(Grant::new("writer", BOOKS, Action::ALL))
        // `reviews` cascades from `books`, so it is reachable and the grant is
        // required whether or not any review exists — see `cascade_reachable`.
        .grant(Grant::new("writer", REVIEWS, Action::ALL))
        .grant(Grant::new("reader", AUTHORS, Action::ALL))
        .policy(Policy::new("low_ids", BOOKS, Action::ALL, visible_only));
    let store = RecordStore::new(
        MemoryStore::new(),
        library(ReferentialAction::Cascade),
        security,
    );
    let authors_table = authors();
    let books_table = books(ReferentialAction::Cascade);
    seed_library(&store, ReferentialAction::Cascade).await;

    let writer = SecurityContext::new(Principal::new(Value::U64(1)).with_role("writer"));
    let txn = store.begin().await.unwrap();
    let seen = txn
        .query(&writer, &books_table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(seen.len(), 1, "premise: book 11 is hidden from this caller");

    txn.delete(&writer, &authors_table, &[Value::U64(1)])
        .await
        .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(
        ids(&store, &books_table).await,
        vec![12],
        "the hidden child must go too, or it would point at nothing"
    );

    // Without delete on the referencing table, the cascade is refused outright
    // rather than quietly skipping it.
    let store = RecordStore::new(
        MemoryStore::new(),
        library(ReferentialAction::Cascade),
        SecurityCatalog::new().grant(Grant::new("reader", AUTHORS, Action::ALL)),
    );
    seed_library(&store, ReferentialAction::Cascade).await;
    let reader = SecurityContext::new(Principal::new(Value::U64(2)).with_role("reader"));
    let txn = store.begin().await.unwrap();
    let refused = txn
        .delete(&reader, &authors_table, &[Value::U64(1)])
        .await
        .unwrap_err();
    assert!(
        matches!(refused, KernelError::AccessDenied { ref table, .. } if table == "books"),
        "got {refused:?}"
    );

    // And the refusal is decided by the schema, not by the data: the same
    // caller is refused deleting an author with no books at all, so the error
    // cannot be read as "something references this row".
    let empty = RecordStore::new(
        MemoryStore::new(),
        library(ReferentialAction::Cascade),
        SecurityCatalog::new().grant(Grant::new("reader", AUTHORS, Action::ALL)),
    );
    let txn = empty.begin().await.unwrap();
    txn.insert(&root(), &authors_table, &author(9, "Solo", Some(30)))
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let txn = empty.begin().await.unwrap();
    assert!(matches!(
        txn.delete(&reader, &authors_table, &[Value::U64(9)]).await,
        Err(KernelError::AccessDenied { .. })
    ));
}

// --- defaults through the store ---------------------------------------------

#[tokio::test]
async fn an_unset_column_is_written_as_its_default() {
    let table = TableDef::builder("items", TableId(20))
        .column("id", ValueType::U64)
        .column("status", ValueType::Str)
        .primary_key(["id"])
        .default_for("status", Value::Str("new".into()))
        .build()
        .unwrap();
    let store = store(Catalog::from_tables([table.clone()]).unwrap());
    let status = table.ordinal_of("status").unwrap();

    let txn = store.begin().await.unwrap();
    txn.insert_partial(
        &root(),
        &table,
        PartialRow::for_table(&table).set(Ordinal(0), Value::U64(1)),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let row = txn.get(&root(), &table, &[Value::U64(1)]).await.unwrap();
    assert_eq!(
        row.unwrap().get(status),
        Some(&Value::Str("new".into())),
        "the default should be stored, not merely implied on read"
    );
}

// --- migrations through the store -------------------------------------------

/// Rows written under one schema, read under the next, over the same bytes.
///
/// The store is reopened with a different catalog rather than the same one,
/// which is what a deployment does: the data does not change, the code does.
#[tokio::test]
async fn rows_survive_a_column_being_added_dropped_and_renamed() {
    let v1 = TableDef::builder("t", TableId(30))
        .column("id", ValueType::U64)
        .column("legacy", ValueType::Str)
        .column("name", ValueType::Str)
        .primary_key(["id"])
        .schema_version(1)
        .build()
        .unwrap();
    let v2 = TableDef::builder("t", TableId(30))
        .column("id", ValueType::U64)
        .column("legacy", ValueType::Str)
        .column("full_name", ValueType::Str)
        .added_column_with_default("tier", ValueType::Str, 2, Value::Str("free".into()))
        .primary_key(["id"])
        .schema_version(2)
        .drop_column("legacy", 2)
        .renamed_column("full_name", "name")
        .build()
        .unwrap();

    let backend = MemoryStore::new();
    let old = RecordStore::new(
        backend.clone(),
        Catalog::from_tables([v1.clone()]).unwrap(),
        SecurityCatalog::new(),
    );
    let txn = old.begin().await.unwrap();
    txn.insert(
        &root(),
        &v1,
        &Row::new(vec![
            Value::U64(1),
            Value::Str("junk".into()),
            Value::Str("Ada".into()),
        ]),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let new = RecordStore::new(
        backend.clone(),
        Catalog::from_tables([v2.clone()]).unwrap(),
        SecurityCatalog::new(),
    );
    let txn = new.begin().await.unwrap();
    let row = txn
        .get(&root(), &v2, &[Value::U64(1)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row,
        Row::new(vec![
            Value::U64(1),
            // Dropped: reads as null even though its bytes are still there.
            Value::Null,
            // Renamed: same ordinal, same bytes, new name.
            Value::Str("Ada".into()),
            // Added: never stored for this row, supplied by the default.
            Value::Str("free".into()),
        ]),
        "the columns after the dropped one must not be read out of its bytes"
    );
    assert_eq!(v2.ordinal_of("name"), v2.ordinal_of("full_name"));

    // A row written under v2 round-trips, and the old row is still readable
    // afterwards: the two versions coexist in one table.
    txn.insert(
        &root(),
        &v2,
        &Row::new(vec![
            Value::U64(2),
            Value::Null,
            Value::Str("Grace".into()),
            Value::Str("paid".into()),
        ]),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let txn = new.begin().await.unwrap();
    let rows = txn
        .query(&root(), &v2, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get(Ordinal(3)), Some(&Value::Str("free".into())));
    assert_eq!(rows[1].get(Ordinal(3)), Some(&Value::Str("paid".into())));
    assert_eq!(rows[1].get(Ordinal(2)), Some(&Value::Str("Grace".into())));

    // And a filter on the renamed column still reaches it, because a rename
    // changes no ordinal and no byte.
    let txn = new.begin().await.unwrap();
    let matching = txn
        .query(
            &root(),
            &v2,
            Expr::eq(v2.ordinal_of("name").unwrap(), Value::Str("Ada".into())),
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(matching.len(), 1);
}

/// The cascade limit fires, and fires before the allocator does.
///
/// A cascade is held in memory and committed atomically, so "how many rows"
/// has to have an answer. The limit is not what makes a cycle terminate — that
/// is the scheduled set, tested above — it is what stops one delete from
/// pulling an unbounded amount of the database into one transaction.
#[tokio::test]
async fn a_cascade_larger_than_the_limit_is_refused() {
    let store = store(library(ReferentialAction::Cascade));
    let authors_table = authors();
    let books_table = books(ReferentialAction::Cascade);

    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &authors_table, &author(1, "Ada", Some(36)))
        .await
        .unwrap();
    let batch: Vec<Row> = (0..=slate_kernel::record::CASCADE_LIMIT as u64)
        .map(|i| book(i, Some(1), "t"))
        .collect();
    txn.insert_many(&root(), &books_table, &batch)
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let refused = txn
        .delete(&root(), &authors_table, &[Value::U64(1)])
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            KernelError::Schema(SchemaError::CascadeTooLarge { .. })
        ),
        "got {refused:?}"
    );
    txn.commit().await.unwrap();
    assert_eq!(
        ids(&store, &authors_table).await,
        vec![1],
        "a refused cascade must remove nothing"
    );
}

// --- a cascade is atomic ----------------------------------------------------

/// Wraps a store and fails the `n`th write of a transaction.
///
/// The same device as `tests/crash.rs`, kept separate because that file sweeps
/// the write path and this one sweeps a delete that fans out. A cascade is the
/// case where "one transaction" stops being obvious: several rows in several
/// tables, decided by a graph walk.
struct Faulty {
    inner: MemoryStore,
    countdown: Arc<AtomicUsize>,
}

impl Faulty {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            countdown: Arc::new(AtomicUsize::new(usize::MAX)),
        }
    }

    fn fail_after(&self, n: usize) {
        self.countdown.store(n, Ordering::SeqCst);
    }

    fn never_fail(&self) {
        self.countdown.store(usize::MAX, Ordering::SeqCst);
    }

    fn spent(&self, budget: usize) -> usize {
        budget - self.countdown.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl KvStore for Faulty {
    async fn begin(&self) -> Result<Box<dyn KvTransaction + Send + '_>> {
        Ok(Box::new(FaultyTxn {
            inner: self.inner.begin().await?,
            countdown: Arc::clone(&self.countdown),
        }))
    }
}

struct FaultyTxn<'a> {
    inner: Box<dyn KvTransaction + Send + 'a>,
    countdown: Arc<AtomicUsize>,
}

impl FaultyTxn<'_> {
    fn charge(&self) -> Result<()> {
        let remaining = self.countdown.load(Ordering::SeqCst);
        if remaining == usize::MAX {
            return Ok(());
        }
        if remaining == 0 {
            return Err(KernelError::Storage(StorageError::new(
                std::io::Error::other("injected write failure"),
            )));
        }
        self.countdown.store(remaining - 1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl KvSnapshot for FaultyTxn<'_> {
    async fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.inner.get(key).await
    }

    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> Result<Box<dyn KvIterator + Send + '_>> {
        self.inner.scan(range, order).await
    }

    fn is_point_in_time(&self) -> bool {
        self.inner.is_point_in_time()
    }
}

#[async_trait]
impl KvTransaction for FaultyTxn<'_> {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        self.charge()?;
        self.inner.put(key, value)
    }

    fn delete(&self, key: Vec<u8>) -> Result<()> {
        self.charge()?;
        self.inner.delete(key)
    }

    async fn commit(self: Box<Self>) -> Result<Option<u64>> {
        self.charge()?;
        self.inner.commit().await
    }

    fn rollback(self: Box<Self>) {
        self.inner.rollback();
    }
}

async fn seed_faulty(store: &RecordStore<Faulty>) {
    let txn = store.begin().await.unwrap();
    txn.insert(&root(), &authors(), &author(1, "Ada", Some(36)))
        .await
        .unwrap();
    for id in [10u64, 11, 12] {
        txn.insert(
            &root(),
            &books(ReferentialAction::Cascade),
            &book(id, Some(1), "t"),
        )
        .await
        .unwrap();
    }
    for id in [100u64, 101] {
        txn.insert(
            &root(),
            &reviews(ReferentialAction::Cascade),
            &review(id, Some(10), 5),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
}

async fn surviving(store: &RecordStore<Faulty>, table: &TableDef) -> usize {
    let txn = store.begin().await.unwrap();
    txn.query(&root(), table, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .len()
}

/// A delete and everything it cascades to land together or not at all.
///
/// Swept at every write position, because the interesting failures are between
/// the row and its index entries, and between one table's rows and the next
/// table's. A half-applied cascade is a review pointing at a book that is gone
/// — which reads as a correct empty result to everyone who looks.
#[tokio::test]
async fn a_cascading_delete_lands_wholly_or_not_at_all() {
    // Counted rather than assumed, so the sweep still covers everything if the
    // write path gains a step.
    let writes = {
        let store = RecordStore::new(
            Faulty::new(),
            library(ReferentialAction::Cascade),
            SecurityCatalog::new(),
        );
        seed_faulty(&store).await;
        const BUDGET: usize = 1_000;
        store.backend().fail_after(BUDGET);
        let txn = store.begin().await.unwrap();
        txn.delete(&root(), &authors(), &[Value::U64(1)])
            .await
            .unwrap();
        txn.commit().await.unwrap();
        store.backend().spent(BUDGET)
    };
    assert!(
        (4..64).contains(&writes),
        "a cascade over 1 author, 3 books and 2 reviews took {writes} writes, \
         which is not a plausible number"
    );

    for fail_at in 0..writes {
        let store = RecordStore::new(
            Faulty::new(),
            library(ReferentialAction::Cascade),
            SecurityCatalog::new(),
        );
        seed_faulty(&store).await;
        store.backend().fail_after(fail_at);

        let txn = store.begin().await.unwrap();
        let deleted = txn.delete(&root(), &authors(), &[Value::U64(1)]).await;
        let committed = if deleted.is_ok() {
            txn.commit().await.is_ok()
        } else {
            false
        };
        store.backend().never_fail();

        let context = format!("cascade failing at write {fail_at}");
        let authors_left = surviving(&store, &authors()).await;
        let books_left = surviving(&store, &books(ReferentialAction::Cascade)).await;
        let reviews_left = surviving(&store, &reviews(ReferentialAction::Cascade)).await;

        let expected = if committed { (0, 0, 0) } else { (1, 3, 2) };
        assert_eq!(
            (authors_left, books_left, reviews_left),
            expected,
            "{context}: the cascade was partly applied"
        );

        // Index entries have to match the rows, not just the row count: an
        // entry left behind for a deleted book is a lookup that returns a row
        // nobody can read.
        assert_eq!(
            index_entry_count(&store.backend().inner, &authors()),
            authors_left,
            "{context}: authors index entries"
        );
        assert_eq!(
            index_entry_count(&store.backend().inner, &books(ReferentialAction::Cascade)),
            books_left,
            "{context}: books index entries"
        );
    }
}

/// The sweep above passes on a store that never fails, so this proves the
/// injector fires and that the cascade really does write more than once.
#[tokio::test]
async fn the_fault_injector_injects_faults() {
    let store = RecordStore::new(
        Faulty::new(),
        library(ReferentialAction::Cascade),
        SecurityCatalog::new(),
    );
    seed_faulty(&store).await;
    store.backend().fail_after(0);

    let txn = store.begin().await.unwrap();
    let outcome = txn.delete(&root(), &authors(), &[Value::U64(1)]).await;
    assert!(
        outcome.is_err(),
        "the first write succeeded with the fault armed at zero"
    );
    drop(txn);

    store.backend().never_fail();
    let txn = store.begin().await.unwrap();
    txn.delete(&root(), &authors(), &[Value::U64(1)])
        .await
        .expect("the same delete must succeed once the fault is disarmed");
    txn.commit().await.unwrap();
    assert_eq!(surviving(&store, &authors()).await, 0);
}

/// A guard against the cheapest way for all of the above to be vacuous.
///
/// Every foreign-key test asserts that something is refused. If the constraints
/// were never consulted at all, most of them would fail — but the reverse
/// mistake, a store that refuses everything, would pass a surprising number. So
/// this asserts the ordinary case works, unadorned.
#[tokio::test]
async fn an_ordinary_write_is_not_refused_by_any_of_this() {
    let store = store(library(ReferentialAction::Cascade));
    seed_library(&store, ReferentialAction::Cascade).await;
    let txn = store.begin().await.unwrap();
    txn.insert(
        &root(),
        &reviews(ReferentialAction::Cascade),
        &review(100, Some(10), 4),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(ids(&store, &authors()).await, vec![1, 2]);
    assert_eq!(
        ids(&store, &books(ReferentialAction::Cascade)).await,
        vec![10, 11, 12]
    );
    assert_eq!(
        ids(&store, &reviews(ReferentialAction::Cascade)).await,
        vec![100]
    );

    // `Query::all()` over each table, so an accidental filter would show up.
    let txn = store.begin().await.unwrap();
    assert_eq!(
        txn.count(&root(), &authors(), &Query::all()).await.unwrap(),
        2
    );
}

/// A pointer to what `RecordTransaction` promises about the two, so the
/// signature is exercised rather than only documented.
#[tokio::test]
async fn a_table_with_no_constraints_is_unaffected() {
    let plain = TableDef::builder("plain", TableId(40))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap();
    assert!(plain.checks().is_empty());
    assert!(plain.foreign_keys().is_empty());

    let store = store(Catalog::from_tables([plain.clone()]).unwrap());
    let txn: RecordTransaction<'_> = store.begin().await.unwrap();
    txn.insert(&root(), &plain, &Row::new(vec![Value::U64(1)]))
        .await
        .unwrap();
    assert!(txn.delete(&root(), &plain, &[Value::U64(1)]).await.unwrap());
    txn.commit().await.unwrap();
    assert!(ids(&store, &plain).await.is_empty());
}

#[tokio::test]
async fn a_check_violation_carries_the_column_and_message_it_was_given() {
    // The error a form reads. Without these it names the check and the table,
    // which tells a caller which *rule* fired and not which *field* to put the
    // message beside — and the only way to get from one to the other is to
    // parse a name the caller chose, which breaks the day somebody renames it.
    let store = store(library(ReferentialAction::Restrict));
    let table = authors();

    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert(&root(), &table, &author(1, "Ada", Some(-1)))
        .await
        .unwrap_err();
    let KernelError::Schema(SchemaError::CheckViolation {
        check,
        column,
        message,
        ..
    }) = &refused
    else {
        panic!("got {refused:?}");
    };
    assert_eq!(check, "age_positive");
    assert_eq!(column.as_deref(), Some("age"));
    assert_eq!(message.as_deref(), Some("Age must be greater than zero."));
    // The message is in the rendered text too, because that is what a log and
    // a bare `Display` caller see.
    assert!(
        refused
            .to_string()
            .contains("Age must be greater than zero."),
        "{refused}"
    );
}

#[tokio::test]
async fn a_check_with_no_column_or_message_reports_neither() {
    // The default, and it must stay absent rather than become an empty string:
    // a form told the column is "" would render the error beside a field whose
    // name is the empty string, which is worse than being told nothing.
    let table = TableDef::builder("plain", TableId(77))
        .column("id", ValueType::U64)
        .nullable_column("age", ValueType::I64)
        .primary_key(["id"])
        .check(CheckDef::new(
            "age_positive",
            Expr::compare(Ordinal(1), CmpOp::Gt, Value::I64(0)),
        ))
        .build()
        .expect("valid schema");
    let store = store(Catalog::from_tables([table.clone()]).expect("catalog"));

    let txn = store.begin().await.unwrap();
    let refused = txn
        .insert(
            &root(),
            &table,
            &Row::new(vec![Value::U64(1), Value::I64(-1)]),
        )
        .await
        .unwrap_err();
    let KernelError::Schema(SchemaError::CheckViolation {
        column, message, ..
    }) = &refused
    else {
        panic!("got {refused:?}");
    };
    assert_eq!(*column, None);
    assert_eq!(*message, None);
}
