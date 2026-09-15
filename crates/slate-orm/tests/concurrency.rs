//! Optimistic concurrency: the lost update, shown and then prevented.
//!
//! The failure this exists for is not exotic. Two requests read the same row,
//! each changes a different field, each writes it back. Neither transaction
//! overlaps the other in time, so the store's write-write detection sees
//! nothing, and the second write silently discards the first's edit. No error,
//! no conflict, no retry — just data that is quietly wrong.
//!
//! Every test here shows both halves: the plain update losing the edit, and
//! `replace_record` refusing. Showing only the refusal would not establish that
//! there was ever anything to refuse.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use slate_orm::{
    Action, Catalog, Grant, KernelError, OrmError, Principal, Record, RecordStore, Records,
    SecurityCatalog, SecurityContext, TableId, Value, memory::MemoryStore,
};

const POSTS: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "posts", id = 1)]
struct Post {
    #[record(pk)]
    id: u64,
    title: String,
    views: u64,
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

async fn seeded() -> RecordStore<MemoryStore> {
    let store = RecordStore::new(
        MemoryStore::new(),
        Catalog::from_tables([Post::table().clone()]).unwrap(),
        SecurityCatalog::new().grant(Grant::new("app", POSTS, Action::EVERYTHING)),
    );
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Post {
            id: 1,
            title: "first".to_owned(),
            views: 0,
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    store
}

async fn read(store: &RecordStore<MemoryStore>) -> Post {
    let txn = store.begin().await.unwrap();
    let post = txn
        .get_record::<Post>(&context(), &[Value::U64(1)])
        .await
        .unwrap()
        .expect("the row is there");
    txn.rollback();
    post
}

#[tokio::test]
async fn a_plain_update_loses_the_other_writer_s_edit() {
    let store = seeded().await;

    // Two readers, each holding the same row. Sequential transactions: there is
    // no overlap in time for the store to detect.
    let mine = read(&store).await;
    let theirs = read(&store).await;
    assert_eq!(mine, theirs);

    // They bump the view count and commit.
    let txn = store.begin().await.unwrap();
    txn.update_record(
        &context(),
        &Post {
            views: theirs.views + 1,
            ..theirs
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // I rename the post, from the copy I read before their write.
    let txn = store.begin().await.unwrap();
    txn.update_record(
        &context(),
        &Post {
            title: "renamed".to_owned(),
            ..mine
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // Their increment is gone. This is the defect, asserted so that the next
    // test means something — and it is silent: both writes succeeded.
    let now = read(&store).await;
    assert_eq!(now.title, "renamed");
    assert_eq!(
        now.views, 0,
        "their view count survived, so this test is no longer showing a lost update"
    );
}

#[tokio::test]
async fn replace_record_refuses_rather_than_losing_it() {
    let store = seeded().await;
    let mine = read(&store).await;
    let theirs = read(&store).await;

    let txn = store.begin().await.unwrap();
    txn.update_record(
        &context(),
        &Post {
            views: theirs.views + 1,
            ..theirs
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // The same rename as above, conditional on the row still being what I read.
    let txn = store.begin().await.unwrap();
    let refused = txn
        .replace_record(
            &context(),
            &mine,
            &Post {
                title: "renamed".to_owned(),
                ..mine.clone()
            },
        )
        .await
        .expect_err("the row moved under me");
    assert!(
        matches!(refused, OrmError::Kernel(KernelError::RowChanged { .. })),
        "{refused}"
    );
    txn.rollback();

    // And nothing was written: their increment is intact and my rename is not
    // half-applied.
    let now = read(&store).await;
    assert_eq!(now.views, 1);
    assert_eq!(now.title, "first");

    // Re-read, re-decide, write: the documented way out, and it works.
    let fresh = read(&store).await;
    let txn = store.begin().await.unwrap();
    txn.replace_record(
        &context(),
        &fresh,
        &Post {
            title: "renamed".to_owned(),
            ..fresh.clone()
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let now = read(&store).await;
    assert_eq!(now.title, "renamed");
    assert_eq!(now.views, 1, "the increment survived the rename");
}

#[tokio::test]
async fn an_unchanged_row_is_replaced_normally() {
    let store = seeded().await;
    let post = read(&store).await;
    let txn = store.begin().await.unwrap();
    txn.replace_record(
        &context(),
        &post,
        &Post {
            views: 7,
            ..post.clone()
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    assert_eq!(read(&store).await.views, 7);
}

#[tokio::test]
async fn a_row_deleted_under_the_writer_is_not_resurrected() {
    let store = seeded().await;
    let post = read(&store).await;

    let txn = store.begin().await.unwrap();
    txn.delete_record::<Post>(&context(), &[Value::U64(1)])
        .await
        .unwrap();
    txn.commit().await.unwrap();

    // Not `RowChanged` but `RowNotFound`, and the difference is worth keeping:
    // a row that moved can be re-read, and a row that is gone cannot. A
    // conditional update that resurrected it would also be a way to undo
    // somebody else's delete without noticing.
    let txn = store.begin().await.unwrap();
    let refused = txn
        .replace_record(
            &context(),
            &post,
            &Post {
                views: 9,
                ..post.clone()
            },
        )
        .await
        .expect_err("the row is gone");
    assert!(
        matches!(refused, OrmError::Kernel(KernelError::RowNotFound { .. })),
        "{refused}"
    );
    txn.rollback();

    let txn = store.begin().await.unwrap();
    assert!(
        txn.get_record::<Post>(&context(), &[Value::U64(1)])
            .await
            .unwrap()
            .is_none()
    );
    txn.rollback();
}

#[tokio::test]
async fn a_previous_naming_a_different_row_is_refused() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    txn.insert_record(
        &context(),
        &Post {
            id: 2,
            title: "second".to_owned(),
            views: 0,
        },
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    // Checking one row and writing another is a mistake no outcome of the read
    // could make sensible, so it is refused before the read rather than
    // producing "compared row 1, wrote row 2".
    let one = read(&store).await;
    let txn = store.begin().await.unwrap();
    let refused = txn
        .replace_record(
            &context(),
            &one,
            &Post {
                id: 2,
                title: "clobbered".to_owned(),
                views: 0,
            },
        )
        .await
        .expect_err("previous and next name different rows");
    assert!(
        matches!(refused, OrmError::Kernel(KernelError::RowChanged { .. })),
        "{refused}"
    );
    txn.rollback();

    // And the case that makes the check worth having rather than redundant:
    // `next` naming a row that does not exist at all. Without the comparison,
    // the read finds nothing and reports `RowNotFound` — sending the caller to
    // look for a missing row when what is actually wrong is that they handed in
    // a mismatched pair. The pair is their bug, and the error should say so.
    let txn = store.begin().await.unwrap();
    let refused = txn
        .replace_record(
            &context(),
            &one,
            &Post {
                id: 99,
                title: "nowhere".to_owned(),
                views: 0,
            },
        )
        .await
        .expect_err("previous and next name different rows");
    assert!(
        matches!(refused, OrmError::Kernel(KernelError::RowChanged { .. })),
        "a mismatched pair should be reported as such, not as a missing row: {refused}"
    );
    txn.rollback();
}
