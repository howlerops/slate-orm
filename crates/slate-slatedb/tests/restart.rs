//! The restart suite, over an in-memory object store.
//!
//! Same checks as the restart half of `s3.rs`, different substrate. These are
//! the fast ones: no sockets, no signing, no HTTP. The checks themselves live
//! in `common/restart.rs`, parameterised by where the bytes live.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::restart::Backing;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;

fn backing() -> Backing {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    Backing::Memory(object_store, "/records".to_owned())
}

macro_rules! restart_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                common::restart::$name(&backing()).await;
            }
        )*
    };
}

restart_tests! {
    a_different_store_is_a_different_database,
    committed_rows_survive_a_restart,
    a_unique_index_still_constrains_after_a_restart,
    the_index_and_the_table_agree_after_a_restart,
    an_uncommitted_transaction_leaves_nothing_behind,
    a_delete_stays_deleted_across_a_restart,
    tenant_isolation_holds_after_a_restart,
    writes_after_a_restart_survive_the_next_one,
    a_covering_scan_agrees_with_the_table_after_a_restart,
}
