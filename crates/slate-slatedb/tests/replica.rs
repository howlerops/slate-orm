//! A real writer and a real replica over one object store.
//!
//! The pool's routing logic is unit-tested against fakes. This is the part that
//! can only be checked for real: that a replica actually observes the writer's
//! data, that the sequence a commit hands back is one the replica can be waited
//! on for, and that the security layer applies on the replica exactly as it does
//! on the writer.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use slate_kernel::store::KvReadStore;
use slate_kernel::{
    Expr, Freshness, ReadWatermark, RecordStore, ReplicaPool, ScanOrder, SecurityContext,
};
use slate_slatedb::{ReplicaMode, SlateReader, SlateStore};
use slatedb::config::DbReaderOptions;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;
use std::time::Duration;

const PATH: &str = "/records";

/// Poll often: the test would otherwise spend its time waiting for the default
/// manifest poll interval rather than exercising anything.
fn eager() -> DbReaderOptions {
    DbReaderOptions {
        manifest_poll_interval: Duration::from_millis(20),
        checkpoint_lifetime: Duration::from_secs(30),
        ..DbReaderOptions::default()
    }
}

async fn writer(store: Arc<dyn ObjectStore>) -> RecordStore<SlateStore> {
    common::record_store(SlateStore::open(PATH, store).await.expect("open writer"))
}

async fn replica(name: &str, store: Arc<dyn ObjectStore>) -> Arc<SlateReader> {
    Arc::new(
        SlateReader::open_with(name, PATH, store, ReplicaMode::Following, eager())
            .await
            .expect("open replica"),
    )
}

/// The core promise: commit, take the token, wait for it, and the replica has
/// the data.
#[tokio::test]
async fn a_replica_catches_up_to_a_commits_token() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let writer = writer(Arc::clone(&object_store)).await;
    let table = common::users();
    let root = SecurityContext::superuser();

    let txn = writer.begin().await.unwrap();
    txn.insert(
        &root,
        &table,
        &common::user(common::TENANT_A, 1, "a@x.com", None, 30),
    )
    .await
    .unwrap();
    let token = txn.commit().await.unwrap().expect("the commit wrote rows");

    let replica = replica("r1", Arc::clone(&object_store)).await;
    replica
        .wait_for_sequence(token.sequence(), Duration::from_secs(10))
        .await
        .expect("the replica should catch up");
    assert!(
        SlateReader::visible_sequence(replica.as_ref()) >= token.sequence(),
        "wait returned before the replica had actually reached the sequence"
    );

    let reads = RecordStore::new(
        Arc::clone(&replica),
        writer.catalog().clone(),
        common::security(),
    );
    let snapshot = reads.snapshot().await.unwrap();
    let row = snapshot
        .get(&root, &table, &common::pk(common::TENANT_A, 1))
        .await
        .unwrap();
    assert_eq!(
        row,
        Some(common::user(common::TENANT_A, 1, "a@x.com", None, 30))
    );

    replica.close().await.unwrap();
    writer.backend().close().await.unwrap();
}

/// A replica must not be a way around the policy.
#[tokio::test]
async fn security_applies_on_a_replica_exactly_as_on_the_writer() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let writer = writer(Arc::clone(&object_store)).await;
    let table = common::users();
    let root = SecurityContext::superuser();

    let txn = writer.begin().await.unwrap();
    for row in [
        common::user(common::TENANT_A, 1, "adult@a.com", None, 40),
        common::user(common::TENANT_A, 2, "minor@a.com", None, 12),
        common::user(common::TENANT_B, 1, "adult@b.com", None, 40),
    ] {
        txn.insert(&root, &table, &row).await.unwrap();
    }
    let token = txn.commit().await.unwrap().expect("rows were written");

    let replica = replica("r1", Arc::clone(&object_store)).await;
    replica
        .wait_for_sequence(token.sequence(), Duration::from_secs(10))
        .await
        .unwrap();

    let reads = RecordStore::new(
        Arc::clone(&replica),
        writer.catalog().clone(),
        common::security(),
    );
    let snapshot = reads.snapshot().await.unwrap();
    let rows = snapshot
        .query(
            &common::member(common::TENANT_A),
            &table,
            Expr::True,
            ScanOrder::Ascending,
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    // The other tenant and the under-age row must both be absent, exactly as
    // they are through the writer.
    assert_eq!(rows.len(), 1, "the replica leaked rows: {rows:?}");

    replica.close().await.unwrap();
    writer.backend().close().await.unwrap();
}

/// End to end through the pool, including the writer as a fallback.
#[tokio::test]
async fn a_pool_serves_reads_and_falls_back_for_the_freshest() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    // One writer, shared: opening a second would fence this one. See
    // `a_second_writer_fences_the_first`.
    let backend = Arc::new(
        SlateStore::open(PATH, Arc::clone(&object_store))
            .await
            .unwrap(),
    );
    let writer = common::record_store_shared(Arc::clone(&backend));
    let table = common::users();
    let root = SecurityContext::superuser();

    let txn = writer.begin().await.unwrap();
    txn.insert(
        &root,
        &table,
        &common::user(common::TENANT_A, 1, "a@x.com", None, 30),
    )
    .await
    .unwrap();
    let token = txn.commit().await.unwrap().expect("rows were written");

    let replicas: Vec<Arc<dyn KvReadStore>> = vec![
        replica("r1", Arc::clone(&object_store)).await,
        replica("r2", Arc::clone(&object_store)).await,
    ];
    let pool = ReplicaPool::new(replicas, writer.catalog().clone(), common::security())
        .with_writer(Arc::clone(&backend) as Arc<dyn KvReadStore>);

    // Threading the commit's token gives read-your-writes off a replica.
    let mut watermark = ReadWatermark::new();
    watermark.observe(token);

    let tenant = ReplicaPool::tenant_of(&table, &common::pk(common::TENANT_A, 1));
    let snapshot = pool
        .snapshot(watermark.freshness(), tenant.as_ref())
        .await
        .expect("a replica should be able to serve this");
    assert!(
        snapshot
            .get(&root, &table, &common::pk(common::TENANT_A, 1))
            .await
            .unwrap()
            .is_some()
    );
    drop(snapshot);

    // `Latest` goes to the writer, which is the only view that can see a write
    // before it is flushed.
    let chosen = pool
        .route(Freshness::Latest, tenant.as_ref())
        .await
        .unwrap();
    assert_eq!(chosen.replica_name(), "writer");

    backend.close().await.unwrap();
}

/// Single-writer means single writer: starting a second one takes over, and the
/// first must find out rather than keep writing into a split brain.
#[tokio::test]
async fn a_second_writer_fences_the_first() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let table = common::users();
    let root = SecurityContext::superuser();

    let first = writer(Arc::clone(&object_store)).await;
    let txn = first.begin().await.unwrap();
    txn.insert(
        &root,
        &table,
        &common::user(common::TENANT_A, 1, "a@x.com", None, 30),
    )
    .await
    .unwrap();
    txn.commit()
        .await
        .expect("the first writer owns the database");

    // A second writer takes over.
    let second = writer(Arc::clone(&object_store)).await;

    // The first writer's next commit must fail, and specifically as a fenced
    // writer rather than as a retryable conflict — a head node that retried
    // this would be a split brain looping forever.
    let txn = first.begin().await.unwrap();
    let outcome = async {
        txn.insert(
            &root,
            &table,
            &common::user(common::TENANT_A, 2, "b@x.com", None, 31),
        )
        .await?;
        txn.commit().await
    }
    .await;

    match outcome {
        Err(slate_kernel::KernelError::WriterFenced) => {}
        other => panic!("expected the fenced writer to be told so, got {other:?}"),
    }
    assert!(
        !slate_kernel::KernelError::WriterFenced.is_retryable(),
        "a fenced writer must never be retried"
    );

    second.backend().close().await.unwrap();
}
