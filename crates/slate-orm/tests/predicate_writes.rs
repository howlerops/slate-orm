//! Predicate writes through the typed record layer.
//!
//! The kernel's own suite (`slate-kernel/tests/predicate_writes.rs`) establishes
//! that `delete_where` and `update_where` are correct. This file establishes the
//! narrower thing the record layer is responsible for: that a call written
//! against a *type* — naming columns through the derive's `COLUMNS` constant
//! rather than by ordinal — reaches the same place.
//!
//! That is worth its own test because the ordinal is exactly what the record
//! layer exists to stop a caller getting wrong, and an off-by-one between
//! `Post::COLUMNS.views` and the column the kernel updates would be invisible
//! in a fixture where every column happens to hold the same type.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_orm::{
    Action, Catalog, CmpOp, Expr, Grant, Principal, Record, RecordStore, Records, Scalar,
    SecurityCatalog, SecurityContext, TableId, Value, memory::MemoryStore,
};

const POSTS: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "posts", id = 1)]
struct Post {
    #[record(pk)]
    id: u64,
    author_id: u64,
    views: i64,
    title: String,
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([Post::table().clone()]).unwrap();
    let security = SecurityCatalog::new().grant(Grant::new("app", POSTS, Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    for (id, author_id) in [(1u64, 7u64), (2, 7), (3, 8)] {
        txn.insert_record(
            &context(),
            &Post {
                id,
                author_id,
                views: 10 * id as i64,
                title: format!("post {id}"),
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    store
}

async fn posts(store: &RecordStore<MemoryStore>) -> Vec<Post> {
    let txn = store.begin().await.unwrap();
    let mut got: Vec<Post> = txn
        .query_records(&context(), &slate_orm::Query::all())
        .await
        .unwrap();
    txn.rollback();
    got.sort_by_key(|p| p.id);
    got
}

#[tokio::test]
async fn delete_records_where_removes_the_matching_rows() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let removed = txn
        .delete_records_where::<Post>(
            &context(),
            Expr::compare(Post::COLUMNS.author_id, CmpOp::Eq, Value::U64(7)),
        )
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(removed, 2);
    let left: Vec<u64> = posts(&store).await.into_iter().map(|p| p.id).collect();
    assert_eq!(left, vec![3]);
}

/// `views = views + 1`, written against the type.
///
/// The assertion that matters is not the arithmetic but *which column moved*:
/// `views` changes and `author_id` does not, so an assignment aimed at the
/// wrong ordinal fails here rather than in production.
#[tokio::test]
async fn update_records_where_increments_the_named_column() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    let written = txn
        .update_records_where::<Post>(
            &context(),
            Expr::compare(Post::COLUMNS.author_id, CmpOp::Eq, Value::U64(7)),
            &[(
                Post::COLUMNS.views,
                Scalar::Add(
                    Box::new(Scalar::column(Post::COLUMNS.views)),
                    Box::new(Scalar::literal(Value::I64(1))),
                ),
            )],
        )
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(written, 2);
    let got = posts(&store).await;
    assert_eq!(got[0].views, 11, "{got:?}");
    assert_eq!(got[1].views, 21, "{got:?}");
    assert_eq!(got[2].views, 30, "the unmatched row changed: {got:?}");
    for post in &got {
        assert!(
            post.author_id == 7 || post.author_id == 8,
            "an assignment landed on the wrong column: {post:?}"
        );
        assert!(post.title.starts_with("post "), "{post:?}");
    }
}
