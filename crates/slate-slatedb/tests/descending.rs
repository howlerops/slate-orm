//! A descending scan over a compacted sorted run comes back in order.
//!
//! SlateDB 0.16 got this wrong: inside a sorted run that spans several SSTs it
//! walked each SST backwards but the SSTs themselves forwards, so a descending
//! scan over compacted data returned runs of descending keys in ascending
//! blocks (slatedb#2067, fixed in 0.17). `ScanOrder::Descending` maps straight
//! onto SlateDB's, and the kernel's reverse-ordered index scans depend on it.
//!
//! Nothing here caught it before, because every other descending scan in this
//! crate reads five rows from a memtable: no compaction, so no sorted run, so
//! the broken path never ran. This test exists to put data where the bug was —
//! small SSTs, a compactor that rewrites rather than moves, and a wait until
//! the manifest shows a sorted run of more than one SST — and then read it
//! through `SlateStore`, not through `slatedb` directly, because the claim is
//! about what this crate's callers see.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use slate_kernel::store::{KeyRange, KvReadStore, KvSnapshot, KvStore, ScanOrder};
use slate_slatedb::SlateStore;
use slatedb::Db;
use slatedb::config::{CompactionWorkerOptions, CompactorOptions, Settings};
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;
use std::time::{Duration, Instant};

const KEYS: u32 = 2_000;
const SST_BYTES: usize = 8 * 1024;

#[tokio::test]
async fn a_descending_scan_over_a_compacted_sorted_run_is_in_order() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let settings = Settings {
        l0_sst_size_bytes: SST_BYTES,
        compactor_options: Some(CompactorOptions {
            poll_interval: Duration::from_millis(20),
            commit_compacted_interval: Duration::from_millis(20),
            // A trivial move puts one L0 SST into a run unchanged, which is a
            // run of one SST: exactly the shape the bug did not touch.
            enable_trivial_move: false,
            worker: Some(CompactionWorkerOptions {
                max_sst_size: SST_BYTES,
                ..CompactionWorkerOptions::default()
            }),
            ..CompactorOptions::default()
        }),
        ..Settings::default()
    };
    let db = Arc::new(
        Db::builder("descending", Arc::clone(&object_store))
            .with_settings(settings)
            .build()
            .await
            .expect("open SlateDB"),
    );

    // Several flushes, so L0 holds several SSTs and the compactor has
    // something to merge.
    let value = vec![b'v'; 64];
    for chunk in 0..8 {
        for i in (chunk * KEYS / 8)..((chunk + 1) * KEYS / 8) {
            db.put(&key(i), &value).await.expect("put");
        }
        db.flush().await.expect("flush");
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let manifest = db.manifest();
        let widest = manifest
            .compacted()
            .iter()
            .map(|run| run.sst_views().len())
            .max()
            .unwrap_or(0);
        // Not also "L0 is empty": the size-tiered scheduler leaves a last
        // small L0 SST that has nothing of its size to merge with, so that
        // waits forever. The scan merges L0 in regardless; the bug is the run.
        if widest >= 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no multi-SST sorted run after 30s: widest run {widest}, {} L0 SSTs",
            manifest.l0().len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Both ways a caller reads: a transaction, which is what the record
    // layer's queries use, and a snapshot, which is what a replica serves.
    let store = SlateStore::from_db(Arc::clone(&db));
    let txn = store.begin().await.expect("begin");
    assert_descending("transaction", &*txn).await;
    let snapshot = store.snapshot().await.expect("snapshot");
    assert_descending("snapshot", &*snapshot).await;
}

async fn assert_descending(via: &str, view: &(dyn KvSnapshot + Send + '_)) {
    let mut iter = view
        .scan(KeyRange::all(), ScanOrder::Descending)
        .await
        .expect("scan");
    let mut seen = Vec::with_capacity(KEYS as usize);
    while let Some(kv) = iter.next().await.expect("next") {
        seen.push(kv.key.to_vec());
    }

    let expected: Vec<Vec<u8>> = (0..KEYS).rev().map(key).collect();
    assert_eq!(
        seen.len(),
        expected.len(),
        "{via}: every key comes back once"
    );
    if let Some(at) = seen.iter().zip(&expected).position(|(a, b)| a != b) {
        panic!(
            "{via}: descending scan out of order at position {at}: got {:?}, expected {:?}",
            String::from_utf8_lossy(&seen[at]),
            String::from_utf8_lossy(&expected[at]),
        );
    }
}

fn key(i: u32) -> Vec<u8> {
    format!("k{i:06}").into_bytes()
}
