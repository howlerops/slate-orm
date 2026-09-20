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

// --- purge ------------------------------------------------------------------

/// One live row and three retired at t=1000, t=5000 and t=9000.
///
/// Aged by moving the clock between deletes, which is what `FixedClock::set`
/// is for. Inserting rows with `deleted_at` already stamped was the first
/// attempt and is *refused* — the write path checks that the writer could read
/// back what it just wrote, and an already-retired row fails its own
/// soft-delete filter. That refusal is correct and worth knowing about: there
/// is no way to create a row that is born deleted.
async fn aged() -> (RecordStore<MemoryStore>, Arc<FixedClock>) {
    let clock = Arc::new(FixedClock::at(1_000));
    let store = RecordStore::new(MemoryStore::new(), catalog(), SecurityCatalog::new())
        .with_clock(clock.clone());

    let txn = store.begin().await.expect("begin");
    txn.insert(&root(), &docs(), &doc(1, "alive"))
        .await
        .expect("insert");
    txn.insert(&root(), &docs(), &doc(2, "old"))
        .await
        .expect("insert");
    txn.insert(&root(), &docs(), &doc(3, "middling"))
        .await
        .expect("insert");
    txn.insert(&root(), &docs(), &doc(4, "recent"))
        .await
        .expect("insert");
    txn.commit().await.expect("commit");

    for (id, at) in [(2_u64, 1_000_i64), (3, 5_000), (4, 9_000)] {
        clock.set(at);
        let txn = store.begin().await.expect("begin");
        txn.delete(&root(), &docs(), &[Value::U64(id)])
            .await
            .expect("retire");
        txn.commit().await.expect("commit");
    }
    clock.set(10_000);
    (store, clock)
}

/// Every row in the table, retired ones included.
async fn all_ids(store: &RecordStore<MemoryStore>) -> Vec<u64> {
    let txn = store.begin().await.expect("begin");
    let mut query = Query::all();
    query.include_deleted = true;
    let table = docs();
    let cursor = txn.execute(&root(), &table, &query).await.expect("execute");
    let rows = cursor.collect().await.expect("collect");
    let mut ids: Vec<u64> = rows
        .iter()
        .map(|row| match row.values().first() {
            Some(Value::U64(id)) => *id,
            other => panic!("id is {other:?}"),
        })
        .collect();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn a_purge_erases_only_rows_retired_before_the_bound() {
    // The bound is the undo window. A purge without one is "forget everything
    // that was ever deleted", which is a different and far less useful
    // operation than "forget what has been deleted long enough".
    let (store, _clock) = aged().await;
    let txn = store.begin().await.expect("begin");
    let purged = txn
        .purge_deleted(&root(), &docs(), 6_000, None)
        .await
        .expect("purge");
    txn.commit().await.expect("commit");

    assert_eq!(purged, 2, "t=1000 and t=5000 are older than t=6000");
    // 1 is alive and was never a candidate; 4 was retired after the bound.
    assert_eq!(all_ids(&store).await, vec![1, 4]);
}

#[tokio::test]
async fn the_bound_is_strict_so_a_row_retired_exactly_then_survives() {
    // Written because a mutation changing `<` to `<=` passed everything: the
    // retirement times were 1000, 5000 and 9000 and the bounds were 6000 and
    // i64::MAX, so no row ever sat *on* the boundary. A purge whose bound is
    // off by one instant is the kind of thing nobody notices until a row that
    // should have survived did not.
    let (store, _clock) = aged().await;
    let txn = store.begin().await.expect("begin");
    let purged = txn
        .purge_deleted(&root(), &docs(), 5_000, None)
        .await
        .expect("purge");
    txn.commit().await.expect("commit");

    assert_eq!(purged, 1, "only t=1000; t=5000 is not *before* t=5000");
    assert_eq!(all_ids(&store).await, vec![1, 3, 4]);
}

#[tokio::test]
async fn a_purge_never_touches_a_live_row() {
    // The failure that would matter most, stated on its own: a bound far in
    // the future must still leave every *un-retired* row alone. Without the
    // `IS NOT NULL` conjunct a null `deleted_at` compares unknown and is
    // withheld — true, and relying on it silently is how a destructive
    // operation acquires a subtle dependency on three-valued logic.
    let (store, _clock) = aged().await;
    let txn = store.begin().await.expect("begin");
    let purged = txn
        .purge_deleted(&root(), &docs(), i64::MAX, None)
        .await
        .expect("purge");
    txn.commit().await.expect("commit");

    assert_eq!(purged, 3, "every retired row, and only those");
    assert_eq!(all_ids(&store).await, vec![1], "the live row survives");
}

#[tokio::test]
async fn a_purged_row_takes_its_index_entries_with_it() {
    // A row erased without its index entries leaves a key pointing at nothing,
    // which the next scan over that index either skips silently or reports as
    // corruption. `erase_row` is the same code the hard-delete path uses, and
    // this is what says so.
    let (store, _clock) = aged().await;
    let txn = store.begin().await.expect("begin");
    txn.purge_deleted(&root(), &docs(), i64::MAX, None)
        .await
        .expect("purge");
    txn.commit().await.expect("commit");

    // Read **index-only**, which is the only way to see this.
    //
    // An ordinary read cannot: it walks the index to a primary key, fetches
    // the row, finds nothing and skips — so an orphaned entry looks exactly
    // like a purged one. A projection of just the indexed column is answered
    // from the keys without a fetch, so a surviving entry is returned.
    //
    // `include_deleted` matters here too: with the soft-delete conjunct in
    // place the planner will not choose an index that lacks `deleted_at`, so
    // the covering scan this test depends on would not happen.
    let txn = store.begin().await.expect("begin");
    let mut query = Query::all()
        .filter(slate_kernel::Expr::eq(
            slate_schema::Ordinal(1),
            Value::Str("old".to_owned()),
        ))
        .select([slate_schema::Ordinal(1)]);
    query.include_deleted = true;
    let table = docs();
    let rows = txn
        .execute(&root(), &table, &query)
        .await
        .expect("execute")
        .collect()
        .await
        .expect("collect");
    assert!(
        rows.is_empty(),
        "a purged row left {} entry/entries in `by_kind`",
        rows.len()
    );
}

#[tokio::test]
async fn a_purge_of_a_table_that_does_not_soft_delete_is_refused() {
    // Not a no-op. A table with no `soft_delete` has no retired rows by
    // construction, so this is a request aimed at the wrong table — and a
    // caller running it nightly would never find that out from a zero.
    let plain = TableDef::builder("plain", NOTES)
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .expect("a valid table");
    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([plain.clone()]).expect("catalog"),
        SecurityCatalog::new(),
    );
    let txn = store.begin().await.expect("begin");
    let refused = txn
        .purge_deleted(&root(), &plain, i64::MAX, None)
        .await
        .expect_err("a table with no soft delete has nothing to purge");
    assert!(
        matches!(
            refused,
            slate_kernel::KernelError::NotSoftDeleting { ref table } if table == "plain"
        ),
        "got {refused:?}"
    );
}

#[tokio::test]
async fn a_purge_past_its_ceiling_is_refused_before_it_erases_anything() {
    // The same ceiling `delete_where` has, and for the same reason: a purge
    // that quietly erased ten million rows because a bound was mistyped is the
    // one mistake here that cannot be undone.
    let (store, _clock) = aged().await;
    let txn = store.begin().await.expect("begin");
    let refused = txn
        .purge_deleted(&root(), &docs(), i64::MAX, Some(2))
        .await
        .expect_err("three retired rows exceed a ceiling of two");
    assert!(
        matches!(
            refused,
            slate_kernel::KernelError::PredicateWriteTooLarge { limit: 2 }
        ),
        "got {refused:?}"
    );
    drop(txn);

    // And nothing was erased: the refusal happens after the scan and before
    // the first delete, so the transaction has written nothing to roll back.
    assert_eq!(all_ids(&store).await, vec![1, 2, 3, 4]);
}

// --- who may see a retired row ----------------------------------------------

/// A store whose `reader` role may read `docs` and nothing more.
fn store_granting(actions: &[slate_kernel::Action]) -> RecordStore<MemoryStore> {
    let security = slate_kernel::SecurityCatalog::new().grant(slate_kernel::Grant::new(
        "reader",
        DOCS,
        actions.to_vec(),
    ));
    RecordStore::new(MemoryStore::new(), catalog(), security)
        .with_clock(Arc::new(FixedClock::at(5_000)))
}

fn reader() -> SecurityContext {
    SecurityContext::new(slate_kernel::Principal::new(Value::U64(1)).with_role("reader"))
}

/// Insert three and retire one, as the superuser, then hand the store back.
async fn seeded_for(store: &RecordStore<MemoryStore>) {
    let txn = store.begin().await.expect("begin");
    for id in 1..=3 {
        txn.insert(&root(), &docs(), &doc(id, "a"))
            .await
            .expect("insert");
    }
    txn.delete(&root(), &docs(), &[Value::U64(2)])
        .await
        .expect("retire");
    txn.commit().await.expect("commit");
}

#[tokio::test]
async fn a_plain_reader_cannot_ask_to_see_retired_rows() {
    // The grant this change exists for. `read` is not `read_deleted`, and a
    // caller holding the first does not silently acquire the second — which
    // is the whole reason `Action::ALL` excludes it.
    let store = store_granting(&[slate_kernel::Action::Read]);
    seeded_for(&store).await;

    let txn = store.begin().await.expect("begin");
    let mut query = Query::all();
    query.include_deleted = true;
    let table = docs();
    let refused = txn
        .execute(&reader(), &table, &query)
        .await
        .expect_err("a plain reader may not lift the soft-delete filter");
    assert!(
        matches!(
            refused,
            slate_kernel::KernelError::AccessDenied { ref action, .. }
            if action == &"read_deleted"
        ),
        "got {refused:?}"
    );
}

#[tokio::test]
async fn a_plain_reader_still_reads_live_rows() {
    // The other half, and the one a mistake here would break loudly: refusing
    // `include_deleted` must not refuse an ordinary read.
    let store = store_granting(&[slate_kernel::Action::Read]);
    seeded_for(&store).await;

    let txn = store.begin().await.expect("begin");
    let table = docs();
    let rows = txn
        .execute(&reader(), &table, &Query::all())
        .await
        .expect("an ordinary read is unaffected")
        .collect()
        .await
        .expect("collect");
    assert_eq!(rows.len(), 2, "the two rows that are not retired");
}

#[tokio::test]
async fn a_reader_granted_read_deleted_sees_them() {
    let store = store_granting(&[
        slate_kernel::Action::Read,
        slate_kernel::Action::ReadDeleted,
    ]);
    seeded_for(&store).await;

    let txn = store.begin().await.expect("begin");
    let mut query = Query::all();
    query.include_deleted = true;
    let table = docs();
    let rows = txn
        .execute(&reader(), &table, &query)
        .await
        .expect("granted")
        .collect()
        .await
        .expect("collect");
    assert_eq!(rows.len(), 3, "all three, retired one included");
}

#[tokio::test]
async fn include_deleted_needs_no_grant_on_a_table_that_does_not_soft_delete() {
    // Demanding a privilege for a no-op teaches callers to ask for privileges
    // they do not need, and a grant asked for often enough gets given.
    let plain = TableDef::builder("plain", NOTES)
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .expect("a valid table");
    let security = slate_kernel::SecurityCatalog::new().grant(slate_kernel::Grant::new(
        "reader",
        NOTES,
        vec![slate_kernel::Action::Read],
    ));
    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([plain.clone()]).expect("catalog"),
        security,
    );
    let txn = store.begin().await.expect("begin");
    txn.insert(&root(), &plain, &Row::new(vec![Value::U64(1)]))
        .await
        .expect("insert");
    txn.commit().await.expect("commit");

    let txn = store.begin().await.expect("begin");
    let mut query = Query::all();
    query.include_deleted = true;
    let rows = txn
        .execute(&reader(), &plain, &query)
        .await
        .expect("no soft delete, so nothing to reveal and nothing to grant")
        .collect()
        .await
        .expect("collect");
    assert_eq!(rows.len(), 1);
}

// --- soft delete under a foreign key ----------------------------------------
//
// A retired row is hidden from reads and still present in storage, so every
// constraint that decides by reading has to say which of those two facts it
// means. The two arms of `deletion_closure` want opposite answers, and until
// this section existed they shared one.

const FOLDERS: TableId = TableId(11);
const FILES: TableId = TableId(12);

/// A hard-deleting parent and a soft-deleting child on `on_delete`.
fn filing(on_delete: slate_schema::ReferentialAction) -> Catalog {
    let folders = TableDef::builder("folders", FOLDERS)
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .expect("a valid table");
    let files = TableDef::builder("files", FILES)
        .column("id", ValueType::U64)
        .nullable_column("folder_id", ValueType::U64)
        .nullable_column("deleted_at", ValueType::I64)
        .primary_key(["id"])
        .soft_delete("deleted_at")
        .foreign_key(
            slate_schema::ForeignKeyDef::builder("files_folder", FOLDERS)
                .column("folder_id")
                .on_delete(on_delete),
        )
        .build()
        .expect("a valid table");
    Catalog::from_tables([folders, files]).expect("a catalog")
}

/// A folder holding one file, with the clock at `at`.
async fn filed(
    on_delete: slate_schema::ReferentialAction,
    at: i64,
) -> (RecordStore<MemoryStore>, Arc<FixedClock>) {
    let clock = Arc::new(FixedClock::at(at));
    let store = RecordStore::new(
        MemoryStore::new(),
        filing(on_delete),
        SecurityCatalog::new(),
    )
    .with_clock(Arc::clone(&clock) as Arc<dyn slate_kernel::clock::Clock>);
    let txn = store.begin().await.expect("a transaction");
    let folders = txn.catalog().table_by_name("folders").expect("folders");
    let files = txn.catalog().table_by_name("files").expect("files");
    txn.insert(&root(), folders, &Row::new(vec![Value::U64(1)]))
        .await
        .expect("insert the folder");
    txn.insert(
        &root(),
        files,
        &Row::new(vec![Value::U64(10), Value::U64(1), Value::Null]),
    )
    .await
    .expect("insert the file");
    txn.commit().await.expect("commit");
    (store, clock)
}

/// Delete folder 1, and say what happened.
async fn drop_folder(store: &RecordStore<MemoryStore>) -> Result<bool, String> {
    let txn = store.begin().await.expect("a transaction");
    let folders = txn.catalog().table_by_name("folders").expect("folders");
    match txn.delete(&root(), folders, &[Value::U64(1)]).await {
        Ok(gone) => {
            txn.commit().await.expect("commit");
            Ok(gone)
        }
        Err(why) => Err(why.to_string()),
    }
}

/// Every row of `files`, retired ones included, as `(id, deleted_at)`.
async fn files_of(store: &RecordStore<MemoryStore>) -> Vec<(u64, Option<i64>)> {
    let txn = store.begin().await.expect("a transaction");
    let files = txn.catalog().table_by_name("files").expect("files");
    let mut query = Query::all();
    query.include_deleted = true;
    let rows = txn
        .execute(&root(), files, &query)
        .await
        .expect("query")
        .collect()
        .await
        .expect("rows");
    rows.into_iter()
        .map(|row| {
            let id = match row.values()[0] {
                Value::U64(id) => id,
                ref other => panic!("id is {other:?}"),
            };
            let at = match row.values()[2] {
                Value::Null => None,
                Value::I64(at) => Some(at),
                ref other => panic!("deleted_at is {other:?}"),
            };
            (id, at)
        })
        .collect()
}

#[tokio::test]
async fn a_restrict_edge_blocks_on_a_live_child() {
    // The control. Without it the next test passes on a schema where the edge
    // never worked at all, and a mutation that broke `RESTRICT` outright would
    // be caught only by tests in another file over another schema.
    let (store, _clock) = filed(slate_schema::ReferentialAction::Restrict, 1_000).await;
    let why = drop_folder(&store).await.expect_err("a live child blocks");
    assert!(why.contains("files"), "{why}");
    assert!(why.contains("files_folder"), "{why}");
}

#[tokio::test]
async fn a_restrict_edge_blocks_on_a_retired_child() {
    // The defect this section was written for: retiring the child made the
    // identical delete succeed, because the search for referencing rows read
    // with the ordinary soft-delete filter on. The row was still there, still
    // holding folder 1's key, and still readable with `include_deleted` — so
    // the parent went and left it pointing at nothing.
    let (store, _clock) = filed(slate_schema::ReferentialAction::Restrict, 1_000).await;

    let txn = store.begin().await.expect("a transaction");
    let files = txn.catalog().table_by_name("files").expect("files");
    assert!(
        txn.delete(&root(), files, &[Value::U64(10)])
            .await
            .expect("retire the file"),
        "the file was there to retire"
    );
    txn.commit().await.expect("commit");

    let why = drop_folder(&store)
        .await
        .expect_err("a retired child is still a child");
    assert!(why.contains("files_folder"), "{why}");
    assert_eq!(
        files_of(&store).await,
        vec![(10, Some(1_000))],
        "and the refusal left the child exactly as it was"
    );
}

#[tokio::test]
async fn a_cascade_edge_leaves_an_already_retired_child_and_its_timestamp_alone() {
    // The other half, and the reason `referencing_rows` takes a parameter
    // instead of one constant: a cascade must *not* reach a retired child.
    // Re-stamping `deleted_at` would push the row's purge deadline out by the
    // gap between the two deletes, so a retention window would silently
    // restart — which is why this test moves the clock rather than deleting
    // twice at the same instant, where a re-stamp is invisible.
    let (store, clock) = filed(slate_schema::ReferentialAction::Cascade, 1_000).await;

    let txn = store.begin().await.expect("a transaction");
    let files = txn.catalog().table_by_name("files").expect("files");
    txn.delete(&root(), files, &[Value::U64(10)])
        .await
        .expect("retire the file");
    txn.commit().await.expect("commit");
    assert_eq!(files_of(&store).await, vec![(10, Some(1_000))]);

    clock.set(9_000);
    assert!(
        drop_folder(&store).await.expect("the cascade is allowed"),
        "the folder was there to delete"
    );
    assert_eq!(
        files_of(&store).await,
        vec![(10, Some(1_000))],
        "the retired child keeps the timestamp its own delete gave it"
    );
}

#[tokio::test]
async fn a_cascade_edge_still_retires_a_live_child() {
    // The control for the one above: the cascade does reach a live child, and
    // retires rather than removes it. Reading with `include_deleted` is what
    // separates "retired by the cascade" from "erased by it".
    let (store, clock) = filed(slate_schema::ReferentialAction::Cascade, 1_000).await;
    clock.set(9_000);
    assert!(drop_folder(&store).await.expect("the cascade is allowed"));
    assert_eq!(files_of(&store).await, vec![(10, Some(9_000))]);
}

// --- restoring a retired row ------------------------------------------------
//
// A retired row used to be writable by nobody. It blocked an insert at its key
// with `DuplicatePrimaryKey` — present — and refused an update and an upsert at
// that same key with `RowNotFound` — absent. Both answers about one row in one
// transaction, so whichever a caller believed, the other was waiting. The line
// drawn here: **a write that names a primary key means the row at that key; a
// write that matches a predicate means the live ones.** `delete` is unaffected,
// because retiring an already-retired row is a no-op rather than a
// contradiction.

/// The one live row and the one retired row of `docs`, as `(id, deleted_at)`.
async fn state(store: &RecordStore<MemoryStore>) -> Vec<(u64, Option<i64>)> {
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let mut query = Query::all();
    query.include_deleted = true;
    let rows = txn
        .execute(&root(), table, &query)
        .await
        .expect("query")
        .collect()
        .await
        .expect("rows");
    let mut out: Vec<(u64, Option<i64>)> = rows
        .into_iter()
        .map(|row| {
            let id = match row.values()[0] {
                Value::U64(id) => id,
                ref other => panic!("id is {other:?}"),
            };
            let at = match row.values()[2] {
                Value::Null => None,
                Value::I64(at) => Some(at),
                ref other => panic!("deleted_at is {other:?}"),
            };
            (id, at)
        })
        .collect();
    out.sort_unstable();
    out
}

#[tokio::test]
async fn an_update_at_a_retired_rows_key_restores_it() {
    // The gap this section closes. `seeded()` retires row 2 at 5,000.
    let store = seeded().await;
    assert_eq!(
        state(&store).await,
        vec![(1, None), (2, Some(5_000)), (3, None)]
    );
    assert_eq!(ids(&store, Query::all()).await, vec![1, 3]);

    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    txn.update(&root(), table, &doc(2, "restored"))
        .await
        .expect("an update at a retired row's key restores it");
    txn.commit().await.expect("commit");

    assert_eq!(state(&store).await, vec![(1, None), (2, None), (3, None)]);
    assert_eq!(
        ids(&store, Query::all()).await,
        vec![1, 2, 3],
        "and an ordinary read, which had stopped returning it, returns it again"
    );
}

#[tokio::test]
async fn an_upsert_over_a_retired_row_restores_it_rather_than_reporting_it_missing() {
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    txn.upsert(&root(), table, &doc(2, "back"))
        .await
        .expect("an upsert names the key too");
    txn.commit().await.expect("commit");
    assert_eq!(state(&store).await, vec![(1, None), (2, None), (3, None)]);
}

#[tokio::test]
async fn an_insert_at_a_retired_rows_key_is_still_refused() {
    // The arm that was always right, and the reason the fix is "make the other
    // three agree with this one" rather than "make all four say absent". An
    // insert that succeeded here would silently overwrite a row the retention
    // window exists to keep.
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let refused = txn
        .insert(&root(), table, &doc(2, "new"))
        .await
        .expect_err("the key is taken by the retired row");
    assert!(
        matches!(
            refused,
            slate_kernel::KernelError::DuplicatePrimaryKey { .. }
        ),
        "{refused:?}"
    );
    assert_eq!(
        state(&store).await,
        vec![(1, None), (2, Some(5_000)), (3, None)]
    );
}

#[tokio::test]
async fn a_predicate_write_still_skips_a_retired_row() {
    // The other side of the line, and the test that stops the fix being
    // "simplified" into `row_filter` where it would apply to everything. An
    // `update_where` whose predicate matches every row must still touch only
    // the live ones: the predicate did not name row 2.
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let affected = txn
        .update_where(
            &root(),
            table,
            slate_kernel::Expr::True,
            &[(
                slate_schema::Ordinal(1),
                slate_kernel::Scalar::Literal(Value::Str("swept".to_owned())),
            )],
            None,
        )
        .await
        .expect("update_where");
    txn.commit().await.expect("commit");
    assert_eq!(affected.len(), 2, "the two live rows, not the retired one");
    assert_eq!(
        state(&store).await,
        vec![(1, None), (2, Some(5_000)), (3, None)]
    );
}

#[tokio::test]
async fn a_policy_still_hides_a_retired_row_from_an_update() {
    // `Deleted::Visible` drops the soft-delete conjunct and nothing else. If it
    // dropped the policy too, a caller could reach a retired row their policy
    // never let them see — and reach it by primary key, which is the cheapest
    // possible probe. `RowNotFound` rather than a refusal naming the policy,
    // for the reason the code gives: which of "hidden" and "missing" it was is
    // exactly what a policy exists not to say.
    let security = slate_kernel::SecurityCatalog::new()
        .grant(slate_kernel::Grant::new(
            "reader",
            DOCS,
            vec![
                slate_kernel::Action::Read,
                slate_kernel::Action::Insert,
                slate_kernel::Action::Update,
                slate_kernel::Action::Delete,
                slate_kernel::Action::ReadDeleted,
            ],
        ))
        .policy(slate_kernel::Policy::new(
            "memos_only",
            DOCS,
            slate_kernel::Action::ALL,
            |_: &SecurityContext| {
                slate_kernel::Expr::eq(slate_schema::Ordinal(1), Value::Str("memo".to_owned()))
            },
        ));
    let store = RecordStore::new(MemoryStore::new(), catalog(), security)
        .with_clock(Arc::new(FixedClock::at(5_000)));
    seeded_for(&store).await; // rows 1..3 of kind "a", row 2 retired

    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let refused = txn
        .update(&reader(), table, &doc(2, "memo"))
        .await
        .expect_err("the policy hides row 2 whether or not it is retired");
    assert!(
        matches!(refused, slate_kernel::KernelError::RowNotFound { .. }),
        "{refused:?}"
    );
    // Unchanged, and still retired: the refusal is not a partial write.
    assert_eq!(
        state(&store).await,
        vec![(1, None), (2, Some(5_000)), (3, None)]
    );
}

#[tokio::test]
async fn a_write_that_supplies_the_soft_delete_column_names_that_column() {
    // What a caller hits first when restoring by hand: read the row with
    // `include_deleted`, edit a field, write it back — timestamp and all. That
    // used to come back as "row-level security forbids writing this row" on a
    // table with no policies at all, which sends the reader to the grants. The
    // column is the kernel's to write, and now the message says so.
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let stamped = Row::new(vec![
        Value::U64(2),
        Value::Str("as read".to_owned()),
        Value::I64(5_000),
    ]);
    let refused = txn
        .update(&root(), table, &stamped)
        .await
        .expect_err("the soft-delete column is not the caller's to write");
    let why = refused.to_string();
    assert!(why.contains("deleted_at"), "{why}");
    assert!(why.contains("null to restore"), "{why}");
    assert!(
        !why.contains("row-level security"),
        "the old message sent the reader to the grants: {why}"
    );
}

#[tokio::test]
async fn restoring_a_row_whose_unique_slot_was_reused_is_refused() {
    // Retiring a row frees its slot in a partial unique index on
    // `deleted_at IS NULL` — that is the documented point of the shape. So
    // restoring one is not always possible, and the interesting case is the one
    // where somebody took the slot in between. It has to be refused rather than
    // written, or the index would hold two rows at one unique key.
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
    txn.delete(&root(), notes, &[Value::U64(1)])
        .await
        .expect("retire 1, freeing the slug");
    txn.insert(&root(), notes, &row(2))
        .await
        .expect("2 takes the freed slug");
    let refused = txn
        .update(&root(), notes, &row(1))
        .await
        .expect_err("restoring 1 would put two live rows on one unique slug");
    assert!(
        matches!(refused, slate_kernel::KernelError::UniqueViolation { .. }),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_bulk_writes_restore_a_retired_row_too() {
    // The bulk path is a separate arm over a separately-read `existing`, and it
    // was the one mutation the tests above did not catch: making it read
    // `Deleted::Hidden` again broke nothing, because nothing exercised it. A
    // mutation that causes no failure is a missing test.
    //
    // Both bulk writes that can touch an existing row, each restoring row 2 in
    // a batch that also writes a live one — so a fix that special-cased a
    // one-row batch would still be caught.
    for (name, bulk) in [("update_many", false), ("upsert_many", true)] {
        let store = seeded().await;
        let txn = store.begin().await.expect("a transaction");
        let table = txn.catalog().table_by_name("docs").expect("the table");
        let batch = [doc(1, "kept"), doc(2, "restored")];
        if bulk {
            txn.upsert_many(&root(), table, &batch).await
        } else {
            txn.update_many(&root(), table, &batch).await
        }
        .unwrap_or_else(|why| panic!("{name} at a retired row's key: {why}"));
        txn.commit().await.expect("commit");
        assert_eq!(
            state(&store).await,
            vec![(1, None), (2, None), (3, None)],
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_bulk_insert_at_a_retired_rows_key_is_still_refused() {
    // The bulk twin of `an_insert_at_a_retired_rows_key_is_still_refused`: the
    // arm that was already right, so that "make every bulk arm say absent"
    // cannot pass.
    let store = seeded().await;
    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let refused = txn
        .insert_many(
            &root(),
            table,
            &[doc(4, "new"), doc(2, "over the retired one")],
        )
        .await
        .expect_err("the key is taken by the retired row");
    assert!(
        matches!(
            refused,
            slate_kernel::KernelError::DuplicatePrimaryKey { .. }
        ),
        "{refused:?}"
    );
    drop(txn);
    assert_eq!(
        state(&store).await,
        vec![(1, None), (2, Some(5_000)), (3, None)],
        "and row 4, earlier in the same batch, was not written either"
    );
}

#[tokio::test]
async fn without_read_deleted_a_retired_row_stays_out_of_reach() {
    // The grant is the line. A caller with read, insert, update and delete but
    // not `read_deleted` sees a retired row exactly as they see a row a policy
    // hides — the key is taken, and there is nothing there to update. Both
    // halves are asserted together, because either one alone reads as a bug:
    // it is the pair that is the design, and the pair this file already chose
    // for policy-hidden rows.
    let store = store_granting(&[
        slate_kernel::Action::Read,
        slate_kernel::Action::Insert,
        slate_kernel::Action::Update,
        slate_kernel::Action::Delete,
    ]);
    seeded_for(&store).await; // 1..3, row 2 retired at 5,000

    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    let taken = txn
        .insert(&reader(), table, &doc(2, "mine now"))
        .await
        .expect_err("the retired row still occupies the key");
    assert!(
        matches!(taken, slate_kernel::KernelError::DuplicatePrimaryKey { .. }),
        "{taken:?}"
    );
    let missing = txn
        .update(&reader(), table, &doc(2, "mine now"))
        .await
        .expect_err("and is not theirs to overwrite");
    assert!(
        matches!(missing, slate_kernel::KernelError::RowNotFound { .. }),
        "{missing:?}"
    );
    drop(txn);
    assert_eq!(
        state(&store).await,
        vec![(1, None), (2, Some(5_000)), (3, None)]
    );
}

#[tokio::test]
async fn read_deleted_is_what_makes_the_same_row_restorable() {
    // The control for the test above, differing in exactly one grant. Without
    // it, "the caller could not restore it" would be evidence of nothing — a
    // missing `update` grant, a wrong key, a table that never soft-deleted.
    let store = store_granting(&[
        slate_kernel::Action::Read,
        slate_kernel::Action::Insert,
        slate_kernel::Action::Update,
        slate_kernel::Action::Delete,
        slate_kernel::Action::ReadDeleted,
    ]);
    seeded_for(&store).await;

    let txn = store.begin().await.expect("a transaction");
    let table = txn.catalog().table_by_name("docs").expect("the table");
    txn.update(&reader(), table, &doc(2, "restored"))
        .await
        .expect("read_deleted is the grant that reaches it");
    txn.commit().await.expect("commit");
    assert_eq!(state(&store).await, vec![(1, None), (2, None), (3, None)]);
}
