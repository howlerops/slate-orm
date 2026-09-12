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
