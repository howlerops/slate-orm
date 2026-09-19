//! A table that retires rows instead of removing them.
//!
//! Four claims, and the last is the one that would be a silent bug rather than
//! a visible one:
//!
//! - **`delete` stamps and keeps.** The row is still in the keyspace and is no
//!   longer returned. Checked by reading it back with `include_deleted`, which
//!   is the only way to tell "retired" from "gone" from outside.
//! - **Every read path hides it.** A point get, a scan, a filtered scan and a
//!   `COUNT(*)` all have to agree, because they reach rows by different routes
//!   and the filter is installed in only one place. If that one place is the
//!   wrong place, one of these four still finds the row.
//! - **A cascade retires rather than removes.** The delete closure walks to the
//!   child and calls the same function, so a soft-deleting child of a
//!   hard-deleting parent keeps its rows. This falls out of where the decision
//!   was put, and the test is what says the placement holds.
//! - **An index-only scan does not resurrect it.** The dangerous one. A covering
//!   scan answers from index keys without reading rows, so a filter the index
//!   cannot evaluate has to prevent the scan being chosen at all rather than
//!   being skipped by it. Nothing in the soft-delete code arranges this — it is
//!   inherited from conjoining the filter before planning — which is exactly
//!   why it needs a test: nobody would think to break it, and a refactor that
//!   moved the filter later would.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::clock::FixedClock;
use slate_kernel::memory::MemoryStore;
use slate_kernel::{Aggregate, Projection, Query, RecordStore, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, IndexDef, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;

const DOCS: TableId = TableId(1);
const NOTES: TableId = TableId(2);

/// `id`, `kind`, `deleted_at` — with an index on `kind` that does not carry
/// `deleted_at`, which is what makes the covering-scan test meaningful.
fn docs() -> TableDef {
    TableDef::builder("docs", DOCS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .nullable_column("deleted_at", ValueType::I64)
        .primary_key(["id"])
        .soft_delete("deleted_at")
        .index(IndexDef::builder("by_kind", slate_schema::IndexId(1)).column("kind"))
        .build()
        .expect("a valid table")
}

fn catalog() -> Catalog {
    Catalog::from_tables([docs()]).expect("a catalog the schema layer accepts")
}

fn store_at(seconds: i64) -> RecordStore<MemoryStore> {
    RecordStore::new(MemoryStore::new(), catalog(), SecurityCatalog::new())
        .with_clock(Arc::new(FixedClock::at(seconds)))
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn doc(id: u64, kind: &str) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(kind.to_owned()),
        Value::Null,
    ])
}

/// Insert three rows and retire the middle one.
async fn seeded() -> RecordStore<MemoryStore> {
    let store = store_at(5_000);
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    for (id, kind) in [(1_u64, "note"), (2, "note"), (3, "memo")] {
        txn.insert(&root(), table, &doc(id, kind))
            .await
            .expect("insert");
    }
    assert!(
        txn.delete(&root(), table, &[Value::U64(2)])
            .await
            .expect("delete"),
        "the row was there to delete"
    );
    txn.commit().await.expect("commit");
    store
}

/// Ids a query returns.
async fn ids(store: &RecordStore<MemoryStore>, query: Query) -> Vec<u64> {
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let cursor = txn.execute(&root(), table, &query).await.expect("query");
    let rows = cursor.collect().await.expect("rows");
    rows.into_iter()
        .map(|row| match row.values()[0] {
            Value::U64(id) => id,
            ref other => panic!("id is {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_deleted_row_is_stamped_and_kept_rather_than_removed() {
    let store = seeded().await;

    assert_eq!(ids(&store, Query::all()).await, vec![1, 3]);

    // Still there, which is the difference between this and a delete.
    let all = Query {
        include_deleted: true,
        ..Query::all()
    };
    assert_eq!(ids(&store, all).await, vec![1, 2, 3]);

    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let cursor = txn
        .execute(
            &root(),
            table,
            &Query {
                include_deleted: true,
                ..Query::all()
            },
        )
        .await
        .expect("query");
    let rows = cursor.collect().await.expect("rows");
    let retired = rows
        .iter()
        .find(|row| row.values()[0] == Value::U64(2))
        .expect("the retired row");
    assert_eq!(
        retired.values()[2],
        Value::I64(5_000),
        "stamped with the clock's time, not a sentinel"
    );
}

#[tokio::test]
async fn a_superuser_does_not_see_a_deleted_row_either() {
    // `root()` *is* a superuser, and `row_filter` returns `Expr::True` for one
    // — so if the conjunct were added after that early return, every test above
    // would still pass and every superuser read would show deleted rows. The
    // seeder, the migration runner and `--seed` are all superusers.
    let store = seeded().await;
    assert_eq!(ids(&store, Query::all()).await, vec![1, 3]);
}

#[tokio::test]
async fn a_point_get_hides_a_deleted_row() {
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    assert!(
        txn.get(&root(), table, &[Value::U64(2)])
            .await
            .expect("get")
            .is_none(),
        "a retired row is not gettable by key"
    );
    assert!(
        txn.get(&root(), table, &[Value::U64(1)])
            .await
            .expect("get")
            .is_some(),
        "a live row still is"
    );
}

#[tokio::test]
async fn deleting_a_row_twice_is_not_an_error_and_does_not_restamp() {
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    // The row is invisible to `delete` for the same reason it is invisible to
    // `get`, so this reports "nothing to delete" rather than stamping again
    // and moving the time of death.
    assert!(
        !txn.delete(&root(), table, &[Value::U64(2)])
            .await
            .expect("delete"),
        "a retired row is already gone as far as delete is concerned"
    );
}

#[tokio::test]
async fn an_aggregate_counts_the_same_rows_the_scan_returns() {
    // A grouped read rebuilds its inner query rather than reusing the caller's,
    // so it is a separate chance to drop the flag. Dropping it makes COUNT(*)
    // disagree with the list beside it, which is the kind of wrong that gets
    // believed.
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");

    let counted = |query| {
        let txn = &txn;
        async move {
            let values = txn
                .aggregate(&root(), table, &query, &[Aggregate::Count])
                .await
                .expect("aggregate");
            match values[0] {
                Value::U64(n) => n,
                ref other => panic!("count is {other:?}"),
            }
        }
    };

    assert_eq!(counted(Query::all()).await, 2, "live rows only");
    assert_eq!(
        counted(Query {
            include_deleted: true,
            ..Query::all()
        })
        .await,
        3,
        "and all three when asked"
    );
}

#[tokio::test]
async fn an_index_only_scan_does_not_answer_from_keys_and_resurrect_a_row() {
    // `by_kind` covers `kind` and the primary key, so a projection of just
    // those two is exactly the shape the planner wants to answer from index
    // entries alone — and the retired row still *has* an index entry, because
    // the index is not partial. The filter mentions `deleted_at`, which the
    // index does not carry, so the covering scan must not be chosen.
    let store = seeded().await;
    let projected = Query {
        projection: Projection::Columns(
            [slate_schema::Ordinal(0), slate_schema::Ordinal(1)].into(),
        ),
        ..Query::all()
    };
    assert_eq!(
        ids(&store, projected).await,
        vec![1, 3],
        "a covering scan must not see through the soft-delete filter"
    );
}

#[tokio::test]
async fn a_partial_index_on_not_deleted_frees_its_entry_when_a_row_is_retired() {
    // The gap table claimed partial indexes "support `WHERE deleted_at IS NULL`
    // well". This is that claim, run: the retired row stops being admitted, so
    // its entry goes, and a unique index of this shape lets the key be reused.
    let table = TableDef::builder("notes", NOTES)
        .column("id", ValueType::U64)
        .column("slug", ValueType::Str)
        .nullable_column("deleted_at", ValueType::I64)
        .primary_key(["id"])
        .soft_delete("deleted_at")
        .index(
            IndexDef::builder("live_slug", slate_schema::IndexId(1))
                .column("slug")
                .unique()
                .only_where(slate_kernel::Expr::IsNull {
                    column: slate_schema::Ordinal(2),
                    negated: false,
                }),
        )
        .build()
        .expect("a valid table");
    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([table]).expect("a catalog"),
        SecurityCatalog::new(),
    )
    .with_clock(Arc::new(FixedClock::at(9_000)));

    let row = |id: u64| {
        Row::new(vec![
            Value::U64(id),
            Value::Str("x".to_owned()),
            Value::Null,
        ])
    };

    let txn = store.begin().await.expect("a transaction");
    let notes = txn.catalog().table_by_name("notes").expect("the table");
    txn.insert(&root(), notes, &row(1)).await.expect("insert");
    // The slot is taken while the row is live.
    assert!(
        txn.insert(&root(), notes, &row(2)).await.is_err(),
        "a unique slug is unique among live rows"
    );
    txn.delete(&root(), notes, &[Value::U64(1)])
        .await
        .expect("delete");
    // And free once it is retired, because the partial predicate stops
    // admitting the row and the entry is deleted with it.
    txn.insert(&root(), notes, &row(2))
        .await
        .expect("the slug is free once the holder is retired");
    txn.commit().await.expect("commit");
}

/// A table declaring `soft_delete` on a column shaped by `build`.
fn declared(
    build: impl FnOnce(slate_schema::TableBuilder) -> slate_schema::TableBuilder,
) -> String {
    let base = TableDef::builder("t", TableId(9))
        .column("id", ValueType::U64)
        .primary_key(["id"]);
    match build(base).soft_delete("marker").build() {
        Ok(_) => panic!("this table should have been refused"),
        Err(why) => why.to_string(),
    }
}

#[test]
fn a_soft_delete_column_that_cannot_hold_a_time_is_refused() {
    let why = declared(|t| t.nullable_column("marker", ValueType::Str));
    assert!(why.contains("Str"), "{why}");
    assert!(why.contains("i64"), "{why}");
}

#[test]
fn a_soft_delete_column_that_is_not_nullable_is_refused() {
    // The rule that catches the likely mistake. Without it the column has no
    // value meaning "not deleted", so a table would go silently empty the
    // moment it was declared — which is the worst possible failure mode and
    // the one a type check alone does not catch.
    let why = declared(|t| t.column("marker", ValueType::I64));
    assert!(why.contains("not nullable"), "{why}");
}

#[test]
fn a_soft_delete_column_in_the_primary_key_is_refused_by_a_rule_that_was_already_there() {
    // This had a dedicated `SoftDeleteInKey` check until a mutation showed it
    // could never fire. A soft-delete column must be nullable, a primary key
    // column may not be nullable, and `NullablePrimaryKey` already says so —
    // so the combination is unreachable from both directions and the extra
    // check was a branch no input could take. It is asserted here rather than
    // re-added: the case must stay refused, by whatever rule does it.
    let why = match TableDef::builder("t", TableId(9))
        .column("id", ValueType::U64)
        .nullable_column("marker", ValueType::I64)
        .primary_key(["id", "marker"])
        .soft_delete("marker")
        .build()
    {
        Ok(_) => panic!("this table should have been refused"),
        Err(why) => why.to_string(),
    };
    assert!(why.contains("nullable"), "{why}");
}

#[test]
fn a_soft_delete_column_that_is_also_managed_is_refused() {
    let why = declared(|t| {
        t.nullable_column("marker", ValueType::I64)
            .managed_for("marker", slate_schema::Managed::UpdatedAt)
    });
    // The managed rules run first and refuse a nullable managed column, so
    // this asserts only that the pair is refused, not which rule caught it.
    assert!(!why.is_empty(), "some refusal");
}

#[tokio::test]
async fn an_ordinary_caller_does_not_see_a_deleted_row() {
    // Every other read here runs as a superuser, which returns from
    // `row_filter` early — so the conjunction onto the tenant-and-policy
    // filter, which is the path almost every real read takes, had no test at
    // all until a surviving mutation said so.
    let catalog = Catalog::from_tables([docs()]).expect("a catalog");
    let store = RecordStore::new(
        MemoryStore::new(),
        catalog,
        SecurityCatalog::new().grant(slate_kernel::Grant::new(
            "reader",
            DOCS,
            slate_kernel::Action::ALL,
        )),
    )
    .with_clock(Arc::new(FixedClock::at(7_000)));

    let caller =
        SecurityContext::new(slate_kernel::Principal::new(Value::U64(1)).with_role("reader"));

    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    for (id, kind) in [(1_u64, "note"), (2, "note")] {
        txn.insert(&caller, table, &doc(id, kind))
            .await
            .expect("insert");
    }
    txn.delete(&caller, table, &[Value::U64(2)])
        .await
        .expect("delete");

    let cursor = txn
        .execute(&caller, table, &Query::all())
        .await
        .expect("query");
    let rows = cursor.collect().await.expect("rows");
    let ids: Vec<u64> = rows
        .iter()
        .map(|row| match row.values()[0] {
            Value::U64(id) => id,
            ref other => panic!("id is {other:?}"),
        })
        .collect();
    assert_eq!(
        ids,
        vec![1],
        "the retired row is hidden from an ordinary caller too"
    );
}
