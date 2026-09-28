//! How many reads a relation load makes, counted rather than argued.
//!
//! # The claim this file settles
//!
//! Three entries say a batched relation load is "two reads rather than N", and
//! all three say it structurally:
//!
//! > "Nothing measures it. *Two reads rather than N* is a claim about round
//! > trips that the code's shape makes obvious and no benchmark here confirms —
//! > the `IN`-deduplication assertion in
//! > `parents_sharing_a_tag_share_one_read_of_it` counts the values in the
//! > filter, which is the request's size and not its latency."
//! > — `ledger/2026-09-16-many-to-many-is-a-composition-not-a-third-relationship.md`
//!
//! That assertion is real and measures the wrong thing twice over: it counts
//! values in an `Expr::In` the test builds *itself* by calling
//! `related_filter`, so it never observes what the loader does, and a loader
//! that built the right filter and then read once per parent anyway would pass
//! it.
//!
//! What is counted here is the loader's calls into [`Records`], which is the
//! layer where a read is a read. `load_related` makes one; `load_related_through`
//! and `load_nested` make two; and — the part that makes it a measurement of
//! *N* rather than a snapshot — the count does not move between 2 parents and
//! 24. An implementation that fell back to a read per parent would read 24
//! times and fail on the number, not on a filter it never issued.
//!
//! # Why a wrapper rather than an instrumented store
//!
//! `MemoryStore` could hold an `AtomicUsize` and the whole crate would get read
//! counting for nothing. Rejected: the count would then be of *kernel* scans,
//! which is a different number — one `find_records` over an `IN` of 24 keys may
//! be one index range or 24 point gets depending on what the planner chooses,
//! and that choice is a separate question with its own tests. The claim under
//! test is about how many times the *relation loader* goes to the store, and
//! counting at `Records` is counting exactly that. It also keeps the instrument
//! in the test that needs it rather than in a type every other test shares.
//!
//! The cost is 22 delegating methods, which is what a trait with no default
//! bodies costs to wrap. They are mechanical and they are checked by the
//! compiler; the two that matter carry the counter.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use async_trait::async_trait;
use slate_orm::{
    Action, Aggregate, Catalog, Chain, Explanation, Expr, Grant, Group, Join, JoinExplanation,
    Ordinal, Page, Query, Record, RecordStore, Records, Result, Scalar, ScanOrder, SecurityCatalog,
    SecurityContext, TableId, TableStats, Value, load_nested, load_related, load_related_through,
    load_through, memory::MemoryStore,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "articles", id = 1)]
#[record(has_many(ArticleTag, foreign = article_id))]
#[record(has_many(Tag, through = ArticleTag))]
struct Article {
    #[record(pk)]
    id: u64,
    title: String,
}

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "tags", id = 2)]
struct Tag {
    #[record(pk)]
    id: u64,
    label: String,
}

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "article_tags", id = 3)]
#[record(belongs_to(Tag, local = tag_id))]
struct ArticleTag {
    #[record(pk)]
    id: u64,
    #[record(index(name = "by_article", id = 10))]
    article_id: u64,
    tag_id: u64,
}

/// A [`Records`] that forwards everything and counts the reads.
///
/// Only the four read methods a relation loader could plausibly reach increment
/// the counter. The rest forward untouched — not because they cannot be
/// counted, but because a counter that moves on a write would make a test
/// about reads pass or fail on the seeding, and the seeding is not the subject.
struct Counting<'a, S: ?Sized> {
    inner: &'a S,
    reads: AtomicUsize,
}

impl<'a, S: Records + Sync + ?Sized> Counting<'a, S> {
    fn new(inner: &'a S) -> Self {
        Self {
            inner,
            reads: AtomicUsize::new(0),
        }
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    fn counted(&self) {
        self.reads.fetch_add(1, Ordering::Relaxed);
    }
}

#[async_trait]
impl<S: Records + Sync + ?Sized> Records for Counting<'_, S> {
    // --- the four that count ------------------------------------------------
    async fn find_records<R: Record>(
        &self,
        context: &SecurityContext,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<Vec<R>> {
        self.counted();
        self.inner.find_records::<R>(context, filter, order).await
    }

    async fn query_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Vec<R>> {
        self.counted();
        self.inner.query_records::<R>(context, query).await
    }

    async fn get_record<R: Record>(
        &self,
        context: &SecurityContext,
        primary_key: &[Value],
    ) -> Result<Option<R>> {
        self.counted();
        self.inner.get_record::<R>(context, primary_key).await
    }

    async fn page_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Page<R>> {
        self.counted();
        self.inner.page_records::<R>(context, query).await
    }

    // --- and the rest, forwarded ---------------------------------------------
    async fn insert_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()> {
        self.inner.insert_record(context, record).await
    }

    async fn update_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()> {
        self.inner.update_record(context, record).await
    }

    async fn replace_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        previous: &R,
        next: &R,
    ) -> Result<()> {
        self.inner.replace_record(context, previous, next).await
    }

    async fn upsert_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()> {
        self.inner.upsert_record(context, record).await
    }

    async fn insert_records<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        records: &[R],
    ) -> Result<()> {
        self.inner.insert_records(context, records).await
    }

    async fn upsert_records<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        records: &[R],
    ) -> Result<()> {
        self.inner.upsert_records(context, records).await
    }

    async fn delete_record<R: Record>(
        &self,
        context: &SecurityContext,
        primary_key: &[Value],
    ) -> Result<bool> {
        self.inner.delete_record::<R>(context, primary_key).await
    }

    async fn remove_record<R: Record + Sync>(
        &self,
        context: &SecurityContext,
        record: &R,
    ) -> Result<()> {
        self.inner.remove_record(context, record).await
    }

    async fn delete_records_where<R: Record>(
        &self,
        context: &SecurityContext,
        predicate: Expr,
    ) -> Result<usize> {
        self.inner
            .delete_records_where::<R>(context, predicate)
            .await
    }

    async fn update_records_where<R: Record>(
        &self,
        context: &SecurityContext,
        predicate: Expr,
        assignments: &[(Ordinal, Scalar)],
    ) -> Result<usize> {
        self.inner
            .update_records_where::<R>(context, predicate, assignments)
            .await
    }

    async fn count_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<u64> {
        self.inner.count_records::<R>(context, query).await
    }

    async fn join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<Vec<(L, Option<R>)>> {
        self.inner.join_records::<L, R>(context, join).await
    }

    async fn outer_join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<Vec<(Option<L>, Option<R>)>> {
        self.inner.outer_join_records::<L, R>(context, join).await
    }

    fn explain_join_records<L: Record, R: Record>(
        &self,
        context: &SecurityContext,
        join: &Join,
    ) -> Result<JoinExplanation> {
        self.inner.explain_join_records::<L, R>(context, join)
    }

    async fn chain_records<A: Record, B: Record, C: Record>(
        &self,
        context: &SecurityContext,
        chain: &Chain,
    ) -> Result<Vec<(Option<A>, Option<B>, Option<C>)>> {
        self.inner.chain_records::<A, B, C>(context, chain).await
    }

    async fn aggregate_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>> {
        self.inner
            .aggregate_records::<R>(context, query, aggregates)
            .await
    }

    async fn group_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>> {
        self.inner
            .group_records::<R>(context, query, group, aggregates)
            .await
    }

    fn explain_records<R: Record>(
        &self,
        context: &SecurityContext,
        query: &Query,
    ) -> Result<Explanation> {
        self.inner.explain_records::<R>(context, query)
    }

    async fn analyze_records<R: Record>(&self, context: &SecurityContext) -> Result<TableStats> {
        self.inner.analyze_records::<R>(context).await
    }
}

fn context() -> SecurityContext {
    SecurityContext::new(slate_orm::Principal::new(Value::U64(1)).with_role("member"))
}

/// `articles` parents, each carrying two tags, with one tag shared by all.
///
/// Parameterised on the parent count because that is the whole point: a claim
/// that something is independent of N cannot be tested at one N.
async fn seeded(articles: u64) -> (RecordStore<MemoryStore>, SecurityContext, Vec<Article>) {
    let catalog = Catalog::from_tables([
        Article::table().clone(),
        Tag::table().clone(),
        ArticleTag::table().clone(),
    ])
    .expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("member", TableId(1), Action::EVERYTHING))
        .grant(Grant::new("member", TableId(2), Action::EVERYTHING))
        .grant(Grant::new("member", TableId(3), Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let context = context();

    let txn = store.begin().await.unwrap();
    // Tag 0 is shared by every article; tag `id` is that article's own. So the
    // second read's `IN` carries `articles + 1` distinct keys however many
    // parents there are, and the loader still makes one call.
    txn.insert_record(
        &context,
        &Tag {
            id: 0,
            label: "shared".to_owned(),
        },
    )
    .await
    .unwrap();
    let mut parents = Vec::new();
    let mut join_id = 0u64;
    for id in 1..=articles {
        txn.insert_record(
            &context,
            &Article {
                id,
                title: format!("article {id}"),
            },
        )
        .await
        .unwrap();
        txn.insert_record(
            &context,
            &Tag {
                id,
                label: format!("tag {id}"),
            },
        )
        .await
        .unwrap();
        for tag_id in [0, id] {
            join_id += 1;
            txn.insert_record(
                &context,
                &ArticleTag {
                    id: join_id,
                    article_id: id,
                    tag_id,
                },
            )
            .await
            .unwrap();
        }
        parents.push(Article {
            id,
            title: format!("article {id}"),
        });
    }
    txn.commit().await.unwrap();
    (store, context, parents)
}

#[tokio::test]
async fn a_has_many_is_one_read_whatever_the_parent_count() {
    for parents in [2u64, 24] {
        let (store, context, articles) = seeded(parents).await;
        let txn = store.begin().await.unwrap();
        let counting = Counting::new(&txn);

        let children: Vec<Vec<ArticleTag>> =
            load_related::<_, Article, ArticleTag>(&counting, &context, &articles)
                .await
                .unwrap();

        assert_eq!(children.len(), parents as usize, "one entry per parent");
        assert_eq!(
            counting.reads(),
            1,
            "{parents} parents should still be one read"
        );
    }
}

#[tokio::test]
async fn a_many_to_many_is_two_reads_whatever_the_parent_count() {
    // The measurement the three entries called structural. 24 parents name 25
    // distinct tags between them, so a loader reading per parent — or per join
    // row, which is 48 — is visibly different from one reading twice.
    for parents in [2u64, 24] {
        let (store, context, articles) = seeded(parents).await;
        let txn = store.begin().await.unwrap();
        let counting = Counting::new(&txn);

        let tags: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&counting, &context, &articles)
            .await
            .unwrap();

        assert_eq!(tags.len(), parents as usize, "one entry per parent");
        assert!(
            tags.iter().all(|mine| mine.len() == 2),
            "each article has the shared tag and its own"
        );
        assert_eq!(
            counting.reads(),
            2,
            "{parents} parents, {} join rows: still two reads",
            parents * 2
        );
    }
}

#[tokio::test]
async fn keeping_the_join_rows_costs_no_extra_read() {
    // `load_related_through` is `load_through` with the middle kept, and the
    // entries claim the same two reads. Asserted separately because "it is the
    // same function" is exactly the kind of reasoning this file exists to stop
    // standing in for a count.
    let (store, context, articles) = seeded(24).await;
    let txn = store.begin().await.unwrap();
    let counting = Counting::new(&txn);

    let tags: Vec<Vec<Tag>> =
        load_related_through::<_, Article, ArticleTag, Tag>(&counting, &context, &articles)
            .await
            .unwrap();

    assert_eq!(tags.len(), 24);
    assert_eq!(counting.reads(), 2, "the middle is kept, not re-read");
}

#[tokio::test]
async fn nested_loading_is_two_reads_whatever_the_parent_count() {
    // The second of the two structural claims:
    // `ledger/2026-09-16-nested-loading-and-a-depth-limit-with-nothing-to-limit.md`
    // — "The claim *two reads* is structural rather than measured."
    for parents in [2u64, 24] {
        let (store, context, articles) = seeded(parents).await;
        let txn = store.begin().await.unwrap();
        let counting = Counting::new(&txn);

        let pairs: Vec<Vec<(ArticleTag, Vec<Tag>)>> =
            load_nested::<_, Article, ArticleTag, Tag>(&counting, &context, &articles)
                .await
                .unwrap();

        assert_eq!(pairs.len(), parents as usize, "one entry per parent");
        assert_eq!(
            counting.reads(),
            2,
            "{parents} parents: still two reads, not one per join row"
        );
    }
}

#[tokio::test]
async fn no_parents_is_no_read_at_all() {
    // The early return in `load_related`, which has a comment arguing for it —
    // "the alternative is an `IN ()`, which is a filter that matches nothing
    // and still pays for a read" — and no test that the read is not paid.
    let (store, context, _) = seeded(2).await;
    let txn = store.begin().await.unwrap();
    let counting = Counting::new(&txn);

    let none: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&counting, &context, &[])
        .await
        .unwrap();

    assert!(none.is_empty());
    assert_eq!(counting.reads(), 0, "no parents, no query");
}
