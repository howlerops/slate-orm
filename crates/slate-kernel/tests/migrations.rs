//! Migrations, and the defect they exist for.
//!
//! The first test is the measurement that motivated the whole module: adding an
//! index to a table that already holds rows does not make a query slower, it
//! makes it return **nothing**. Everything else here is about making that
//! impossible to reach by accident.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::migrate::{self, MigrationPlan, Refusal, Step};
use slate_kernel::store::{KeyRange, KvSnapshot, KvStore, KvTransaction};
use slate_kernel::{
    Action, CmpOp, Expr, Grant, KernelError, Principal, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as Atomics};

/// A store that counts the writes through it.
///
/// Needed because "did the migration do anything" is a claim about writes, and
/// the report is the runner's own account of itself. A runner that wrote the
/// state key unconditionally would return an empty step list and still write.
struct Counting {
    inner: MemoryStore,
    puts: Arc<AtomicUsize>,
}

struct CountingTxn<'a> {
    inner: Box<dyn KvTransaction + Send + 'a>,
    puts: Arc<AtomicUsize>,
}

impl Clone for Counting {
    /// Shares the store and the counter, because `MemoryStore::clone` shares
    /// its own state and a counter that reset per clone would count nothing.
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            puts: Arc::clone(&self.puts),
        }
    }
}

impl Counting {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            puts: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn puts(&self) -> usize {
        self.puts.load(Atomics::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvStore for Counting {
    async fn begin(&self) -> slate_kernel::Result<Box<dyn KvTransaction + Send + '_>> {
        Ok(Box::new(CountingTxn {
            inner: self.inner.begin().await?,
            puts: Arc::clone(&self.puts),
        }))
    }
}

#[async_trait::async_trait]
impl KvSnapshot for CountingTxn<'_> {
    async fn get(&self, key: &[u8]) -> slate_kernel::Result<Option<bytes::Bytes>> {
        self.inner.get(key).await
    }
    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> slate_kernel::Result<Box<dyn slate_kernel::store::KvIterator + Send + '_>> {
        self.inner.scan(range, order).await
    }
    fn is_point_in_time(&self) -> bool {
        self.inner.is_point_in_time()
    }
}

#[async_trait::async_trait]
impl KvTransaction for CountingTxn<'_> {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> slate_kernel::Result<()> {
        self.puts.fetch_add(1, Atomics::SeqCst);
        self.inner.put(key, value)
    }
    fn delete(&self, key: Vec<u8>) -> slate_kernel::Result<()> {
        self.inner.delete(key)
    }
    async fn commit(self: Box<Self>) -> slate_kernel::Result<Option<u64>> {
        self.inner.commit().await
    }
    fn rollback(self: Box<Self>) {
        self.inner.rollback();
    }
}

const USERS: TableId = TableId(1);
const BY_EMAIL: IndexId = IndexId(10);
const MEMBERS: TableId = TableId(2);
const BY_TEAM_EMAIL: IndexId = IndexId(11);

/// The table, with its index optional so the same data can be read back under
/// a schema that has one and a schema that does not.
fn users(indexed: bool) -> TableDef {
    let mut builder = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id"]);
    if indexed {
        builder = builder.index(IndexDef::builder("by_email", BY_EMAIL).column("email"));
    }
    builder.build().unwrap()
}

/// A second table, three columns wide, for the partial index.
///
/// Separate from `users` because the width matters to the planner: an index on
/// `email` plus the primary key covers every column of `users`, so an index-only
/// scan is available and gets chosen. Add a third column and the index stops
/// covering, the plan needs a row lookup per match, and on a small table a full
/// scan wins. That is why the defect at the top of this file needs the narrow
/// shape to be *visible* — see the comment there.
fn members(index: Option<slate_schema::IndexBuilder>) -> TableDef {
    let mut builder = TableDef::builder("members", MEMBERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .column("team", ValueType::U64)
        .primary_key(["id"]);
    if let Some(index) = index {
        builder = builder.index(index);
    }
    builder.build().unwrap()
}

/// How many entries the keyspace actually holds for an index.
///
/// The oracle the planner-visible assertions cannot be: whether a query finds a
/// row depends on which plan the cost model picked, and that changes with the
/// width of the table and the number of rows. The number of keys under the
/// index prefix does not depend on anything but the backfill.
async fn index_entries(store: &MemoryStore, index: IndexId) -> usize {
    let txn = store.begin().await.unwrap();
    let mut cursor = txn
        .scan(
            KeyRange::prefix(&slate_kernel::keys::index_prefix_of(index)),
            ScanOrder::Ascending,
        )
        .await
        .unwrap();
    let mut n = 0;
    while cursor.next().await.unwrap().is_some() {
        n += 1;
    }
    drop(cursor);
    txn.rollback();
    n
}

fn catalog(table: TableDef) -> Catalog {
    Catalog::from_tables([table]).unwrap()
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("member"))
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("member", USERS, Action::EVERYTHING))
        .grant(Grant::new("member", MEMBERS, Action::EVERYTHING))
}

fn row(table: &TableDef, id: u64, email: &str, team: u64) -> Row {
    let mut values = vec![Value::U64(id), Value::Str(email.into())];
    if table.columns().len() > 2 {
        values.push(Value::U64(team));
    }
    Row::new(values)
}

async fn seed<S: KvStore + Clone>(store: &S, table: &TableDef, rows: &[(u64, &str, u64)]) {
    let records = RecordStore::new(store.clone(), catalog(table.clone()), security());
    let txn = records.begin().await.unwrap();
    for (id, email, team) in rows {
        txn.insert(&context(), table, &row(table, *id, email, *team))
            .await
            .unwrap();
    }
    txn.commit().await.unwrap();
}

/// How many rows a filter on `email` returns, under `table`'s schema.
async fn found<S: KvStore + Clone>(store: &S, table: &TableDef, email: &str) -> usize {
    let records = RecordStore::new(store.clone(), catalog(table.clone()), security());
    let txn = records.begin().await.unwrap();
    let cursor = txn
        .query(
            &context(),
            table,
            Expr::compare(Ordinal(1), CmpOp::Eq, Value::Str(email.into())),
            ScanOrder::Ascending,
        )
        .await
        .unwrap();
    let rows = cursor.collect().await.unwrap();
    txn.rollback();
    rows.len()
}

#[tokio::test]
async fn an_index_added_after_the_rows_returns_nothing_until_it_is_built() {
    let store = MemoryStore::new();
    let before = users(false);
    let after = users(true);
    seed(
        &store,
        &before,
        &[(1, "a@x", 1), (2, "b@x", 1), (3, "c@x", 2)],
    )
    .await;

    // The defect, measured. The planner sees an index covering the predicate,
    // costs it as the cheap option, and scans a key range nothing ever wrote
    // into. No error, no warning, and the row is still on disk.
    //
    // The shape matters, and it makes the defect worse rather than narrower:
    // `users` is two columns wide, so the index plus the primary key covers the
    // query and an index-only scan is the cheapest plan. Widen the table by one
    // column and the same query on the same unbuilt index answers *correctly*,
    // because a full scan wins on cost. So whether an unmigrated deploy returns
    // right answers or empty ones depends on the cost model — which is not a
    // thing anyone should be relying on, and is why the fix is a refusal at
    // startup rather than a note about when it matters.
    assert_eq!(
        found(&store, &after, "b@x").await,
        0,
        "this is the defect; if it has been fixed elsewhere, this test should be rewritten \
         rather than deleted"
    );
    // The same query under the schema that wrote the rows finds it, which is
    // what makes the line above a wrong answer rather than an empty table.
    assert_eq!(found(&store, &before, "b@x").await, 1);

    // The migration is the fix. Two steps, because this table predates the
    // state key entirely — which is the case every existing deployment is in
    // the first time it runs this.
    let report = migrate::migrate(&store, &catalog(after.clone()))
        .await
        .unwrap();
    assert!(
        matches!(
            report.steps.as_slice(),
            [
                Step::Register { .. },
                Step::BuildIndex {
                    index: BY_EMAIL,
                    ..
                }
            ]
        ),
        "{:?}",
        report.steps
    );
    assert_eq!(report.entries_written, vec![3]);

    assert_eq!(found(&store, &after, "b@x").await, 1);
    assert_eq!(found(&store, &after, "nobody@x").await, 0);
}

#[tokio::test]
async fn a_table_nobody_has_migrated_is_refused_at_startup_not_at_query_time() {
    let store = MemoryStore::new();
    let before = users(false);
    seed(&store, &before, &[(1, "a@x", 1)]).await;

    // `verify` is the guard, and it names the index rather than the table: an
    // operator reading this has to know what to run and why.
    let error = migrate::verify(&store, &catalog(users(true)))
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("by_email"), "{message}");
    assert!(message.contains("returns no rows"), "{message}");

    migrate::migrate(&store, &catalog(users(true)))
        .await
        .unwrap();
    migrate::verify(&store, &catalog(users(true)))
        .await
        .unwrap();
}

#[tokio::test]
async fn migrating_twice_does_nothing_the_second_time() {
    let store = MemoryStore::new();
    seed(&store, &users(false), &[(1, "a@x", 1), (2, "b@x", 1)]).await;
    let catalog = catalog(users(true));

    let first = migrate::migrate(&store, &catalog).await.unwrap();
    assert_eq!(first.entries_written, vec![2]);

    // Not "it does not crash": the plan is *empty*, so a second deploy of the
    // same binary does no work at all. A runner that rebuilt every index on
    // every start would pass a test that only checked the rows afterwards.
    let plan = migrate::plan(&store, &catalog).await.unwrap();
    assert!(plan.is_empty(), "{plan:?}");
    let second = migrate::migrate(&store, &catalog).await.unwrap();
    assert!(second.steps.is_empty(), "{:?}", second.steps);
    assert_eq!(found(&store, &users(true), "b@x").await, 1);
}

#[tokio::test]
async fn a_migration_with_nothing_to_do_writes_nothing_at_all() {
    // "No steps" and "no writes" are different claims, and only the second one
    // is what a deploy actually wants: rewriting an unchanged state record on
    // every start is a write-write conflict surface against every other process
    // doing the same. Asserting the step list alone let a mutation that removed
    // the guard survive, so this counts the puts.
    let counted = Counting::new();
    let settled = catalog(users(true));
    migrate::migrate(&counted, &settled).await.unwrap();
    seed(&counted, &users(true), &[(1, "a@x", 1)]).await;

    let before = counted.puts();
    migrate::migrate(&counted, &settled).await.unwrap();
    assert_eq!(
        counted.puts() - before,
        0,
        "a migration with nothing to do still wrote to the store"
    );
    // And the check that keeps the counter honest: a migration with something
    // to do does write.
    let before = counted.puts();
    migrate::migrate(&counted, &catalog(members(None)))
        .await
        .unwrap();
    assert!(counted.puts() > before, "the counter is not counting");
}

#[tokio::test]
async fn a_new_table_costs_one_registration_and_no_reads() {
    let store = MemoryStore::new();
    let catalog = catalog(users(true));
    let report = migrate::migrate(&store, &catalog).await.unwrap();
    // Register, then build — and the build writes nothing, because there is
    // nothing to build over. The first deploy of a new table is free.
    assert!(
        matches!(
            report.steps.as_slice(),
            [Step::Register { .. }, Step::BuildIndex { .. }]
        ),
        "{:?}",
        report.steps
    );
    assert_eq!(report.entries_written, vec![0]);
    migrate::verify(&store, &catalog).await.unwrap();
}

#[tokio::test]
async fn a_changed_column_type_is_refused_rather_than_read_as_something_else() {
    let store = MemoryStore::new();
    migrate::migrate(&store, &catalog(users(true)))
        .await
        .unwrap();
    seed(&store, &users(true), &[(1, "a@x", 1)]).await;

    // `email` was text and is now bytes. Same number of columns, same
    // positions, same primary key — so nothing about the *shape* changed and
    // nothing fails at write time. Old rows simply decode as a type they were
    // not written as, which is the quiet corruption the fingerprint is for.
    let retyped = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Bytes)
        .primary_key(["id"])
        .index(IndexDef::builder("by_email", BY_EMAIL).column("email"))
        .build()
        .unwrap();
    // The column count is deliberately unchanged, so this refusal can only come
    // from the type. An earlier version of this test changed the width too, and
    // passed for that reason instead.
    assert_eq!(retyped.columns().len(), users(true).columns().len());

    let plan = migrate::plan(&store, &catalog(retyped.clone()))
        .await
        .unwrap();
    assert!(plan.is_blocked(), "{plan:?}");
    assert!(matches!(
        plan.refusals.as_slice(),
        [Refusal::LayoutChanged { .. }]
    ));
    // And the refusal is a refusal: applying it runs none of the steps.
    let error = migrate::apply(&store, &catalog(retyped.clone()), &plan)
        .await
        .unwrap_err();
    assert!(
        matches!(error, KernelError::MigrationRefused { .. }),
        "{error}"
    );
    let message = error.to_string();
    // The message names the column that moved. It used to name what the
    // fingerprint *covers* — six things, one of which — because a hash cannot
    // say which; the stored schema can, and this is the assertion that
    // changed when it started to.
    assert!(message.contains("column 1 `email`"), "{message}");
    assert!(message.contains("was string"), "{message}");
    assert!(message.contains("Renames"), "{message}");
}

#[tokio::test]
async fn a_rename_is_not_a_migration() {
    let store = MemoryStore::new();
    migrate::migrate(&store, &catalog(users(true)))
        .await
        .unwrap();
    seed(&store, &users(true), &[(1, "a@x", 1)]).await;

    // A name appears nowhere on disk, which is why `renamed_column` is free.
    // If the fingerprint covered names this would be refused, and a rename
    // would be indistinguishable from the type change above.
    let renamed = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email_address", ValueType::Str)
        .primary_key(["id"])
        .renamed_column("email_address", "email")
        .index(IndexDef::builder("by_email", BY_EMAIL).column("email_address"))
        .build()
        .unwrap();

    let plan = migrate::plan(&store, &catalog(renamed.clone()))
        .await
        .unwrap();
    assert!(plan.is_empty(), "a rename asked for work: {plan:?}");
}

#[tokio::test]
async fn adding_a_nullable_column_is_not_a_migration_but_it_is_a_new_layout() {
    let store = MemoryStore::new();
    migrate::migrate(&store, &catalog(users(false)))
        .await
        .unwrap();
    seed(&store, &users(false), &[(1, "a@x", 1)]).await;

    let widened = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .added_column("nickname", ValueType::Str, 2)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();

    // It changes the fingerprint — the column count moved — and it is no
    // longer refused. **This assertion was the opposite until the schema was
    // stored**, and the comment then said why: "the runner cannot tell an
    // appended column from a retyped one and the safe answer to 'I cannot
    // tell' is no", which it called the sharpest limitation of the
    // fingerprint. It can tell now, so a genuinely additive change no longer
    // needs a hand.
    let plan = migrate::plan(&store, &catalog(widened.clone()))
        .await
        .unwrap();
    assert!(!plan.is_blocked(), "{plan:?}");
    assert!(
        plan.steps
            .iter()
            .any(|step| matches!(step, Step::WidenSchema { added, .. } if added == &["nickname"])),
        "{plan:?}"
    );

    // And the row written before the column existed still reads, with the new
    // column null. This is the claim the whole change rests on: nothing is
    // rewritten because nothing needs to be — `decode_row` reads the row's own
    // schema version and skips a column that had not been added yet.
    migrate::migrate(&store, &catalog(widened.clone()))
        .await
        .unwrap();
    let records = RecordStore::new(store.clone(), catalog(widened.clone()), security());
    let txn = records.begin().await.unwrap();
    let stored = txn
        .get(&context(), &widened, &[Value::U64(1)])
        .await
        .unwrap()
        .expect("the row written before the widening is gone");
    txn.rollback();
    let nickname = widened.ordinal_of("nickname").unwrap();
    assert_eq!(stored.get(nickname), Some(&Value::Null));
    // The columns that were there still read as themselves, which is the half
    // a wrong prefix comparison would break.
    assert_eq!(
        stored.get(widened.ordinal_of("email").unwrap()),
        Some(&Value::Str("a@x".into()))
    );
}

/// A retype is still refused, which is the other half of the pair.
///
/// Without this the change above would read as "layout refusals were
/// weakened". They were narrowed: an append is told from a retype and only one
/// of the two is let through.
#[tokio::test]
async fn a_retype_is_still_refused_after_an_append_is_allowed() {
    let store = MemoryStore::new();
    migrate::migrate(&store, &catalog(users(false)))
        .await
        .unwrap();

    let retyped = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::U64)
        .added_column("nickname", ValueType::Str, 2)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();

    // Appended *and* retyped, which is the case that makes the prefix
    // comparison load-bearing: a differ that reported only the column count
    // would call this additive and let a `string` column be read as `u64`.
    let plan = migrate::plan(&store, &catalog(retyped)).await.unwrap();
    assert!(plan.is_blocked(), "{plan:?}");
    assert!(
        plan.why_blocked().contains("column 1 `email`"),
        "{}",
        plan.why_blocked()
    );
}

/// A column appended at a version rows were already written at is refused.
///
/// It looks additive and is not: `present_at` would say the column *was* there
/// when those rows were written, so the decoder would read a value nobody
/// wrote. The `added_in` is the whole of what makes an append safe, and this
/// is the case where it is wrong.
#[tokio::test]
async fn a_column_appended_at_an_already_written_version_is_refused() {
    let store = MemoryStore::new();
    let versioned = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();
    migrate::migrate(&store, &catalog(versioned)).await.unwrap();

    // Added at 2, and rows have already been written at 2.
    let widened = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .added_column("nickname", ValueType::Str, 2)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();
    let plan = migrate::plan(&store, &catalog(widened)).await.unwrap();
    assert!(plan.is_blocked(), "{plan:?}");
}

/// A table that *loses* a column is refused, which the append rule does not
/// cover by symmetry.
///
/// An append is safe because `present_at` reads the row's own written version
/// and hands back a default for a column that did not exist yet. A narrowing
/// has no such mechanism working for it: every stored row was encoded with the
/// wider column list, so the trailing value is still in the bytes with nothing
/// declaring what it is. Letting it through would either misread the row or
/// silently orphan a value, and both are worse than a refusal at startup.
///
/// This test exists because a mutation that replaced the "and it did not get
/// wider" half of the compatibility test with `false` survived the whole
/// suite: every other case here either changes the prefix or adds a column, so
/// nothing pinned the direction.
#[tokio::test]
async fn a_table_that_loses_a_column_is_refused() {
    let store = MemoryStore::new();
    let wide = members(None);
    migrate::migrate(&store, &catalog(wide.clone()))
        .await
        .unwrap();
    seed(&store, &wide, &[(1, "a@x", 7)]).await;

    // The same table with `team` gone. The prefix is untouched, so the only
    // layout change is the count — which is exactly the shape an append has,
    // and is why the count alone cannot decide.
    let narrowed = TableDef::builder("members", MEMBERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id"])
        .schema_version(2)
        .build()
        .unwrap();

    let plan = migrate::plan(&store, &catalog(narrowed)).await.unwrap();
    assert!(plan.is_blocked(), "{plan:?}");
    assert!(
        plan.why_blocked().contains("had 3 columns and now has 2"),
        "{}",
        plan.why_blocked()
    );
}

#[tokio::test]
async fn building_a_unique_index_over_duplicates_refuses_instead_of_hiding_a_row() {
    let store = MemoryStore::new();
    let plain = users(false);
    migrate::migrate(&store, &catalog(plain.clone()))
        .await
        .unwrap();
    // Two rows with the same email, which the schema of the day permitted.
    seed(&store, &plain, &[(1, "same@x", 1), (2, "same@x", 2)]).await;

    let unique = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id"])
        .index(
            IndexDef::builder("by_email", BY_EMAIL)
                .column("email")
                .unique(),
        )
        .build()
        .unwrap();

    // A unique entry's key omits the primary key, so the second row writes the
    // *same* key. Left alone it would overwrite the first row's pointer and
    // leave an index naming one row and hiding the other — a worse outcome than
    // the missing index this module exists to fix.
    let error = migrate::migrate(&store, &catalog(unique.clone()))
        .await
        .unwrap_err();
    assert!(
        matches!(error, KernelError::UniqueViolation { .. }),
        "{error}"
    );
    // And it left nothing behind: the failing batch rolled back, so the index
    // holds no entries at all rather than the one it wrote before the clash.
    assert_eq!(index_entries(&store, BY_EMAIL).await, 0);
    // The state still says the index is not built, so `verify` refuses rather
    // than reporting a half-built index as done.
    assert!(migrate::verify(&store, &catalog(unique)).await.is_err());
}

#[tokio::test]
async fn a_partial_index_backfills_only_the_rows_it_admits() {
    let store = MemoryStore::new();
    let plain = members(None);
    migrate::migrate(&store, &catalog(plain.clone()))
        .await
        .unwrap();
    seed(
        &store,
        &plain,
        &[(1, "a@x", 1), (2, "b@x", 1), (3, "c@x", 2), (4, "d@x", 2)],
    )
    .await;

    let partial = members(Some(
        IndexDef::builder("by_team_email", BY_TEAM_EMAIL)
            .column("email")
            .only_where(Expr::compare(Ordinal(2), CmpOp::Eq, Value::U64(1))),
    ));

    // Two of the four rows are on team 1. A backfill that ignored `admits`
    // would write four entries, and the index would then claim rows the
    // predicate excludes — which the planner trusts and does not re-check.
    let report = migrate::migrate(&store, &catalog(partial.clone()))
        .await
        .unwrap();
    assert_eq!(report.entries_written, vec![2], "{report:?}");
    // Asserted against the keyspace rather than against a query, because a
    // query's answer here depends on which plan the cost model picks.
    assert_eq!(index_entries(&store, BY_TEAM_EMAIL).await, 2);
}

#[tokio::test]
async fn dropping_an_index_from_the_schema_reclaims_its_entries() {
    let store = MemoryStore::new();
    let indexed = users(true);
    migrate::migrate(&store, &catalog(indexed.clone()))
        .await
        .unwrap();
    seed(&store, &indexed, &[(1, "a@x", 1), (2, "b@x", 1)]).await;
    assert_eq!(index_entries(&store, BY_EMAIL).await, 2);

    // The write path only deletes entries for indexes it can see, so an index
    // removed from the schema leaves keys that nothing will ever reclaim: not a
    // correctness problem, because nothing reads them, but dead weight that
    // outlives every row it described.
    let plan = migrate::plan(&store, &catalog(users(false))).await.unwrap();
    assert!(
        matches!(
            plan.steps.as_slice(),
            [Step::DropIndex {
                index: BY_EMAIL,
                ..
            }]
        ),
        "{:?}",
        plan.steps
    );
    migrate::apply(&store, &catalog(users(false)), &plan)
        .await
        .unwrap();

    assert_eq!(index_entries(&store, BY_EMAIL).await, 0);
    // And the rows themselves are untouched.
    assert_eq!(found(&store, &users(false), "b@x").await, 1);
}

#[tokio::test]
async fn a_backfill_larger_than_one_batch_writes_every_row() {
    let store = MemoryStore::new();
    let plain = users(false);
    migrate::migrate(&store, &catalog(plain.clone()))
        .await
        .unwrap();

    // Over the batch size, so the resume path runs for real. A backfill whose
    // batches restarted from the beginning would loop for ever; one that
    // skipped a key at each boundary would be short by the number of batches,
    // which is why this asserts the exact count rather than "more than one".
    let owned: Vec<(u64, String, u64)> = (1..=2_500u64)
        .map(|id| (id, format!("u{id}@x"), id % 3))
        .collect();
    let rows: Vec<(u64, &str, u64)> = owned
        .iter()
        .map(|(id, email, team)| (*id, email.as_str(), *team))
        .collect();
    for chunk in rows.chunks(500) {
        seed(&store, &plain, chunk).await;
    }

    let report = migrate::migrate(&store, &catalog(users(true)))
        .await
        .unwrap();
    assert_eq!(report.entries_written, vec![2_500], "{report:?}");
    assert_eq!(index_entries(&store, BY_EMAIL).await, 2_500);
    // Spot-check the rows either side of every batch boundary.
    for id in [1u64, 1_000, 1_001, 2_000, 2_001, 2_500] {
        assert_eq!(
            found(&store, &users(true), &format!("u{id}@x")).await,
            1,
            "row {id} is missing from the index"
        );
    }
}

#[tokio::test]
async fn an_unreadable_state_record_is_a_refusal_naming_the_table() {
    let store = MemoryStore::new();
    migrate::migrate(&store, &catalog(users(true)))
        .await
        .unwrap();

    // Overwrite the state key with something that is not a state record.
    let txn = store.begin().await.unwrap();
    txn.put(
        slate_kernel::keys::meta_key(USERS),
        slate_tuple::encode(&[
            Value::U64(99),
            Value::U64(1),
            Value::U64(1),
            Value::Bytes(Vec::new().into()),
        ]),
    )
    .unwrap();
    txn.commit().await.unwrap();

    let plan = migrate::plan(&store, &catalog(users(true))).await.unwrap();
    assert!(
        matches!(
            plan.refusals.as_slice(),
            [Refusal::UnknownFormat { format: 99, .. }]
        ),
        "{plan:?}"
    );
    assert!(
        plan.why_blocked().contains("users"),
        "{}",
        plan.why_blocked()
    );
}

/// Every `ValueType` fingerprints differently from every other.
///
/// `type_code` is written out by hand precisely so that a type cannot be
/// renumbered by accident, and it ends in `_ => 0` so a variant added upstream
/// compiles. That arm is deliberate and it is also a trap: a type nobody adds
/// a line for takes code 0, and 0 is shared, so two such columns fingerprint
/// identically and a migration between them is invisible. Nothing checked
/// that, because until `ValueType::ALL` existed there was no way to loop over
/// the type space — a test naming nine types by hand would have gone stale
/// exactly the way the thing it was checking does.
///
/// Through `fingerprint` rather than `type_code`, which is private: the
/// property that matters is that two schemas differing only in a column's type
/// hash apart, and that is what a caller sees.
#[test]
fn every_value_type_fingerprints_apart() {
    let mut seen: std::collections::BTreeMap<u64, ValueType> = std::collections::BTreeMap::new();
    for kind in ValueType::ALL {
        let builder = TableDef::builder("t", USERS).column("id", ValueType::U64);
        // An array column must declare what it holds, so it cannot be built
        // the same way as the other nine. The element type is hashed as well
        // as the array's own code, which is why it is pinned here rather than
        // varied: this test is about `type_code`, and
        // `the_element_type_is_part_of_the_fingerprint` is about the element.
        let builder = if kind == ValueType::Array {
            builder.array_column("v", ValueType::Bool)
        } else {
            builder.column("v", kind)
        };
        let table = builder
            .primary_key(["id"])
            .build()
            .unwrap_or_else(|e| panic!("a table with a {kind} column should build: {e}"));
        let hash = migrate::fingerprint(&table);
        if let Some(clash) = seen.insert(hash, kind) {
            panic!(
                "a {kind} column and a {clash} column fingerprint identically \
                 ({hash:#x}); give {kind} its own arm in type_code"
            );
        }
    }
    assert_eq!(
        seen.len(),
        ValueType::ALL.len(),
        "one fingerprint per type, and no type skipped"
    );
}

#[test]
fn the_fingerprint_ignores_names_and_notices_layout() {
    let base = users(false);
    let same = users(false);
    assert_eq!(migrate::fingerprint(&base), migrate::fingerprint(&same));

    let renamed = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email_address", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap();
    assert_eq!(
        migrate::fingerprint(&base),
        migrate::fingerprint(&renamed),
        "a rename moves no bytes and must not look like a migration"
    );

    // Nullability changes how a value is read back, so it counts.
    let nullable = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .nullable_column("email", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap();
    assert_ne!(migrate::fingerprint(&base), migrate::fingerprint(&nullable));

    // So does the primary key: it is the row key. Two cases, because they fail
    // separately — a key of a different *length* and a key of the same length
    // over a different column. Only the second catches a fingerprint that
    // hashes how many key columns there are and not which.
    let widened_key = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["id", "email"])
        .build()
        .unwrap();
    assert_ne!(
        migrate::fingerprint(&base),
        migrate::fingerprint(&widened_key)
    );

    let moved_key = TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::Str)
        .primary_key(["email"])
        .build()
        .unwrap();
    assert_eq!(moved_key.primary_key().len(), base.primary_key().len());
    assert_ne!(
        migrate::fingerprint(&base),
        migrate::fingerprint(&moved_key),
        "the key moved to another column; the length is the same and the row keys are not"
    );

    // And the tenant column, which is what decides whether index keys carry a
    // tenant prefix. `None` and `Some(Ordinal(0))` must differ: a table whose
    // tenant is its first key column lays its index out differently from one
    // with no tenant scoping at all, and both have the same columns.
    let plain_pair = TableDef::builder("scoped", TableId(3))
        .column("tenant", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["tenant", "name"])
        .build()
        .unwrap();
    let scoped = TableDef::builder("scoped", TableId(3))
        .column("tenant", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["tenant", "name"])
        .tenant_column("tenant")
        .build()
        .unwrap();
    assert_ne!(
        migrate::fingerprint(&plain_pair),
        migrate::fingerprint(&scoped),
        "tenant scoping changes the index key layout and must not fingerprint the same"
    );

    // An index is not in the fingerprint at all: it is tracked by name in the
    // built list, and adding one is a step rather than a refusal.
    assert_eq!(
        migrate::fingerprint(&base),
        migrate::fingerprint(&users(true))
    );
}

#[test]
fn a_plan_with_a_refusal_is_blocked_and_says_so() {
    let plan = MigrationPlan {
        steps: vec![Step::Register {
            table: USERS,
            name: "users".into(),
        }],
        refusals: vec![Refusal::LayoutChanged {
            table: "users".into(),
            stored: 1,
            current: 2,
            // No stored schema, which is the case this assertion is about:
            // being blocked does not depend on being able to say why.
            changes: vec![],
        }],
    };
    assert!(plan.is_blocked());
    assert!(!plan.is_empty());
    assert!(plan.why_blocked().contains("users"));
}

// --- the stored schema -------------------------------------------------------

/// Every layout the fingerprint moves for, the stored schema names — and
/// nothing else moves either.
///
/// This is the property that justifies storing exactly the hash's inputs, and
/// `docs/persisting-the-schema.md` says it should be a test rather than a hope:
/// *every difference the fingerprint detects is one the stored schema can name,
/// and every difference the stored schema can name is one the fingerprint
/// detects.* Both directions, over one list, so a change to either side that
/// breaks the correspondence fails here.
///
/// A rename is in the list with `same: true` — it is the one difference the
/// stored schema holds and deliberately does not report, which is the whole
/// reason names are stored and not hashed.
#[test]
fn every_layout_change_moves_the_fingerprint() {
    let base = users(false);
    let cases: Vec<(&str, TableDef, bool)> = vec![
        (
            "a rename",
            TableDef::builder("users", USERS)
                .column("id", ValueType::U64)
                .column("email_address", ValueType::Str)
                .primary_key(["id"])
                .build()
                .unwrap(),
            true,
        ),
        (
            "a retype",
            TableDef::builder("users", USERS)
                .column("id", ValueType::U64)
                .column("email", ValueType::U64)
                .primary_key(["id"])
                .build()
                .unwrap(),
            false,
        ),
        (
            "nullability",
            TableDef::builder("users", USERS)
                .column("id", ValueType::U64)
                .nullable_column("email", ValueType::Str)
                .primary_key(["id"])
                .build()
                .unwrap(),
            false,
        ),
        (
            "a third column",
            TableDef::builder("users", USERS)
                .column("id", ValueType::U64)
                .column("email", ValueType::Str)
                .column("team", ValueType::U64)
                .primary_key(["id"])
                .build()
                .unwrap(),
            false,
        ),
        (
            "a wider key",
            TableDef::builder("users", USERS)
                .column("id", ValueType::U64)
                .column("email", ValueType::Str)
                .primary_key(["id", "email"])
                .build()
                .unwrap(),
            false,
        ),
        (
            "a tenant column",
            TableDef::builder("users", USERS)
                .column("id", ValueType::U64)
                .column("email", ValueType::Str)
                .primary_key(["id"])
                .tenant_column("id")
                .build()
                .unwrap(),
            false,
        ),
        (
            // Against the scale-2 baseline below. A decimal's scale decides
            // what its stored units *mean*, so it is a layout change in the
            // same sense a retype is.
            "a different scale",
            TableDef::builder("money", TableId(90))
                .column("id", ValueType::U64)
                .decimal_column("amount", 3)
                .primary_key(["id"])
                .build()
                .unwrap(),
            false,
        ),
        (
            "the same scale",
            TableDef::builder("money", TableId(90))
                .column("id", ValueType::U64)
                .decimal_column("amount", 2)
                .primary_key(["id"])
                .build()
                .unwrap(),
            true,
        ),
    ];

    // The scale cases are against each other rather than against `users`,
    // which has no decimal column; everything else is against `base`.
    for (what, table, same) in cases {
        let against = if table.name() == "money" {
            TableDef::builder("money", TableId(90))
                .column("id", ValueType::U64)
                .decimal_column("amount", 2)
                .primary_key(["id"])
                .build()
                .unwrap()
        } else {
            base.clone()
        };
        let hashed_same = migrate::fingerprint(&against) == migrate::fingerprint(&table);
        let named = migrate::layout_changes(
            &migrate::StoredSchema::of(&against),
            &migrate::StoredSchema::of(&table),
        );
        assert_eq!(
            hashed_same,
            named.is_empty(),
            "{what}: the fingerprint says {}, the stored schema says {named:?}",
            if hashed_same { "same" } else { "moved" }
        );
        assert_eq!(hashed_same, same, "{what}: the fingerprint disagrees");
    }
}

/// The refusal names the column, which is the point of storing anything.
#[tokio::test]
async fn a_retyped_column_is_refused_by_name_rather_than_by_hash() {
    let store = MemoryStore::new();
    let before = Catalog::from_tables([users(false)]).unwrap();
    migrate::migrate(&store, &before).await.unwrap();

    let after = Catalog::from_tables([TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap()])
    .unwrap();
    let plan = migrate::plan(&store, &after).await.unwrap();
    assert_eq!(plan.refusals.len(), 1, "{:?}", plan.refusals);

    let Refusal::LayoutChanged { changes, .. } = &plan.refusals[0] else {
        panic!("expected a layout refusal, got {:?}", plan.refusals[0]);
    };
    assert_eq!(changes.len(), 1, "{changes:?}");
    let message = plan.refusals[0].to_string();
    // The column, by ordinal *and* by the name the database has — both,
    // because an ordinal alone is what the old message effectively gave and a
    // name alone does not survive a rename.
    assert!(message.contains("column 1 `email`"), "{message}");
    assert!(message.contains("was string"), "{message}");
    assert!(message.contains("is now u64"), "{message}");
    // And it no longer leads with two hex numbers, which is the thing this
    // replaced.
    assert!(!message.contains("0x"), "{message}");
}

/// A record written before stored schemas still refuses, and says so.
///
/// The permanent case, not a transition: an old binary's record, a restored
/// backup, a table nobody has migrated since the upgrade. The refusal falls
/// back to the hash and tells the reader which situation they are in.
#[tokio::test]
async fn a_record_with_no_stored_schema_falls_back_to_the_hash() {
    let store = MemoryStore::new();
    let before = Catalog::from_tables([users(false)]).unwrap();
    migrate::migrate(&store, &before).await.unwrap();

    // Rewrite the record as a version 1 one, which is what an older binary
    // would have left. Done through the public encoder rather than by hand so
    // the test cannot drift from the format.
    let txn = store.begin().await.unwrap();
    let key = slate_kernel::keys::meta_key(USERS);
    let old = migrate::TableState {
        schema_version: 0,
        fingerprint: migrate::fingerprint(&users(false)),
        built: vec![],
        schema: None,
    };
    txn.put(key.clone(), migrate::encode_state(&old)).unwrap();
    txn.commit().await.unwrap();

    let after = Catalog::from_tables([TableDef::builder("users", USERS)
        .column("id", ValueType::U64)
        .column("email", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap()])
    .unwrap();
    let plan = migrate::plan(&store, &after).await.unwrap();
    let Refusal::LayoutChanged { changes, .. } = &plan.refusals[0] else {
        panic!("expected a layout refusal, got {:?}", plan.refusals[0]);
    };
    assert!(changes.is_empty(), "{changes:?}");
    let message = plan.refusals[0].to_string();
    assert!(message.contains("predates stored schemas"), "{message}");
    // The hash is back in the message, because it is all there is.
    assert!(message.contains("0x"), "{message}");

    // And migrating once upgrades the record, so the *next* refusal names the
    // column. That is the lazy upgrade, end to end.
    migrate::migrate(&store, &before).await.unwrap();
    let plan = migrate::plan(&store, &after).await.unwrap();
    let Refusal::LayoutChanged { changes, .. } = &plan.refusals[0] else {
        panic!("expected a layout refusal, got {:?}", plan.refusals[0]);
    };
    assert_eq!(changes.len(), 1, "{changes:?}");
}

/// A version 1 record still reads, which is the compatibility promise.
#[tokio::test]
async fn a_version_one_record_reads_and_a_version_two_one_round_trips() {
    let store = MemoryStore::new();
    let catalog = Catalog::from_tables([users(true)]).unwrap();
    migrate::migrate(&store, &catalog).await.unwrap();

    let states = migrate::stored_state(&store, &catalog).await.unwrap();
    let state = states[0].1.as_ref().unwrap();
    let schema = state.schema.as_ref().expect("a fresh migration writes one");
    assert_eq!(*schema, migrate::StoredSchema::of(&users(true)));
    // The names are there, which is the addition to the hash's inputs.
    assert_eq!(schema.columns[1].name, "email");
    // And the indexes still round trip beside it, which is what the record
    // held before and must keep holding.
    assert_eq!(state.built, vec![BY_EMAIL]);
}

/// Every field of a stored column survives the round trip, over a table that
/// actually has one of each.
///
/// The test above round-trips `users`, whose two columns are non-null, plain
/// `u64` and `str` with no scale, no element type, no droppedness and no
/// tenant column — so four of `StoredColumn`'s six fields and the tenant tag
/// were written, read and compared against zero on both sides. Four mutations
/// that dropped a field from the encoder survived the whole suite.
///
/// What they would have cost is not an unreadable record: it is a table that
/// migrates once and is **refused on every startup after**, because the
/// decoded schema disagrees with the one computed from the catalog in a field
/// that never reached the keyspace. That is the second assertion here, and it
/// is the one an operator would have met.
#[tokio::test]
async fn every_stored_column_field_survives_the_round_trip() {
    fn rich() -> TableDef {
        TableDef::builder("rich", TableId(3))
            .column("tenant", ValueType::U64)
            .column("id", ValueType::U64)
            // Nullability.
            .nullable_column("note", ValueType::Str)
            // A scale.
            .decimal_column("price", 2)
            // An element type.
            .array_column("tags", ValueType::Str)
            // And droppedness, which needs a column to have been there first.
            .column("legacy", ValueType::Str)
            .drop_column("legacy", 2)
            .primary_key(["tenant", "id"])
            .tenant_column("tenant")
            .schema_version(2)
            .build()
            .unwrap()
    }

    let store = MemoryStore::new();
    let catalog = Catalog::from_tables([rich()]).unwrap();
    migrate::migrate(&store, &catalog).await.unwrap();

    let states = migrate::stored_state(&store, &catalog).await.unwrap();
    let state = states[0].1.as_ref().unwrap();
    let schema = state.schema.as_ref().expect("a fresh migration writes one");
    // The oracle: what came back is what the catalog says, field for field,
    // rather than a list of assertions naming the fields somebody remembered.
    assert_eq!(*schema, migrate::StoredSchema::of(&rich()));

    // And the symptom, which is what a lost field actually costs.
    let again = migrate::plan(&store, &catalog).await.unwrap();
    assert!(again.is_empty(), "a second startup found work: {again:?}");
}

/// A record that says version 1 and carries more than a version 1 record is
/// corrupt, not a version 2 one.
///
/// The format byte decides, and the bytes after it must agree. Reading the
/// extra as a schema anyway would mean a record could claim any version and be
/// parsed as whatever its length suggested, which is the misparse the format
/// byte exists to prevent — the module's own opening argument.
#[tokio::test]
async fn a_version_one_record_with_more_after_it_is_refused() {
    let store = MemoryStore::new();
    let catalog = Catalog::from_tables([users(false)]).unwrap();
    migrate::migrate(&store, &catalog).await.unwrap();

    // A version 1 record, with a version 2 record's tail glued on. Built from
    // the two encoders rather than by hand, so the test cannot drift from the
    // format it is about.
    let v1 = migrate::encode_state(&migrate::TableState {
        schema_version: 0,
        fingerprint: migrate::fingerprint(&users(false)),
        built: vec![],
        schema: None,
    });
    let v2 = migrate::encode_state(&migrate::TableState {
        schema_version: 0,
        fingerprint: migrate::fingerprint(&users(false)),
        built: vec![],
        schema: Some(migrate::StoredSchema::of(&users(false))),
    });
    let mut mixed = v1.clone();
    mixed.extend_from_slice(&v2[v1.len()..]);

    let txn = store.begin().await.unwrap();
    txn.put(slate_kernel::keys::meta_key(USERS), mixed).unwrap();
    txn.commit().await.unwrap();

    let states = migrate::stored_state(&store, &catalog).await.unwrap();
    let Err(Refusal::Corrupt { detail, .. }) = &states[0].1 else {
        panic!("expected a corrupt refusal, got {:?}", states[0].1);
    };
    assert!(detail.contains("bytes after it"), "{detail}");
}

/// The schema blob carries its own version, and a foreign one is refused.
///
/// Its own rather than leaning on the outer format byte, because the outer one
/// says how the *record* is laid out and this says what a column record holds.
/// A blob from a newer binary would decode into plausible nonsense without
/// this — a column count read out of a name, and so on — which is the misparse
/// the outer format byte already exists to prevent, one level down.
#[tokio::test]
async fn a_schema_blob_from_a_newer_binary_is_refused_rather_than_misread() {
    let store = MemoryStore::new();
    let catalog = Catalog::from_tables([users(false)]).unwrap();
    migrate::migrate(&store, &catalog).await.unwrap();

    // The version 1 prefix of a version 2 record, then a blob claiming a
    // version this binary does not know. The prefix is taken from the real
    // encoder and only the blob is hand-built, so the test is about the blob.
    let with_schema = migrate::encode_state(&migrate::TableState {
        schema_version: 0,
        fingerprint: migrate::fingerprint(&users(false)),
        built: vec![],
        schema: Some(migrate::StoredSchema::of(&users(false))),
    });
    let without = migrate::encode_state(&migrate::TableState {
        schema_version: 0,
        fingerprint: migrate::fingerprint(&users(false)),
        built: vec![],
        schema: None,
    });
    let prefix_len = without.len();
    let mut future = with_schema[..prefix_len].to_vec();
    future.extend_from_slice(&slate_tuple::encode(&[Value::U64(99), Value::U64(0)]));

    let txn = store.begin().await.unwrap();
    txn.put(slate_kernel::keys::meta_key(USERS), future)
        .unwrap();
    txn.commit().await.unwrap();

    let states = migrate::stored_state(&store, &catalog).await.unwrap();
    let Err(Refusal::Corrupt { detail, .. }) = &states[0].1 else {
        panic!("expected a corrupt refusal, got {:?}", states[0].1);
    };
    assert!(detail.contains("version 99"), "{detail}");

    // And bytes after a blob that otherwise decodes are refused too, for the
    // reason the outer record refuses them: a record that parses a prefix and
    // ignores the rest cannot tell a longer format from a corrupt one.
    let mut trailing = with_schema.clone();
    trailing.extend_from_slice(&slate_tuple::encode(&[Value::U64(0)]));
    let txn = store.begin().await.unwrap();
    txn.put(slate_kernel::keys::meta_key(USERS), trailing)
        .unwrap();
    txn.commit().await.unwrap();

    let states = migrate::stored_state(&store, &catalog).await.unwrap();
    let Err(Refusal::Corrupt { detail, .. }) = &states[0].1 else {
        panic!("expected a corrupt refusal, got {:?}", states[0].1);
    };
    assert!(detail.contains("after the schema"), "{detail}");
}
