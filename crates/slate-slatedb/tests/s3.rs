//! The shared suite, run over the S3 protocol.
//!
//! By default this speaks to an S3 server running in this process, so the S3
//! code path is genuinely exercised without a Docker service container. Set
//! `SLATE_S3_BUCKET` (and the other `SLATE_S3_*` variables) to run the same
//! checks against MinIO, Tigris, Cloudflare R2 or AWS instead.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
#[path = "common/s3server.rs"]
mod s3server;

use common::restart::Backing;
use slate_kernel::RecordStore;
use slate_slatedb::{S3Config, SlateStore};
use std::sync::atomic::{AtomicU64, Ordering};

/// Keeps each test in its own prefix so they cannot collide, which matters when
/// the target is a shared bucket on a real service.
static RUN: AtomicU64 = AtomicU64::new(0);

fn unique_path(name: &str) -> String {
    let n = RUN.fetch_add(1, Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("/slate-orm-tests/{name}-{stamp}-{n}")
}

/// A store on whichever S3 service the environment selects.
///
/// The `LocalS3` is returned alongside so the caller keeps it alive; dropping it
/// would stop the server underneath the store.
async fn s3_store(name: &str) -> (RecordStore<SlateStore>, Option<s3server::LocalS3>) {
    match S3Config::from_env() {
        // A real service was configured; use it.
        Some(config) => {
            let backend = SlateStore::open_s3(unique_path(name), config)
                .await
                .expect("open against the configured S3 service");
            (common::record_store(backend), None)
        }
        None => {
            let server = s3server::LocalS3::start("slate-orm").await;
            let backend = SlateStore::open_s3(unique_path(name), server.config())
                .await
                .expect("open against the in-process S3 server");
            (common::record_store(backend), Some(server))
        }
    }
}

/// Generate one test per shared check.
macro_rules! s3_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let (store, _server) = s3_store(stringify!($name)).await;
                common::$name(&store).await;
                store.backend().close().await.unwrap();
            }
        )*
    };
}

s3_tests! {
    round_trip,
    rollback_leaves_no_index_state,
    concurrent_unique_writers_conflict,
    sequential_duplicate_names_the_index,
    scans_are_ordered_both_ways,
    update_retires_the_old_index_entry,
    tenant_scoping_and_policies_hold,
}

/// The restart checks, over S3.
///
/// These matter more here than over the in-memory object store: reopening is
/// exactly where a backend's manifest handling and caching could differ, and
/// "the data is still there" is not a claim that transfers from one substrate
/// to another by argument.
///
/// Each takes a [`Backing`] rather than a store, because a restart means
/// opening the same location twice — so the server has to outlive both opens.
macro_rules! s3_restart_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let (backing, _server) = s3_backing(stringify!($name)).await;
                common::restart::$name(&backing).await;
            }
        )*
    };
}

/// A location on whichever S3 service the environment selects, plus the server
/// keeping it alive when that service is the in-process one.
async fn s3_backing(name: &str) -> (Backing, Option<s3server::LocalS3>) {
    match S3Config::from_env() {
        Some(config) => (Backing::S3(config, unique_path(name)), None),
        None => {
            let server = s3server::LocalS3::start("slate-orm").await;
            let backing = Backing::S3(server.config(), unique_path(name));
            (backing, Some(server))
        }
    }
}

s3_restart_tests! {
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
