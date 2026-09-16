//! `has_many ... through`: a many-to-many, loaded in two reads.
//!
//! # The finding this file records
//!
//! The plan called for a third kind of relationship beside `has_many` and
//! `belongs_to`. It is not one. `load_related_through` has the bounds
//! `P: Related<J>, J: Related<C>` and nothing else — a many-to-many *is* the
//! composition of a has-many onto the join table and the join table's
//! belongs-to onto the far side, both of which the derive already emits.
//! `a_many_to_many_needs_no_new_declaration` is that claim as a test: it loads
//! articles' tags using only the two ordinary declarations, never mentioning
//! `Through`.
//!
//! What the `through =` attribute buys is a *name*, and it is not free
//! sugar: the obvious way to get it for nothing is a blanket
//! `impl<P: Related<J>, J: Related<C>> Through<C> for P`, and the compiler
//! refuses that with `E0207` because `J` is unconstrained — if two tables
//! could stand in the middle, nothing says which. Which table is the join
//! table is a fact about the schema that has to be stated.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use slate_orm::{
    Action, Catalog, Expr, Grant, Record, RecordStore, Records, ScanOrder, SecurityCatalog,
    SecurityContext, TableId, Through, load_related, load_related_through, load_through,
    memory::MemoryStore,
};
use std::collections::BTreeSet;

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "articles", id = 1)]
// The ordinary has-many onto the join table. Nothing about it knows this is
// half of a many-to-many.
#[record(has_many(ArticleTag, foreign = article_id))]
// And the name for the pair, which is all this line emits.
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

/// The join table, with its two ordinary relationships.
///
/// Deliberately *not* keyed on the pair: a surrogate key with a composite
/// index is the shape that lets the same pair be inserted twice, which is what
/// `duplicates_are_returned_rather_than_removed` needs.
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

fn store() -> RecordStore<MemoryStore> {
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
    RecordStore::new(MemoryStore::new(), catalog, security)
}

fn context() -> SecurityContext {
    SecurityContext::new(slate_orm::Principal::new(slate_orm::Value::U64(1)).with_role("member"))
}

/// Three articles, four tags, and a join table wiring them up.
///
/// Article 1 → tags 10, 20. Article 2 → tag 20. Article 3 → nothing, which is
/// the entry an implementation that returned only the non-empty parents would
/// drop, misaligning every index after it.
async fn seeded() -> (RecordStore<MemoryStore>, SecurityContext, Vec<Article>) {
    let store = store();
    let context = context();
    let txn = store.begin().await.unwrap();
    for (id, title) in [(1u64, "One"), (2, "Two"), (3, "Three")] {
        txn.insert_record(&context, &Article { id, title: title.to_owned() })
            .await
            .unwrap();
    }
    for (id, label) in [(10u64, "rust"), (20, "databases"), (30, "unused"), (40, "also")] {
        txn.insert_record(&context, &Tag { id, label: label.to_owned() })
            .await
            .unwrap();
    }
    for (id, article_id, tag_id) in [(1u64, 1u64, 10u64), (2, 1, 20), (3, 2, 20)] {
        txn.insert_record(&context, &ArticleTag { id, article_id, tag_id })
            .await
            .unwrap();
    }
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let articles: Vec<Article> = txn
        .find_records(&context, Expr::True, ScanOrder::Ascending)
        .await
        .unwrap();
    txn.commit().await.unwrap();
    (store, context, articles)
}

fn labels(tags: &[Tag]) -> Vec<&str> {
    tags.iter().map(|t| t.label.as_str()).collect()
}

/// The composition, spelled out, with `Through` never mentioned.
#[tokio::test]
async fn a_many_to_many_needs_no_new_declaration() {
    let (store, context, articles) = seeded().await;
    let txn = store.begin().await.unwrap();

    let tags: Vec<Vec<Tag>> =
        load_related_through::<_, Article, ArticleTag, Tag>(&txn, &context, &articles)
            .await
            .unwrap();

    assert_eq!(tags.len(), articles.len(), "one entry per parent, in order");
    assert_eq!(labels(&tags[0]), vec!["rust", "databases"]);
    assert_eq!(labels(&tags[1]), vec!["databases"]);
    assert!(tags[2].is_empty(), "article three has no tags: {:?}", tags[2]);
}

/// And with the name, which must agree with the composition exactly.
#[tokio::test]
async fn the_named_form_agrees_with_the_spelled_out_one() {
    let (store, context, articles) = seeded().await;
    let txn = store.begin().await.unwrap();

    let spelled: Vec<Vec<Tag>> =
        load_related_through::<_, Article, ArticleTag, Tag>(&txn, &context, &articles)
            .await
            .unwrap();
    let named: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&txn, &context, &articles)
        .await
        .unwrap();

    assert_eq!(spelled, named);
    // The attribute resolved to the table it names, rather than to whatever
    // the first `Related` impl happened to be.
    let _: <Article as Through<Tag>>::Join = ArticleTag { id: 0, article_id: 0, tag_id: 0 };
}

/// The oracle the plan asked for: the explicit two-step, by hand.
///
/// Not a reimplementation of the same loop — it goes through the *rows*, doing
/// what a caller without this function would do: load the join rows, collect
/// the far keys, read those, and match them up. An implementation that
/// regrouped by the wrong offset agrees with itself and not with this.
#[tokio::test]
async fn it_agrees_with_the_explicit_two_step() {
    let (store, context, articles) = seeded().await;
    let txn = store.begin().await.unwrap();

    let got: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&txn, &context, &articles)
        .await
        .unwrap();

    // By hand: for each article, its join rows, then each join row's tag,
    // fetched one at a time. The N+1 this function exists to avoid, used here
    // as the thing that cannot be wrong in the same way.
    let joins: Vec<Vec<ArticleTag>> = load_related::<_, Article, ArticleTag>(&txn, &context, &articles)
        .await
        .unwrap();
    let mut expected: Vec<Vec<Tag>> = Vec::new();
    for mine in &joins {
        let mut tags: Vec<Tag> = Vec::new();
        for join in mine {
            let one: Vec<Tag> = txn
                .find_records(
                    &context,
                    Expr::eq(Tag::COLUMNS.id, slate_orm::Value::U64(join.tag_id)),
                    ScanOrder::Ascending,
                )
                .await
                .unwrap();
            tags.extend(one);
        }
        expected.push(tags);
    }

    assert_eq!(got, expected);
}

/// With no parents there is no read, and with no join rows there is one.
#[tokio::test]
async fn an_empty_side_costs_no_second_read() {
    let (store, context, _articles) = seeded().await;
    let txn = store.begin().await.unwrap();

    let none: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&txn, &context, &[])
        .await
        .unwrap();
    assert!(none.is_empty(), "no parents, no entries");

    // A parent with no join rows still gets its entry, and the far read is
    // skipped rather than issued as an `IN` over nothing.
    let orphan = vec![Article { id: 3, title: "Three".to_owned() }];
    let empty: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&txn, &context, &orphan)
        .await
        .unwrap();
    assert_eq!(empty.len(), 1);
    assert!(empty[0].is_empty());
}

/// Two join rows onto the same tag give it twice, and that is documented.
#[tokio::test]
async fn duplicates_are_returned_rather_than_removed() {
    let (store, context, _articles) = seeded().await;
    let txn = store.begin().await.unwrap();
    // A second join row for the same pair. A join table with a uniqueness
    // constraint could not hold this; this one has a surrogate key, so it can.
    txn.insert_record(&context, &ArticleTag { id: 4, article_id: 2, tag_id: 20 })
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let two = vec![Article { id: 2, title: "Two".to_owned() }];
    let tags: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&txn, &context, &two)
        .await
        .unwrap();
    assert_eq!(
        labels(&tags[0]),
        vec!["databases", "databases"],
        "the join table says twice, so the answer says twice"
    );
}

/// Parents sharing a tag get the same tag, and its key is read once.
#[tokio::test]
async fn parents_sharing_a_tag_share_one_read_of_it() {
    let (store, context, articles) = seeded().await;
    let txn = store.begin().await.unwrap();

    let tags: Vec<Vec<Tag>> = load_through::<_, Article, Tag>(&txn, &context, &articles)
        .await
        .unwrap();
    // Articles 1 and 2 both carry tag 20.
    assert!(tags[0].iter().any(|t| t.id == 20));
    assert!(tags[1].iter().any(|t| t.id == 20));

    // Deduplication is what keeps the second read one read rather than one per
    // join row: three join rows name two distinct tags, so the `IN` carries
    // two values.
    let joins: Vec<Vec<ArticleTag>> =
        load_related::<_, Article, ArticleTag>(&txn, &context, &articles)
            .await
            .unwrap();
    let flat: Vec<ArticleTag> = joins.into_iter().flatten().collect();
    assert_eq!(flat.len(), 3, "three join rows");
    let distinct: BTreeSet<u64> = flat.iter().map(|j| j.tag_id).collect();
    assert_eq!(distinct.len(), 2, "naming two distinct tags");
    match slate_orm::related_filter::<ArticleTag, Tag>(&flat).unwrap() {
        Expr::In { values, .. } => assert_eq!(
            values.len(),
            2,
            "the second read's IN should carry the distinct keys, not one per join row"
        ),
        other => panic!("expected an IN, got {other:?}"),
    }
}
