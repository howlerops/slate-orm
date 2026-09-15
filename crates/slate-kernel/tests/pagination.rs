//! Keyset pagination: pages that cost the same and do not shift.
//!
//! Two claims, and they are tested differently because they fail differently.
//!
//! The **cost** claim is that page N costs what page 1 costs. No assertion
//! about the rows can see it — an offset returns exactly the same page — so the
//! store is wrapped in a counter and the reads are counted.
//!
//! The **correctness** claim is that a row inserted or deleted ahead of the
//! cursor cannot shift a page. That one *is* visible in the rows, and the test
//! shows the offset version getting it wrong beside the cursor version getting
//! it right, because a test that only showed the right answer would not
//! establish that there was ever a problem.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use bytes::Bytes;
use slate_kernel::memory::MemoryStore;
use slate_kernel::store::{KeyRange, KvIterator, KvSnapshot, KvStore, KvTransaction, ScanOrder};
use slate_kernel::{
    AccessSummary, Action, CmpOp, Expr, Grant, KernelError, Principal, Query, RecordStore,
    SecurityCatalog, SecurityContext, SortKey,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as Atomics};

const NOTES: TableId = TableId(1);
const BY_KIND: IndexId = IndexId(10);

/// A store that counts the key-value pairs read through it.
///
/// Pairs rather than scans: both pagination styles open exactly one cursor, so
/// counting scans would report them identical. What differs is how far each one
/// walks, and that is the thing the feature exists to change.
struct Counting {
    inner: MemoryStore,
    pairs: Arc<AtomicUsize>,
}

struct CountingTxn<'a> {
    inner: Box<dyn KvTransaction + Send + 'a>,
    pairs: Arc<AtomicUsize>,
}

struct CountingIter<'a> {
    inner: Box<dyn KvIterator + Send + 'a>,
    pairs: Arc<AtomicUsize>,
}

impl Clone for Counting {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            pairs: Arc::clone(&self.pairs),
        }
    }
}

impl Counting {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            pairs: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn pairs(&self) -> usize {
        self.pairs.load(Atomics::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvIterator for CountingIter<'_> {
    async fn next(&mut self) -> slate_kernel::Result<Option<slate_kernel::store::KeyValue>> {
        let pair = self.inner.next().await?;
        if pair.is_some() {
            self.pairs.fetch_add(1, Atomics::SeqCst);
        }
        Ok(pair)
    }
}

#[async_trait::async_trait]
impl KvStore for Counting {
    async fn begin(&self) -> slate_kernel::Result<Box<dyn KvTransaction + Send + '_>> {
        Ok(Box::new(CountingTxn {
            inner: self.inner.begin().await?,
            pairs: Arc::clone(&self.pairs),
        }))
    }
}

#[async_trait::async_trait]
impl KvSnapshot for CountingTxn<'_> {
    async fn get(&self, key: &[u8]) -> slate_kernel::Result<Option<Bytes>> {
        let found = self.inner.get(key).await?;
        if found.is_some() {
            self.pairs.fetch_add(1, Atomics::SeqCst);
        }
        Ok(found)
    }
    async fn scan(
        &self,
        range: KeyRange,
        order: ScanOrder,
    ) -> slate_kernel::Result<Box<dyn KvIterator + Send + '_>> {
        Ok(Box::new(CountingIter {
            inner: self.inner.scan(range, order).await?,
            pairs: Arc::clone(&self.pairs),
        }))
    }
    fn is_point_in_time(&self) -> bool {
        self.inner.is_point_in_time()
    }
}

#[async_trait::async_trait]
impl KvTransaction for CountingTxn<'_> {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> slate_kernel::Result<()> {
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

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", BY_KIND).column("kind"))
        .build()
        .unwrap()
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

fn store(inner: Counting, table: &TableDef) -> RecordStore<Counting> {
    RecordStore::new(
        inner,
        Catalog::from_tables([table.clone()]).unwrap(),
        SecurityCatalog::new().grant(Grant::new("app", NOTES, Action::EVERYTHING)),
    )
}

async fn seed(records: &RecordStore<Counting>, table: &TableDef, ids: impl Iterator<Item = u64>) {
    let txn = records.begin().await.unwrap();
    for id in ids {
        txn.insert(
            &context(),
            table,
            &Row::new(vec![
                Value::U64(id),
                Value::Str(if id % 2 == 0 { "even" } else { "odd" }.into()),
                Value::Str(format!("body {id}")),
            ]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
}

async fn run(records: &RecordStore<Counting>, table: &TableDef, query: Query) -> Vec<u64> {
    let txn = records.begin().await.unwrap();
    let rows = txn
        .execute(&context(), table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    txn.rollback();
    rows.iter()
        .map(|row| match row.values()[0] {
            Value::U64(id) => id,
            ref other => panic!("id is not a u64: {other:?}"),
        })
        .collect()
}

async fn refused(records: &RecordStore<Counting>, table: &TableDef, query: Query) -> KernelError {
    let txn = records.begin().await.unwrap();
    let error = txn
        .execute(&context(), table, &query)
        .await
        .expect_err("this query should have been refused");
    txn.rollback();
    error
}

#[tokio::test]
async fn a_cursor_walks_the_table_a_page_at_a_time() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=10).await;

    let mut seen: Vec<u64> = Vec::new();
    let mut cursor: Option<Value> = None;
    let mut requests = 0usize;
    loop {
        let mut query = Query::all().limit(3);
        if let Some(at) = &cursor {
            query = query.after(vec![at.clone()]);
        }
        let page = run(&records, &table, query).await;
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 3, "{page:?}");
        cursor = Some(Value::U64(page[page.len() - 1]));
        seen.extend(page);

        // The bound is load-bearing rather than defensive, and a mutation
        // proved it: make the cursor *inclusive* and the last page repeats its
        // own final row for ever, so this loop never ends. A hanging test is
        // worse than a failing one — it reads as broken infrastructure rather
        // than a broken assertion, and telling the two apart here took a
        // stalled run and a look at `/proc`.
        requests += 1;
        assert!(
            requests <= 10,
            "paging ten rows three at a time has taken {requests} requests; \
             the cursor is not advancing"
        );
    }
    // Every row once, in order, and the loop ended on its own rather than on a
    // count the test knew in advance.
    assert_eq!(seen, (1..=10).collect::<Vec<u64>>());
}

#[tokio::test]
async fn a_page_costs_the_same_however_far_in_it_is() {
    let table = notes();
    let counting = Counting::new();
    let records = store(counting.clone(), &table);
    seed(&records, &table, 1..=500).await;

    // The last page, reached by offset. Every row before it is read and thrown
    // away — the executor says so in a comment, and this is the measurement of
    // what that costs.
    let before = counting.pairs();
    let by_offset = run(&records, &table, Query::all().limit(5).offset(490)).await;
    let offset_pairs = counting.pairs() - before;

    // The same page, reached by the key the previous page ended on.
    let before = counting.pairs();
    let by_cursor = run(
        &records,
        &table,
        Query::all().limit(5).after(vec![Value::U64(490)]),
    )
    .await;
    let cursor_pairs = counting.pairs() - before;

    // Identical rows — which is why no assertion about the rows could have
    // caught the difference.
    assert_eq!(by_offset, by_cursor);
    assert_eq!(by_offset, vec![491, 492, 493, 494, 495]);

    println!("PAGE 99 OF 100: offset read {offset_pairs} pairs, cursor read {cursor_pairs}");
    // The bound is deliberately loose. The claim is "the cursor does not pay
    // for the rows it skips", not a ratio — a ratio would pin the prefetch
    // depth and the row encoding, neither of which this test is about.
    assert!(
        cursor_pairs * 10 < offset_pairs,
        "offset read {offset_pairs} pairs and the cursor read {cursor_pairs}; \
         the cursor is supposed to skip the rows before it, not read them"
    );
    // And the honest lower bound: a page of five rows cannot be free.
    assert!(cursor_pairs >= 5, "{cursor_pairs}");
}

#[tokio::test]
async fn a_row_deleted_ahead_of_the_cursor_does_not_skip_one() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=10).await;

    // Page one, both ways: the same three rows.
    assert_eq!(
        run(&records, &table, Query::all().limit(3)).await,
        vec![1, 2, 3]
    );

    // Someone deletes row 1 between the pages. This is the ordinary case, not
    // a contrived one: it is what a list of anything editable does all day.
    let txn = records.begin().await.unwrap();
    txn.delete(&context(), &table, &[Value::U64(1)])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    // By offset, row 4 is silently skipped: it has moved into the window the
    // first page already returned. Nothing reports this, and the reader never
    // sees row 4 at all.
    let skipped = run(&records, &table, Query::all().limit(3).offset(3)).await;
    assert_eq!(skipped, vec![5, 6, 7], "{skipped:?}");

    // By cursor, page two is what it should be. The key of the last row seen
    // did not move when its neighbour was deleted.
    let correct = run(
        &records,
        &table,
        Query::all().limit(3).after(vec![Value::U64(3)]),
    )
    .await;
    assert_eq!(correct, vec![4, 5, 6], "{correct:?}");
}

#[tokio::test]
async fn a_cursor_is_exclusive_so_no_row_is_served_twice() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=6).await;

    let first = run(&records, &table, Query::all().limit(3)).await;
    let second = run(
        &records,
        &table,
        Query::all().limit(3).after(vec![Value::U64(first[2])]),
    )
    .await;
    // An inclusive bound would repeat row 3 here, which is the bug people write
    // when they reach for this by hand.
    assert_eq!(first, vec![1, 2, 3]);
    assert_eq!(second, vec![4, 5, 6]);
}

#[tokio::test]
async fn a_descending_cursor_resumes_downwards() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=6).await;

    let first = run(
        &records,
        &table,
        Query::all().order(ScanOrder::Descending).limit(2),
    )
    .await;
    assert_eq!(first, vec![6, 5]);

    // "After" follows the scan order, so descending it means *below*. Applying
    // the ascending bound here would return nothing at all, which is the shape
    // of mistake a test with one direction never finds.
    let second = run(
        &records,
        &table,
        Query::all()
            .order(ScanOrder::Descending)
            .limit(2)
            .after(vec![Value::U64(5)]),
    )
    .await;
    assert_eq!(second, vec![4, 3]);
}

#[tokio::test]
async fn a_cursor_composes_with_a_filter() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=12).await;

    let only_even = Expr::compare(Ordinal(1), CmpOp::Eq, Value::Str("even".into()));
    let first = run(
        &records,
        &table,
        Query::all().filter(only_even.clone()).limit(3),
    )
    .await;
    assert_eq!(first, vec![2, 4, 6]);

    // The cursor narrows the range and the filter still runs on what is left,
    // so the second page is the next three *matching* rows — not the rows after
    // the third match regardless of whether they match.
    let second = run(
        &records,
        &table,
        Query::all()
            .filter(only_even)
            .limit(3)
            .after(vec![Value::U64(6)]),
    )
    .await;
    assert_eq!(second, vec![8, 10, 12]);
}

#[tokio::test]
async fn a_cursor_past_the_end_is_an_empty_page_not_an_error() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=3).await;
    assert!(
        run(
            &records,
            &table,
            Query::all().limit(3).after(vec![Value::U64(99)])
        )
        .await
        .is_empty()
    );
    // And a cursor on a key that was deleted still works: what matters is where
    // the key *sorts*, not whether a row is there.
    assert_eq!(
        run(
            &records,
            &table,
            Query::all().limit(3).after(vec![Value::U64(1)])
        )
        .await,
        vec![2, 3]
    );
}

#[tokio::test]
async fn a_cursor_applies_to_a_single_row_read_too() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=6).await;

    // A filter pinning the whole primary key plans as a point read, not a
    // range — so the cursor cannot narrow bounds and has to decide the one
    // row's fate directly. Both answers are asserted, because a version that
    // simply kept the row passed every other test in this file: the ranges are
    // where the interesting cases are, and this path has none.
    let inside = run(
        &records,
        &table,
        Query::all()
            .filter(Expr::compare(Ordinal(0), CmpOp::Eq, Value::U64(5)))
            .after(vec![Value::U64(3)]),
    )
    .await;
    assert_eq!(
        inside,
        vec![5],
        "row 5 is after the cursor and should be kept"
    );

    let behind = run(
        &records,
        &table,
        Query::all()
            .filter(Expr::compare(Ordinal(0), CmpOp::Eq, Value::U64(5)))
            .after(vec![Value::U64(5)]),
    )
    .await;
    assert!(
        behind.is_empty(),
        "the cursor is exclusive, so the row it names is behind it: {behind:?}"
    );

    // And downwards, where the comparison reverses.
    let below = run(
        &records,
        &table,
        Query::all()
            .order(ScanOrder::Descending)
            .filter(Expr::compare(Ordinal(0), CmpOp::Eq, Value::U64(2)))
            .after(vec![Value::U64(4)]),
    )
    .await;
    assert_eq!(below, vec![2], "descending, row 2 is after a cursor at 4");
}

#[tokio::test]
async fn a_read_the_cursor_rules_out_explains_as_reading_nothing() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=50).await;
    let txn = records.begin().await.unwrap();

    // The same correction `plan_hinted` already makes for a contradictory
    // range, and for the reason recorded there: a plan that reads no rows but
    // is costed as though it read the table makes EXPLAIN describe a query
    // nobody ran.
    //
    // Reached through a *point* read, and that is not incidental. A cursor past
    // the last row does **not** empty the range — the bounds are still a
    // perfectly good slice of the keyspace, it simply has nothing in it, and no
    // planner can know that without reading. The one case where a cursor makes
    // the plan provably read nothing is a single-row read whose row is behind
    // it. An earlier version of this test asserted the first case and failed,
    // which is how that distinction got established rather than assumed.
    let ruled_out = txn
        .explain(
            &context(),
            &table,
            &Query::all()
                .filter(Expr::compare(Ordinal(0), CmpOp::Eq, Value::U64(5)))
                .after(vec![Value::U64(40)]),
        )
        .unwrap();
    assert_eq!(ruled_out.estimated_rows, 0.0, "{ruled_out:?}");
    assert_eq!(ruled_out.estimated_cost, 0.0, "{ruled_out:?}");

    // The comparison that keeps the assertion honest: the same point read with
    // the cursor *behind* the row is costed as reading something.
    let kept = txn
        .explain(
            &context(),
            &table,
            &Query::all()
                .filter(Expr::compare(Ordinal(0), CmpOp::Eq, Value::U64(5)))
                .after(vec![Value::U64(1)]),
        )
        .unwrap();
    assert!(kept.estimated_rows > 0.0, "{kept:?}");
    txn.rollback();
}

#[tokio::test]
async fn a_sorted_query_is_refused_rather_than_paged_wrongly() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=6).await;

    // Sorted by `kind`, which the primary key says nothing about. The rows are
    // re-ordered after they are read, so "after key 3" does not name a position
    // in the output — applying it to the input would drop rows the sort would
    // have put on this page.
    let error = refused(
        &records,
        &table,
        Query::all()
            .sort_by([SortKey::asc(Ordinal(1))])
            .limit(2)
            .after(vec![Value::U64(3)]),
    )
    .await;
    assert!(
        matches!(error, KernelError::InvalidCursor { .. }),
        "{error}"
    );
    let said = error.to_string();
    assert!(said.contains("primary key"), "{said}");
    assert!(said.contains("offset"), "{said}");
}

#[tokio::test]
async fn a_cursor_of_the_wrong_width_is_refused() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=3).await;
    let error = refused(
        &records,
        &table,
        Query::all().after(vec![Value::U64(1), Value::U64(2)]),
    )
    .await;
    assert!(
        matches!(error, KernelError::InvalidCursor { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("1 key column"), "{error}");
}

#[tokio::test]
async fn a_cursor_on_a_grouped_read_is_refused_rather_than_ignored() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=6).await;

    let txn = records.begin().await.unwrap();
    let error = txn
        .aggregate(
            &context(),
            &table,
            &Query::all().after(vec![Value::U64(3)]),
            &[slate_kernel::Aggregate::Count],
        )
        .await
        .expect_err("a cursor on an aggregate should be refused");
    txn.rollback();
    assert!(
        matches!(error, KernelError::InvalidCursor { .. }),
        "{error}"
    );
    // The failure this refusal prevents is a *plausible* wrong number, which is
    // why it is a refusal and not a dropped field.
    assert!(error.to_string().contains("groups"), "{error}");
}

#[tokio::test]
async fn a_cursor_pins_the_access_path_to_the_primary_key() {
    // A *selective* indexed column, and real statistics, because without both
    // of those the planner picks a table scan anyway and this test is about
    // nothing. An earlier version asserted only the rows and a mutation
    // removing the pinning survived it; the version after that asserted the
    // plan and failed, which is how the fixture came to look like this.
    let table = TableDef::builder("notes", NOTES)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", BY_KIND).column("kind"))
        .build()
        .unwrap();
    let records = store(Counting::new(), &table);

    // One row in two hundred carries the value being filtered on.
    let txn = records.begin().await.unwrap();
    for id in 1..=200u64 {
        txn.insert(
            &context(),
            &table,
            &Row::new(vec![
                Value::U64(id),
                Value::Str(if id == 150 { "rare" } else { "common" }.into()),
                Value::Str(format!("body {id}")),
            ]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = records.begin().await.unwrap();
    let stats = txn.analyze(&context(), &table).await.unwrap();
    txn.rollback();
    let records = store(Counting::new(), &table)
        .with_statistics(slate_kernel::Statistics::default().with(NOTES, stats));
    let txn = records.begin().await.unwrap();
    for id in 1..=200u64 {
        txn.insert(
            &context(),
            &table,
            &Row::new(vec![
                Value::U64(id),
                Value::Str(if id == 150 { "rare" } else { "common" }.into()),
                Value::Str(format!("body {id}")),
            ]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = records.begin().await.unwrap();
    // Projected to the indexed column and the key, so the index answers
    // without reading a row at all. That is what makes it decisively cheaper
    // than a scan here — a filter being selective was not enough on its own,
    // which is worth knowing: on this cost model a scan of two hundred rows
    // beats an index lookup plus two hundred row reads.
    let selective = Query::all()
        .filter(Expr::compare(
            Ordinal(1),
            CmpOp::Eq,
            Value::Str("rare".into()),
        ))
        .select([Ordinal(0), Ordinal(1)])
        .limit(3);
    let without = txn.explain(&context(), &table, &selective).unwrap();
    assert!(
        matches!(
            without.access,
            AccessSummary::IndexScan { .. } | AccessSummary::IndexOnlyScan { .. }
        ),
        "this query is supposed to reach for the index, and reached for {:?} — \
         the rest of this test means nothing without that",
        without.access
    );

    // The same query with a cursor plans as a table scan instead, because a
    // page boundary named by a primary key cannot describe a position in an
    // index's order. Which plan runs follows from the request, not from how
    // selective the filter happens to be.
    let paged = selective.after(vec![Value::U64(100)]);
    let with = txn.explain(&context(), &table, &paged).unwrap();
    assert!(
        matches!(with.access, AccessSummary::TableScan),
        "a cursor should have pinned this to the key range, and it planned {:?}",
        with.access
    );
    txn.rollback();

    let page = run(&records, &table, paged).await;
    assert_eq!(page, vec![150], "{page:?}");
}

#[tokio::test]
async fn an_explicit_index_hint_with_a_cursor_is_refused_rather_than_overridden() {
    let table = notes();
    let records = store(Counting::new(), &table);
    seed(&records, &table, 1..=20).await;

    // The cursor *defaults* the access path; it does not override a caller who
    // named one. Asking for an index and a cursor together is a contradiction —
    // rows in index order, a boundary in key order — and the answer is no,
    // rather than quietly doing something else than what was asked.
    let error = refused(
        &records,
        &table,
        Query::all()
            .using_index(BY_KIND)
            .limit(3)
            .after(vec![Value::U64(5)]),
    )
    .await;
    assert!(
        matches!(error, KernelError::InvalidCursor { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("index"), "{error}");
}
