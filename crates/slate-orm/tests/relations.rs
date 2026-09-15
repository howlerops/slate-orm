//! Relationships, and the claim that loading them costs one read.
//!
//! The claim this file keeps honest: **`load_related` issues one query for the
//! whole set of parents, not one per parent.** That is the entire reason the
//! function exists, and it is a claim about *how many reads happen*, which no
//! assertion about the returned rows can make. So the store is wrapped in a
//! counter and the count is asserted.
//!
//! The relationships are written by hand here rather than derived. That is
//! deliberate for now: the trait is the contract and the attribute is sugar
//! over it, so the contract gets the tests.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use async_trait::async_trait;
use bytes::Bytes;
use slate_kernel::store::{
    KeyRange, KvIterator, KvSnapshot, KvStore, KvTransaction, ScanOrder as StoreOrder,
};
use slate_orm::{
    Action, Catalog, Expr, Grant, Ordinal, Record, RecordStore, Records, SecurityCatalog,
    SecurityContext, TableId, Value, load_one_related, load_related,
    memory::MemoryStore,
    relation::{Related, related_filter},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as Atomics};

/// A `KvStore` that counts the reads underneath it.
///
/// The claim `load_related` exists to make is about *how many reads happen*,
/// and no assertion about the rows that come back can see that: a loop issuing
/// one query per author returns exactly the same rows as one `IN`. So the
/// claim is measured where it is made, at the store.
///
/// Scans rather than gets, because `find_records` opens one cursor per call and
/// that is the thing whose count must not grow with the number of parents. The
/// count is shared through an `Arc` so the test can read it while the store
/// still owns its half.
struct Counting<S> {
    inner: S,
    scans: Arc<AtomicUsize>,
}

struct CountingTxn<'a> {
    inner: Box<dyn KvTransaction + Send + 'a>,
    scans: Arc<AtomicUsize>,
}

#[async_trait]
impl<S: KvStore> KvStore for Counting<S> {
    async fn begin(&self) -> slate_kernel::Result<Box<dyn KvTransaction + Send + '_>> {
        Ok(Box::new(CountingTxn {
            inner: self.inner.begin().await?,
            scans: Arc::clone(&self.scans),
        }))
    }
}

#[async_trait]
impl KvSnapshot for CountingTxn<'_> {
    async fn get(&self, key: &[u8]) -> slate_kernel::Result<Option<Bytes>> {
        self.inner.get(key).await
    }

    async fn scan(
        &self,
        range: KeyRange,
        order: StoreOrder,
    ) -> slate_kernel::Result<Box<dyn KvIterator + Send + '_>> {
        self.scans.fetch_add(1, Atomics::SeqCst);
        self.inner.scan(range, order).await
    }

    fn is_point_in_time(&self) -> bool {
        self.inner.is_point_in_time()
    }
}

#[async_trait]
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

const AUTHORS: TableId = TableId(1);
const BOOKS: TableId = TableId(2);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "authors", id = 1)]
struct Author {
    #[record(pk)]
    id: u64,
    name: String,
}

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "books", id = 2)]
struct Book {
    #[record(pk)]
    id: u64,
    #[record(index(name = "by_author", id = 10))]
    author_id: u64,
    title: String,
}

/// An author's books: their `id` matches a book's `author_id`.
impl Related<Book> for Author {
    fn local() -> Ordinal {
        Ordinal(0)
    }
    fn foreign() -> Ordinal {
        Ordinal(1)
    }
}

/// And the other way: a book's `author_id` matches an author's `id`. Two impls
/// rather than one bidirectional declaration, because the ordinals swap roles.
impl Related<Author> for Book {
    fn local() -> Ordinal {
        Ordinal(1)
    }
    fn foreign() -> Ordinal {
        Ordinal(0)
    }
}

fn store() -> (RecordStore<Counting<MemoryStore>>, Arc<AtomicUsize>) {
    let catalog =
        Catalog::from_tables([Author::table().clone(), Book::table().clone()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("member", AUTHORS, Action::EVERYTHING))
        .grant(Grant::new("member", BOOKS, Action::EVERYTHING));
    let scans = Arc::new(AtomicUsize::new(0));
    let store = RecordStore::new(
        Counting {
            inner: MemoryStore::new(),
            scans: Arc::clone(&scans),
        },
        catalog,
        security,
    );
    (store, scans)
}

fn context() -> SecurityContext {
    SecurityContext::new(slate_orm::Principal::new(Value::U64(1)).with_role("member"))
}

/// Three authors, and books spread unevenly: one with several, one with one,
/// one with none. The uneven shape is the point — an implementation that
/// returned the same list for every parent, or dropped the empty one, passes a
/// balanced fixture.
async fn seeded() -> (
    RecordStore<Counting<MemoryStore>>,
    SecurityContext,
    Arc<AtomicUsize>,
) {
    let (store, scans) = store();
    let ctx = context();
    let txn = store.begin().await.unwrap();
    for (id, name) in [(1u64, "Le Guin"), (2, "Calvino"), (3, "Borges")] {
        txn.insert_record(
            &ctx,
            &Author {
                id,
                name: name.to_owned(),
            },
        )
        .await
        .unwrap();
    }
    for (id, author_id, title) in [
        (10u64, 1u64, "A Wizard of Earthsea"),
        (11, 1, "The Dispossessed"),
        (12, 1, "The Left Hand of Darkness"),
        (13, 2, "Invisible Cities"),
    ] {
        txn.insert_record(
            &ctx,
            &Book {
                id,
                author_id,
                title: title.to_owned(),
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    (store, ctx, scans)
}

#[tokio::test]
async fn a_has_many_loads_every_parents_children_in_parent_order() {
    let (store, ctx, _scans) = seeded().await;
    let txn = store.begin().await.unwrap();
    let authors: Vec<Author> = txn
        .find_records(&ctx, slate_orm::Expr::True, slate_orm::ScanOrder::Ascending)
        .await
        .unwrap();
    assert_eq!(authors.len(), 3);

    let books = load_related::<_, Author, Book>(&txn, &ctx, &authors)
        .await
        .unwrap();

    // One entry per parent, in parent order, including the empty one. An
    // implementation that returned only the parents with children would come
    // back with two entries and silently misalign every index after the first
    // gap -- which is the bug this shape exists to make impossible.
    assert_eq!(books.len(), authors.len());
    for (author, mine) in authors.iter().zip(&books) {
        assert!(
            mine.iter().all(|b| b.author_id == author.id),
            "{}'s list has someone else's book: {mine:?}",
            author.name
        );
    }
    let counts: Vec<usize> = books.iter().map(Vec::len).collect();
    let by_name: Vec<(&str, usize)> = authors
        .iter()
        .map(|a| a.name.as_str())
        .zip(counts.iter().copied())
        .collect();
    assert!(by_name.contains(&("Le Guin", 3)), "{by_name:?}");
    assert!(by_name.contains(&("Calvino", 1)), "{by_name:?}");
    assert!(by_name.contains(&("Borges", 0)), "{by_name:?}");
}

#[tokio::test]
async fn a_belongs_to_gives_one_parent_and_shares_it() {
    let (store, ctx, _scans) = seeded().await;
    let txn = store.begin().await.unwrap();
    let books: Vec<Book> = txn
        .find_records(&ctx, slate_orm::Expr::True, slate_orm::ScanOrder::Ascending)
        .await
        .unwrap();
    assert_eq!(books.len(), 4);

    let authors = load_one_related::<_, Book, Author>(&txn, &ctx, &books)
        .await
        .unwrap();
    assert_eq!(authors.len(), books.len());
    for (book, author) in books.iter().zip(&authors) {
        let author = author.as_ref().expect("every book has an author");
        assert_eq!(author.id, book.author_id, "{book:?} got {author:?}");
    }
    // Three of the four books share one author, and all three got it. A loader
    // that moved rows out of the group rather than copying them would give the
    // first book its author and the rest `None`.
    let le_guin = authors
        .iter()
        .filter(|a| a.as_ref().is_some_and(|a| a.id == 1))
        .count();
    assert_eq!(le_guin, 3, "{authors:?}");
}

#[tokio::test]
async fn loading_no_parents_reads_nothing_and_returns_nothing() {
    let (store, ctx, scans) = seeded().await;
    let txn = store.begin().await.unwrap();
    let none: Vec<Author> = Vec::new();

    // The read count, not just the result. Deleting the early return leaves the
    // result correct — an `IN` over no values matches nothing, so the empty
    // vector comes back either way — and issues a read to discover it. This
    // test is named for reading nothing, so it has to measure that; asserting
    // only the result was a mutation survivor, and this line is why it is not.
    let before = scans.load(Atomics::SeqCst);
    let books = load_related::<_, Author, Book>(&txn, &ctx, &none)
        .await
        .unwrap();
    assert_eq!(
        scans.load(Atomics::SeqCst) - before,
        0,
        "loading the relations of no parents issued a read"
    );
    assert!(books.is_empty());
}

#[tokio::test]
async fn the_filter_carries_one_value_per_distinct_key_not_one_per_parent() {
    // What deduplication does is invisible everywhere else: it changes neither
    // the rows returned nor the number of reads, only the size of the request.
    // So it is asserted on the request. Three of these four books share an
    // author, and the filter must name that author once.
    let books: Vec<Book> = [(10u64, 1u64), (11, 1), (12, 1), (13, 2)]
        .into_iter()
        .map(|(id, author_id)| Book {
            id,
            author_id,
            title: String::new(),
        })
        .collect();

    match related_filter::<Book, Author>(&books).unwrap() {
        Expr::In { column, values } => {
            // The author side's own column, not the book's.
            assert_eq!(column, Ordinal(0));
            assert_eq!(values, vec![Value::U64(1), Value::U64(2)], "{values:?}");
        }
        other => panic!("expected an IN over the parents' keys, got {other:?}"),
    }
}

#[tokio::test]
async fn the_filter_of_no_parents_matches_nothing() {
    // Documented rather than incidental: the empty filter is a correct answer
    // (no parents have no children), and `load_related` still declines to issue
    // it. Both halves are claims, so both are tested — the second one above.
    let none: Vec<Author> = Vec::new();
    match related_filter::<Author, Book>(&none).unwrap() {
        Expr::In { column, values } => {
            assert_eq!(column, Ordinal(1));
            assert!(values.is_empty(), "{values:?}");
        }
        other => panic!("expected an empty IN, got {other:?}"),
    }
}

#[tokio::test]
async fn a_parent_whose_children_were_deleted_gets_an_empty_list() {
    let (store, ctx, _scans) = seeded().await;
    let txn = store.begin().await.unwrap();
    txn.delete_record::<Book>(&ctx, &[Value::U64(13)])
        .await
        .unwrap();
    txn.commit().await.unwrap();
    let txn = store.begin().await.unwrap();
    let calvino: Vec<Author> = txn
        .find_records(
            &ctx,
            slate_orm::Expr::compare(Ordinal(0), slate_orm::CmpOp::Eq, Value::U64(2)),
            slate_orm::ScanOrder::Ascending,
        )
        .await
        .unwrap();
    assert_eq!(calvino.len(), 1);
    let books = load_related::<_, Author, Book>(&txn, &ctx, &calvino)
        .await
        .unwrap();
    assert_eq!(books, vec![Vec::<Book>::new()]);
}

#[tokio::test]
async fn loading_relations_costs_one_read_however_many_parents() {
    // The measurement the whole design rests on. A lazy accessor would issue
    // one read per parent; this issues one for the set, and the difference is
    // invisible in the rows that come back — both return the same books.
    //
    // Counted in *scans*, because `find_records` opens one cursor per call and
    // that is the number that must not grow with the parents.
    let (store, ctx, scans) = seeded().await;
    let txn = store.begin().await.unwrap();
    let authors: Vec<Author> = txn
        .find_records(&ctx, slate_orm::Expr::True, slate_orm::ScanOrder::Ascending)
        .await
        .unwrap();
    assert_eq!(authors.len(), 3);

    let before = scans.load(Atomics::SeqCst);
    let books = load_related::<_, Author, Book>(&txn, &ctx, &authors)
        .await
        .unwrap();
    let cost = scans.load(Atomics::SeqCst) - before;

    // Every author's books arrived.
    assert_eq!(books.iter().map(Vec::len).sum::<usize>(), 4);
    // And it cost one read, not one per author. The bound is `<= 1` rather than
    // `== 1` so that a future planner choosing a point get over a scan — which
    // is a *better* plan and opens no cursor — does not fail a test about
    // costing less.
    assert!(
        cost <= 1,
        "loading {} authors' books cost {cost} scans; a per-parent loop would cost {}",
        authors.len(),
        authors.len()
    );

    // The comparison the claim is against, measured rather than asserted: the
    // loop this function exists to replace, over the same parents.
    let before = scans.load(Atomics::SeqCst);
    for author in &authors {
        let _: Vec<Book> = txn
            .find_records(
                &ctx,
                slate_orm::Expr::compare(Ordinal(1), slate_orm::CmpOp::Eq, Value::U64(author.id)),
                slate_orm::ScanOrder::Ascending,
            )
            .await
            .unwrap();
    }
    let looped = scans.load(Atomics::SeqCst) - before;
    // Printed as well as asserted, because the docs quote the pair and a number
    // in a doc that no run emits is a number somebody typed from memory.
    println!(
        "{} parents: batched {cost} scan(s), per-parent loop {looped}",
        authors.len()
    );
    assert!(
        looped > cost,
        "the per-parent loop cost {looped} scans and the batched load cost {cost}; \
         if those are equal the counter is not measuring what this test claims"
    );
}
