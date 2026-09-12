//! The shared suite, run over an in-memory object store.
//!
//! Same checks as `s3.rs`, different substrate. These are the fast ones: no
//! sockets, no signing, no HTTP.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use slate_kernel::RecordStore;
use slate_slatedb::SlateStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;

async fn memory_store() -> RecordStore<SlateStore> {
    let backend = SlateStore::open("/records", Arc::new(InMemory::new()))
        .await
        .expect("open slatedb");
    common::record_store(backend)
}

macro_rules! memory_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let store = memory_store().await;
                common::$name(&store).await;
                store.backend().close().await.unwrap();
            }
        )*
    };
}

memory_tests! {
    round_trip,
    rollback_leaves_no_index_state,
    concurrent_unique_writers_conflict,
    sequential_duplicate_names_the_index,
    scans_are_ordered_both_ways,
    update_retires_the_old_index_entry,
    tenant_scoping_and_policies_hold,
}
