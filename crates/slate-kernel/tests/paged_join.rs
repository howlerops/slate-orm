//! Paging a join and a chain: the property, the refusals, and the three
//! measurements the design rests on.
//!
//! The property is the item. `docs/orm-comparison.md`'s N3 says so:
//!
//! > Paging through with a cursor under concurrent inserts visits every row
//! > exactly once. That property is the whole item; if a design cannot be
//! > tested that way, it is the wrong design.
//!
//! The design is in `docs/paging-a-join.md`. Its one line: **a page of a join
//! is a page of its driving table**, so the cursor is the left input's primary
//! key and the left input is given the window it already honours. Every joined
//! row derives from exactly one left row, so visiting every left row once
//! visits every joined row once — and that reduces the property here to the
//! one `Query::after` already holds.
//!
//! The three measurements at the bottom of this file are the ones that chose
//! the design, and two of them contradicted the comments that were in the code
//! when they were taken.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};

use slate_kernel::{
    Action, Chain, Grant, Join, JoinAlgorithm, JoinStep, JoinType, Query, RecordStore,
    SecurityCatalog, SecurityContext, Side, SortKey, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);
const REVIEWS: TableId = TableId(3);

fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("id", ValueType::U64)
        .column("region", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_region", IndexId(10)).column("region"))
        .build()
        .expect("valid schema")
}

fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("id", ValueType::U64)
        .column("author_id", ValueType::U64)
        .column("pages", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_author", IndexId(20)).column("author_id"))
        .build()
        .expect("valid schema")
}

fn reviews() -> TableDef {
    TableDef::builder("reviews", REVIEWS)
        .column("id", ValueType::U64)
        .column("book_id", ValueType::U64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_book", IndexId(30)).column("book_id"))
        .build()
        .expect("valid schema")
}

fn author(id: u64) -> Row {
    Row::new(vec![Value::U64(id), Value::Str(format!("r{}", id % 3))])
}

/// Fan-out on purpose, and uneven: `id % 9` over 20 books gives some authors
/// three books, some two, and author 8 none of the first twenty — so a page
/// boundary lands inside a group and one left row matches nothing.
fn book(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::U64(id % 9),
        Value::I64((id % 7) as i64 * 10),
    ])
}

fn review(id: u64) -> Row {
    Row::new(vec![Value::U64(id), Value::U64(id % 20)])
}

const AUTHOR_ROWS: u64 = 8;
const BOOK_ROWS: u64 = 20;
const REVIEW_ROWS: u64 = 30;

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([authors(), books(), reviews()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", AUTHORS, Action::ALL))
        .grant(Grant::new("r", BOOKS, Action::ALL))
        .grant(Grant::new("r", REVIEWS, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for id in 0..AUTHOR_ROWS {
        txn.insert(&root, &authors(), &author(id)).await.unwrap();
    }
    for id in 0..BOOK_ROWS {
        txn.insert(&root, &books(), &book(id)).await.unwrap();
    }
    for id in 0..REVIEW_ROWS {
        txn.insert(&root, &reviews(), &review(id)).await.unwrap();
    }
    txn.commit().await.unwrap();
    store
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn u64_of(value: &Value) -> u64 {
    match value {
        Value::U64(v) => *v,
        other => panic!("expected u64, got {other:?}"),
    }
}

/// One page of a join: its rows as `(author id, book id)`, the cursor it ends
/// on, and how many left rows it read.
struct Page {
    rows: Vec<(u64, Option<u64>)>,
    cursor: Option<Vec<Value>>,
    left_rows: usize,
}

async fn page(
    store: &RecordStore<MemoryStore>,
    join: &Join,
    size: usize,
) -> Result<Page, slate_kernel::KernelError> {
    let txn = store.begin().await.unwrap();
    let (left, right) = (authors(), books());
    let mut cursor = txn.join(&root(), &left, &right, join).await?;
    let mut rows = Vec::new();
    while let Some(row) = cursor.next().await? {
        rows.push((
            u64_of(&row.left.as_ref().expect("left present").values()[0]),
            row.right.as_ref().map(|r| u64_of(&r.values()[0])),
        ));
    }
    let left_rows = cursor.driving_rows();
    // A short page proves there is nothing after it, so it gets no cursor —
    // the same rule the single-table path uses, and the same trade: reading
    // one row further to be sure would be paid on every page to save one empty
    // request at the end of a sequence most callers never finish.
    let cursor = (left_rows >= size)
        .then(|| cursor.page_end(&left))
        .flatten();
    Ok(Page {
        rows,
        cursor,
        left_rows,
    })
}

/// Walk every page of a join, calling `between` after each one.
async fn walk<F, Fut>(
    store: &RecordStore<MemoryStore>,
    make: impl Fn(Option<Vec<Value>>) -> Join,
    size: usize,
    mut between: F,
) -> Vec<(u64, Option<u64>)>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut seen = Vec::new();
    let mut after: Option<Vec<Value>> = None;
    for pages in 0..100 {
        assert!(
            pages < 99,
            "the walk did not terminate: the cursor is stuck"
        );
        let got = page(store, &make(after.clone()), size).await.unwrap();
        seen.extend(got.rows);
        let Some(next) = got.cursor else { break };
        after = Some(next);
        between(pages).await;
    }
    seen
}

fn paged_join(after: Option<Vec<Value>>, size: usize) -> Join {
    let join = Join::equating(Ordinal(0), Ordinal(1))
        .left(Query::all())
        .right(Query::all())
        .limit(size);
    match after {
        Some(key) => join.after(key),
        None => join.paging(),
    }
}

// --- the property ---------------------------------------------------------

/// Paging a join start to finish visits every joined row exactly once.
#[tokio::test]
async fn paging_a_join_visits_every_row_exactly_once() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let whole: Vec<(u64, Option<u64>)> = txn
        .join(
            &root(),
            &authors(),
            &books(),
            &Join::equating(Ordinal(0), Ordinal(1)),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                u64_of(&row.left.as_ref().unwrap().values()[0]),
                row.right.as_ref().map(|r| u64_of(&r.values()[0])),
            )
        })
        .collect();

    for size in 1..=5 {
        let mut paged = walk(&store, |after| paged_join(after, size), size, |_| async {}).await;
        let mut expected = whole.clone();
        paged.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            paged, expected,
            "paging in pages of {size} left rows did not reproduce the whole join"
        );
    }
}

/// The property N3 names: under concurrent inserts, paging visits every row
/// that existed throughout exactly once.
///
/// The inserts go *behind* the cursor — author ids below where the walk has
/// reached — which is the case `OFFSET` gets wrong. With an offset each
/// insertion shifts every later page by one and a row is served twice or
/// skipped; with a key the page boundary does not move. The assertion is on
/// the rows that existed before the walk began, because a row inserted
/// mid-walk may legitimately be seen or missed depending on where it lands.
#[tokio::test]
async fn paging_a_join_under_concurrent_inserts_visits_each_original_row_once() {
    let store = seeded().await;
    let before: BTreeSet<(u64, Option<u64>)> = {
        let txn = store.begin().await.unwrap();
        txn.join(
            &root(),
            &authors(),
            &books(),
            &Join::equating(Ordinal(0), Ordinal(1)),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                u64_of(&row.left.as_ref().unwrap().values()[0]),
                row.right.as_ref().map(|r| u64_of(&r.values()[0])),
            )
        })
        .collect()
    };

    let seen = walk(
        &store,
        |after| paged_join(after, 2),
        2,
        |round| {
            let store = &store;
            async move {
                // A book for an author the walk has already passed: this is
                // exactly the insertion that shifts an offset and does not
                // move a key.
                let txn = store.begin().await.unwrap();
                let id = BOOK_ROWS + round as u64;
                txn.insert(
                    &root(),
                    &books(),
                    &Row::new(vec![Value::U64(id), Value::U64(0), Value::I64(1)]),
                )
                .await
                .unwrap();
                txn.commit().await.unwrap();
            }
        },
    )
    .await;

    let mut counts: BTreeMap<(u64, Option<u64>), usize> = BTreeMap::new();
    for row in &seen {
        *counts.entry(*row).or_default() += 1;
    }
    for row in &before {
        assert_eq!(
            counts.get(row).copied().unwrap_or(0),
            1,
            "row {row:?} existed throughout and was visited {:?} times, not once",
            counts.get(row).copied().unwrap_or(0),
        );
    }
    for (row, count) in &counts {
        assert_eq!(*count, 1, "row {row:?} was visited {count} times");
    }
}

/// A page boundary never splits a left row's matches.
///
/// This is what makes the property above provable in one line rather than
/// argued: a joined row belongs to exactly one left row, and a left row
/// belongs to exactly one page.
#[tokio::test]
async fn a_page_boundary_falls_between_left_rows() {
    let store = seeded().await;
    let mut after: Option<Vec<Value>> = None;
    let mut all_pages: Vec<BTreeSet<u64>> = Vec::new();
    // Bounded, and the bound is an assertion rather than a quiet `break`.
    //
    // This was an unbounded `loop`, and a mutation dropping the cursor
    // (`paged.left.after = None`, so every page is page one) made it **hang**
    // rather than fail — the cursor came back non-empty forever. A test that
    // hangs under a mutation is a test that hangs CI, which is strictly worse
    // than one that fails: a failure names itself in seconds and a hang is
    // fifteen minutes and a timeout nobody can attribute. Every walk in this
    // file is bounded for that reason, and this is the one that was not.
    for guard in 0..50 {
        assert!(
            guard < 49,
            "the walk did not terminate: the cursor is stuck"
        );
        let got = page(&store, &paged_join(after.clone(), 3), 3)
            .await
            .unwrap();
        all_pages.push(got.rows.iter().map(|(left, _)| *left).collect());
        let Some(next) = got.cursor else { break };
        after = Some(next);
    }
    // No author appears on two pages: each page owns its left rows outright.
    for (i, page) in all_pages.iter().enumerate() {
        for (j, other) in all_pages.iter().enumerate() {
            if i < j {
                let shared: Vec<_> = page.intersection(other).collect();
                assert!(
                    shared.is_empty(),
                    "author(s) {shared:?} appear on both page {i} and page {j}"
                );
            }
        }
    }
}

/// A left row that matches nothing still advances the cursor.
///
/// The failure this pins is a caller looping forever: under an inner join such
/// a row is read and emits nothing, so a cursor taken from the *emitted* rows
/// would not move when a whole page matched nothing. The boundary comes from
/// the scan instead, which is the reason `page_end` tracks the left rows read.
#[tokio::test]
async fn a_page_whose_left_rows_match_nothing_still_advances() {
    let store = seeded().await;
    // Authors 100..104 exist and have no books at all.
    let txn = store.begin().await.unwrap();
    for id in 100..105u64 {
        txn.insert(&root(), &authors(), &author(id)).await.unwrap();
    }
    txn.commit().await.unwrap();

    // Start the walk past every author that has a book.
    let mut after = Some(vec![Value::U64(99)]);
    let mut pages = 0;
    let mut rows = 0;
    loop {
        let got = page(&store, &paged_join(after.clone(), 2), 2)
            .await
            .unwrap();
        pages += 1;
        rows += got.rows.len();
        assert!(
            pages < 20,
            "the walk did not terminate: the cursor is stuck"
        );
        let Some(next) = got.cursor else { break };
        assert_ne!(
            Some(&next),
            after.as_ref(),
            "the cursor did not move although the page read {} left rows",
            got.left_rows
        );
        after = Some(next);
    }
    assert_eq!(rows, 0, "those authors have no books");
    assert!(pages >= 3, "five authors in pages of two is at least three");
}

/// Both algorithms a paged join may run give the same pages.
///
/// The cost model picks one, and on this fixture it always picks the hash
/// join — which a mutation proved by deleting the nested loop's boundary
/// tracking and surviving the whole file. So the loop is forced here, and the
/// two are compared rather than each being checked alone: "they agree" is the
/// property `join_oracle.rs` establishes for the rows, and a paged read needs
/// it for the *pages* too, because the design rests on the algorithms agreeing
/// about which rows a windowed side yields.
#[tokio::test]
async fn both_streaming_algorithms_give_the_same_pages() {
    let store = seeded().await;
    let mut per_algorithm: Vec<Vec<(u64, Option<u64>)>> = Vec::new();
    let mut cursors: Vec<Vec<Option<Vec<Value>>>> = Vec::new();
    for algorithm in [
        JoinAlgorithm::NestedLoop,
        JoinAlgorithm::Hash { build: Side::Right },
    ] {
        let mut seen = Vec::new();
        let mut ends = Vec::new();
        let mut after: Option<Vec<Value>> = None;
        for guard in 0..50 {
            assert!(guard < 49, "{algorithm:?} did not terminate");
            let join = paged_join(after.clone(), 2).using(algorithm);
            let got = page(&store, &join, 2).await.unwrap();
            seen.extend(got.rows);
            ends.push(got.cursor.clone());
            let Some(next) = got.cursor else { break };
            after = Some(next);
        }
        seen.sort_unstable();
        per_algorithm.push(seen);
        cursors.push(ends);
    }
    assert_eq!(
        per_algorithm[0], per_algorithm[1],
        "the nested loop and the hash join disagreed about what a page contains"
    );
    assert_eq!(
        cursors[0], cursors[1],
        "the two algorithms disagreed about where the pages end, which would make \
         a cursor taken from one unusable against the other"
    );
}

/// A forced nested loop pages, and its boundary is the left row it last read.
///
/// The direct version of the mutation above: without the loop's own tracking
/// there is no boundary at all, so the first page has no cursor and the walk
/// ends after one page with most of the rows missing.
#[tokio::test]
async fn a_forced_nested_loop_reports_its_page_boundary() {
    let store = seeded().await;
    let join = paged_join(None, 3).using(JoinAlgorithm::NestedLoop);
    let got = page(&store, &join, 3).await.unwrap();
    assert_eq!(got.left_rows, 3, "the loop read three left rows");
    assert_eq!(
        got.cursor,
        Some(vec![Value::U64(2)]),
        "and the page ends on the third of them"
    );
}

// --- refusals -------------------------------------------------------------

fn refusal(error: &slate_kernel::KernelError) -> String {
    match error {
        slate_kernel::KernelError::InvalidCursor { reason, .. } => reason.clone(),
        other => panic!("expected InvalidCursor, got {other:?}"),
    }
}

#[tokio::test]
async fn a_right_or_full_outer_join_refuses_a_cursor() {
    let store = seeded().await;
    for build in [
        Join::equating(Ordinal(0), Ordinal(1)).right_outer(),
        Join::equating(Ordinal(0), Ordinal(1)).full_outer(),
    ] {
        let txn = store.begin().await.unwrap();
        let error = txn
            .join(&root(), &authors(), &books(), &build.limit(2).paging())
            .await
            .expect_err("a right-preserving join cannot be paged by the left key");
        assert!(
            refusal(&error).contains("belong to no page"),
            "unhelpful refusal: {}",
            refusal(&error)
        );
    }
}

#[tokio::test]
async fn forcing_a_hash_build_on_the_left_refuses_a_cursor() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let error = txn
        .join(
            &root(),
            &authors(),
            &books(),
            &Join::equating(Ordinal(0), Ordinal(1))
                .using(JoinAlgorithm::Hash { build: Side::Left })
                .limit(2)
                .paging(),
        )
        .await
        .expect_err("building the left side consumes the side the page is defined by");
    assert!(refusal(&error).contains("build a hash table on the left"));
}

#[tokio::test]
async fn an_offset_and_a_cursor_together_are_refused() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let error = txn
        .join(
            &root(),
            &authors(),
            &books(),
            &Join::equating(Ordinal(0), Ordinal(1))
                .limit(2)
                .offset(3)
                .paging(),
        )
        .await
        .expect_err("counting and keying are two ways to say where a page starts");
    assert!(refusal(&error).contains("Drop the offset"));
}

#[tokio::test]
async fn a_page_with_no_size_is_refused() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let error = txn
        .join(
            &root(),
            &authors(),
            &books(),
            &Join::equating(Ordinal(0), Ordinal(1)).paging(),
        )
        .await
        .expect_err("a page needs a size");
    assert!(refusal(&error).contains("a page needs a size"));
}

/// The first page raises the refusals the second one would.
///
/// Without `paging` on the first request the caller learns on page two that
/// page one was never resumable, which is the worst moment to find out.
#[tokio::test]
async fn the_first_page_of_an_unpageable_join_fails_rather_than_the_second() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    // No `after`: this is page one.
    let error = txn
        .join(
            &root(),
            &authors(),
            &books(),
            &Join::equating(Ordinal(0), Ordinal(1))
                .right_outer()
                .limit(2)
                .paging(),
        )
        .await
        .expect_err("page one of an unpageable join must fail");
    assert!(refusal(&error).contains("belong to no page"));
}

/// The left input's own refusals arrive in the kernel's own words.
#[tokio::test]
async fn a_left_side_sorted_off_the_key_refuses_through_the_left_query() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let error = txn
        .join(
            &root(),
            &authors(),
            &books(),
            &Join::equating(Ordinal(0), Ordinal(1))
                .left(Query::all().sort_by([SortKey::desc(Ordinal(1))]))
                .limit(2)
                .paging(),
        )
        .await
        .expect_err("a sort the primary key does not give cannot be resumed");
    assert!(
        refusal(&error).contains("re-ordered after they are read"),
        "the left's own refusal should come through verbatim: {}",
        refusal(&error)
    );
}

// --- chains ---------------------------------------------------------------

/// The ordinal space a three-table chain accumulates into.
fn chain_schema() -> slate_kernel::JoinSchema {
    let tables = [authors(), books(), reviews()];
    slate_kernel::JoinSchema::over(tables.iter())
}

/// books joined onto authors, by `authors.id = books.author_id`.
fn books_step() -> JoinStep {
    JoinStep::equating(chain_schema().at(0, Ordinal(0)), Ordinal(1)).query(Query::all())
}

/// reviews joined onto books, by `books.id = reviews.book_id`.
fn reviews_step() -> JoinStep {
    JoinStep::equating(chain_schema().at(1, Ordinal(0)), Ordinal(1)).query(Query::all())
}

fn paged_chain(after: Option<Vec<Value>>, size: usize) -> Chain {
    let chain = Chain::from(Query::all())
        .join(books_step())
        .join(reviews_step())
        .limit(size);
    match after {
        Some(key) => chain.after(key),
        None => chain.paging(),
    }
}

async fn chain_page(
    store: &RecordStore<MemoryStore>,
    chain: &Chain,
    size: usize,
) -> Result<(Vec<Vec<u64>>, Option<Vec<Value>>), slate_kernel::KernelError> {
    let txn = store.begin().await.unwrap();
    let tables = [authors(), books(), reviews()];
    let refs: Vec<&TableDef> = tables.iter().collect();
    let cursor = txn.chain(&root(), &refs, chain).await?;
    let driving = cursor.driving_rows();
    let end = cursor.page_end().map(<[Value]>::to_vec);
    let rows: Vec<Vec<u64>> = cursor
        .collect()
        .await?
        .into_iter()
        .map(|row| {
            row.rows()
                .iter()
                .map(|r| {
                    u64_of(
                        &r.as_ref()
                            .expect("inner chain: every table present")
                            .values()[0],
                    )
                })
                .collect()
        })
        .collect();
    Ok((rows, (driving >= size).then_some(end).flatten()))
}

/// Paging a three-table chain visits every row exactly once.
#[tokio::test]
async fn paging_a_chain_visits_every_row_exactly_once() {
    let store = seeded().await;
    let tables = [authors(), books(), reviews()];
    let refs: Vec<&TableDef> = tables.iter().collect();
    let txn = store.begin().await.unwrap();
    let whole: Vec<Vec<u64>> = txn
        .chain(
            &root(),
            &refs,
            &Chain::from(Query::all())
                .join(books_step())
                .join(reviews_step()),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            row.rows()
                .iter()
                .map(|r| u64_of(&r.as_ref().unwrap().values()[0]))
                .collect()
        })
        .collect();

    for size in 1..=4 {
        let mut seen = Vec::new();
        let mut after: Option<Vec<Value>> = None;
        for guard in 0..100 {
            assert!(guard < 99, "the walk did not terminate");
            let (rows, next) = chain_page(&store, &paged_chain(after.clone(), size), size)
                .await
                .unwrap();
            seen.extend(rows);
            let Some(next) = next else { break };
            after = Some(next);
        }
        let mut expected = whole.clone();
        seen.sort();
        expected.sort();
        assert_eq!(
            seen, expected,
            "pages of {size} did not reproduce the chain"
        );
    }
}

/// A chain's page holds exactly `limit` rows of the *first* table.
///
/// `paging_a_chain_visits_every_row_exactly_once` compares the union over every
/// page against the whole chain, and a mutation that never applied the page
/// size to the first step survived it: page one then returns the whole chain,
/// its cursor is the last first-table row, page two is empty, and the union is
/// still right. A union says nothing about where the pages divide, so this
/// counts the first table's rows in one page.
#[tokio::test]
async fn a_chain_page_holds_exactly_that_many_first_table_rows() {
    let store = seeded().await;
    let (rows, cursor) = chain_page(&store, &paged_chain(None, 2), 2).await.unwrap();
    let firsts: BTreeSet<u64> = rows.iter().map(|row| row[0]).collect();
    assert_eq!(
        firsts.len(),
        2,
        "a page of two is two authors, and this one held {firsts:?}"
    );
    assert!(
        rows.len() > firsts.len(),
        "the fixture has fan-out, so a page is more rows than authors: {} rows",
        rows.len()
    );
    assert!(cursor.is_some(), "a full page gets a cursor");
}

#[tokio::test]
async fn a_right_outer_chain_step_refuses_a_cursor() {
    let store = seeded().await;
    let tables = [authors(), books(), reviews()];
    let refs: Vec<&TableDef> = tables.iter().collect();
    let txn = store.begin().await.unwrap();
    let error = txn
        .chain(
            &root(),
            &refs,
            &Chain::from(Query::all())
                .join(books_step())
                .join(reviews_step().right_outer())
                .limit(2)
                .paging(),
        )
        .await
        .expect_err("a right-preserving step cannot be paged by the first table's key");
    assert!(
        refusal(&error).contains("step 2"),
        "the refusal should name which step: {}",
        refusal(&error)
    );
}

// --- the three measurements the design rests on ---------------------------

/// **Measurement 1.** Only two of the three algorithms keep the left side's
/// key order, and no algorithm does for a right-preserving join.
///
/// This is why the cursor is *not* a position in the output. Recorded as a
/// test so that a change making the output order accidental-and-different is
/// visible rather than silent.
#[tokio::test]
async fn which_algorithms_keep_the_left_key_order() {
    let store = seeded().await;
    let mut ordered: BTreeMap<(String, String), bool> = BTreeMap::new();
    for kind in [
        JoinType::Inner,
        JoinType::Left,
        JoinType::Right,
        JoinType::Full,
    ] {
        for algorithm in [
            JoinAlgorithm::NestedLoop,
            JoinAlgorithm::Hash { build: Side::Left },
            JoinAlgorithm::Hash { build: Side::Right },
        ] {
            if matches!(algorithm, JoinAlgorithm::NestedLoop) && kind.preserves(Side::Right) {
                continue; // refused, and `join_oracle.rs` pins that.
            }
            let base = Join::equating(Ordinal(0), Ordinal(1));
            let join = match kind {
                JoinType::Inner => base,
                JoinType::Left => base.left_outer(),
                JoinType::Right => base.right_outer(),
                JoinType::Full => base.full_outer(),
            }
            .using(algorithm);
            let txn = store.begin().await.unwrap();
            let rows = txn
                .join(&root(), &authors(), &books(), &join)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            let lefts: Vec<Option<u64>> = rows
                .iter()
                .map(|r| r.left.as_ref().map(|l| u64_of(&l.values()[0])))
                .collect();
            ordered.insert(
                (format!("{kind:?}"), format!("{algorithm:?}")),
                lefts.windows(2).all(|w| w[0] <= w[1]),
            );
        }
    }
    let by =
        |kind: JoinType, algorithm: &str| ordered[&(format!("{kind:?}"), algorithm.to_owned())];
    assert!(by(JoinType::Inner, "NestedLoop"));
    assert!(by(JoinType::Inner, "Hash { build: Right }"));
    assert!(
        !by(JoinType::Inner, "Hash { build: Left }"),
        "building the left side puts its rows through hash buckets, which is the whole reason \
         the cursor is not a position in the output"
    );
    assert!(by(JoinType::Left, "NestedLoop"));
    assert!(by(JoinType::Left, "Hash { build: Right }"));
    assert!(!by(JoinType::Left, "Hash { build: Left }"));
    for kind in [JoinType::Right, JoinType::Full] {
        for algorithm in ["Hash { build: Left }", "Hash { build: Right }"] {
            assert!(
                !by(kind, algorithm),
                "{kind:?}/{algorithm} looked left-ordered; its preserved right rows have no left \
                 row at all, so this fixture is not exercising them"
            );
        }
    }
}

/// **Measurement 2.** A side's `limit` and `offset` are honoured, and the
/// three algorithms agree exactly on which rows come back.
///
/// Both `Join`'s doc comment and `JoinInput.query`'s field comment said these
/// were *ignored by the kernel*. They were not, and had not been:
/// `SecuredReads::execute` ends in `with_window`. Paging is built on this
/// agreement, so it is pinned here.
#[tokio::test]
async fn a_side_window_is_honoured_and_every_algorithm_agrees() {
    let store = seeded().await;
    for (name, left) in [
        ("limit", Query::all().limit(3)),
        ("offset", Query::all().offset(5)),
    ] {
        let mut per_algorithm: Vec<BTreeSet<(u64, u64)>> = Vec::new();
        for algorithm in [
            JoinAlgorithm::NestedLoop,
            JoinAlgorithm::Hash { build: Side::Left },
            JoinAlgorithm::Hash { build: Side::Right },
        ] {
            let txn = store.begin().await.unwrap();
            let rows = txn
                .join(
                    &root(),
                    &authors(),
                    &books(),
                    &Join::equating(Ordinal(0), Ordinal(1))
                        .left(left.clone())
                        .using(algorithm),
                )
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            per_algorithm.push(
                rows.iter()
                    .map(|r| {
                        (
                            u64_of(&r.left.as_ref().unwrap().values()[0]),
                            u64_of(&r.right.as_ref().unwrap().values()[0]),
                        )
                    })
                    .collect(),
            );
        }
        assert!(
            per_algorithm.windows(2).all(|w| w[0] == w[1]),
            "the three algorithms disagreed about a side's {name}: {per_algorithm:?}"
        );
        let lefts: BTreeSet<u64> = per_algorithm[0].iter().map(|(l, _)| *l).collect();
        match name {
            "limit" => assert_eq!(lefts, BTreeSet::from([0, 1, 2])),
            _ => assert_eq!(lefts, BTreeSet::from([5, 6, 7])),
        }
    }
}

/// **Measurement 3.** A side's `sort` is algorithm-dependent — honoured where
/// the left streams, silently dropped where it is hashed.
///
/// Worse than either being ignored or being honoured, and the reason the wire
/// refuses it on an input while allowing the window above.
#[tokio::test]
async fn a_side_sort_is_honoured_by_some_algorithms_and_dropped_by_others() {
    let store = seeded().await;
    let mut first: BTreeMap<String, u64> = BTreeMap::new();
    for algorithm in [
        JoinAlgorithm::NestedLoop,
        JoinAlgorithm::Hash { build: Side::Left },
        JoinAlgorithm::Hash { build: Side::Right },
    ] {
        let txn = store.begin().await.unwrap();
        let rows = txn
            .join(
                &root(),
                &authors(),
                &books(),
                &Join::equating(Ordinal(0), Ordinal(1))
                    .left(Query::all().sort_by([SortKey::desc(Ordinal(0))]))
                    .using(algorithm),
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        first.insert(
            format!("{algorithm:?}"),
            u64_of(&rows[0].left.as_ref().unwrap().values()[0]),
        );
    }
    assert_eq!(first["NestedLoop"], AUTHOR_ROWS - 1, "the loop honours it");
    assert_eq!(
        first["Hash { build: Right }"],
        AUTHOR_ROWS - 1,
        "the probe side streams, so it honours it"
    );
    assert_eq!(
        first["Hash { build: Left }"], 0,
        "the built side loses the order, silently — which is why the wire refuses a side's sort"
    );
}
