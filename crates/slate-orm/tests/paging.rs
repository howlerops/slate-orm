//! `page_records`, and the cursor a caller does not have to build.
//!
//! The kernel's own pagination suite covers what a cursor *does*. What is left
//! here is the part that only exists at this layer: the caller never names the
//! primary key. That matters because building a cursor by hand means knowing
//! which columns are the key and in what order, at every call site, and getting
//! it wrong is invisible — a cursor built from the wrong column still pages,
//! just through a sequence nobody asked for.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use slate_orm::{
    Action, Catalog, Grant, Page, Principal, Query, Record, RecordStore, Records, SecurityCatalog,
    SecurityContext, TableId, Value, memory::MemoryStore,
};

const NOTES: TableId = TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "notes", id = 1)]
struct Note {
    #[record(pk)]
    id: u64,
    body: String,
}

/// A composite key, because the single-column case is the one where a caller
/// building the cursor by hand is least likely to get it wrong.
#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "entries", id = 2, tenant = "tenant")]
struct Entry {
    #[record(pk)]
    tenant: u64,
    #[record(pk)]
    id: u64,
    body: String,
}

fn context() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(1))
            .with_role("app"),
    )
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([Note::table().clone(), Entry::table().clone()]).unwrap();
    let security = SecurityCatalog::new()
        .grant(Grant::new("app", NOTES, Action::EVERYTHING))
        .grant(Grant::new("app", TableId(2), Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    for id in 1..=7u64 {
        txn.insert_record(
            &context(),
            &Note {
                id,
                body: format!("note {id}"),
            },
        )
        .await
        .unwrap();
        txn.insert_record(
            &context(),
            &Entry {
                tenant: 1,
                id,
                body: format!("entry {id}"),
            },
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    store
}

#[tokio::test]
async fn paging_walks_the_whole_table_without_the_caller_naming_a_key() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();

    let mut seen: Vec<u64> = Vec::new();
    let mut cursor: Option<Vec<Value>> = None;
    let mut requests = 0usize;
    loop {
        let mut query = Query::all().limit(3);
        if let Some(at) = cursor.take() {
            query = query.after(at);
        }
        let page: Page<Note> = txn.page_records(&context(), &query).await.unwrap();
        requests += 1;
        seen.extend(page.rows.iter().map(|note| note.id));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
        assert!(requests < 10, "the loop is not terminating");
    }
    assert_eq!(seen, (1..=7).collect::<Vec<u64>>());
    // Seven rows in pages of three: 3, 3, 1. The third page is short, so it is
    // the last and there is no fourth request. A cursor API that could not tell
    // a short page from a full one would have made four.
    assert_eq!(requests, 3);
}

#[tokio::test]
async fn a_composite_key_pages_without_the_caller_assembling_it() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();

    let first: Page<Entry> = txn
        .page_records(&context(), &Query::all().limit(4))
        .await
        .unwrap();
    assert_eq!(
        first.rows.iter().map(|e| e.id).collect::<Vec<u64>>(),
        vec![1, 2, 3, 4]
    );
    // Both key columns, in key order, and the caller wrote neither. The tenant
    // leads because it leads the primary key — which is exactly the detail a
    // hand-built cursor gets wrong, and which then pages through one tenant's
    // rows using another tenant's boundary.
    let next = first.next.expect("a full page has a cursor");
    assert_eq!(next, vec![Value::U64(1), Value::U64(4)]);

    let second: Page<Entry> = txn
        .page_records(&context(), &Query::all().limit(4).after(next))
        .await
        .unwrap();
    assert_eq!(
        second.rows.iter().map(|e| e.id).collect::<Vec<u64>>(),
        vec![5, 6, 7]
    );
    assert!(second.is_last());
}

#[tokio::test]
async fn a_full_last_page_still_returns_a_cursor_and_the_next_is_empty() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();

    // Seven rows in pages of seven: the first page is full, so nothing about it
    // proves it is the last. The documented behaviour is a cursor and one more
    // empty request, rather than a lookahead read on every page to avoid it.
    let full: Page<Note> = txn
        .page_records(&context(), &Query::all().limit(7))
        .await
        .unwrap();
    assert_eq!(full.rows.len(), 7);
    let next = full.next.expect("a full page cannot prove it is the last");

    let empty: Page<Note> = txn
        .page_records(&context(), &Query::all().limit(7).after(next))
        .await
        .unwrap();
    assert!(empty.rows.is_empty());
    assert!(empty.is_last());
}

#[tokio::test]
async fn a_page_with_no_limit_is_refused() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();
    // Not defaulted to some page size: a silent default is a number the caller
    // did not choose deciding how much of their table they read.
    let error = txn
        .page_records::<Note>(&context(), &Query::all())
        .await
        .expect_err("a page needs a size");
    assert!(error.to_string().contains("limit"), "{error}");
}
