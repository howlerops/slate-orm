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

/// Reads through the pool stay correct while the writer is committing.
///
/// Every other pool test is quiet: one write, then reads. The interesting
/// question is what a reader sees while the writer is moving, because a
/// replica's view advances on its own — between two reads in the same
/// transaction, or between an index entry and the row it points at.
///
/// The property is *not* that a reader sees the newest data; a replica lags by
/// design. It is that whatever it sees is a state the database was actually in:
/// row ids are contiguous from zero, because the writer commits them in order,
/// so any gap means a reader saw a later write without an earlier one.
#[tokio::test]
async fn reads_through_the_pool_stay_consistent_while_the_writer_commits() {
    const WRITES: u64 = 40;

    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let backend = Arc::new(
        SlateStore::open(PATH, Arc::clone(&object_store))
            .await
            .unwrap(),
    );
    let writer = common::record_store_shared(Arc::clone(&backend));
    let table = common::users();
    let root = SecurityContext::superuser();

    let replicas: Vec<Arc<dyn KvReadStore>> = vec![
        replica("load1", Arc::clone(&object_store)).await,
        replica("load2", Arc::clone(&object_store)).await,
        replica("load3", Arc::clone(&object_store)).await,
    ];
    let pool = Arc::new(
        ReplicaPool::new(replicas, writer.catalog().clone(), common::security())
            .with_writer(Arc::clone(&backend) as Arc<dyn KvReadStore>),
    );

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let writing = {
        let done = Arc::clone(&done);
        async move {
            for id in 0..WRITES {
                let txn = writer.begin().await.unwrap();
                txn.insert(
                    &root,
                    &table,
                    &common::user(common::TENANT_A, id, &format!("u{id}@x.com"), None, 30),
                )
                .await
                .unwrap();
                txn.commit().await.unwrap();
                tokio::task::yield_now().await;
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    };

    let reading = {
        let pool = Arc::clone(&pool);
        let done = Arc::clone(&done);
        async move {
            let table = common::users();
            let root = SecurityContext::superuser();
            let mut reads = 0usize;
            let mut deepest = 0usize;
            while !done.load(std::sync::atomic::Ordering::SeqCst) || reads < 8 {
                let snapshot = pool
                    .snapshot(Freshness::Any, None)
                    .await
                    .expect("a replica should always be able to serve `Any`");
                let mut ids: Vec<u64> = snapshot
                    .query(&root, &table, Expr::True, ScanOrder::Ascending)
                    .await
                    .unwrap()
                    .collect()
                    .await
                    .unwrap()
                    .iter()
                    .map(|r| match r.values()[1] {
                        slate_tuple::Value::U64(id) => id,
                        ref other => panic!("id was {other:?}"),
                    })
                    .collect();
                ids.sort_unstable();

                // Whatever prefix of the writes this replica has caught up to,
                // it must be a *prefix*: 0..n with nothing missing.
                let expected: Vec<u64> = (0..ids.len() as u64).collect();
                assert_eq!(
                    ids, expected,
                    "a replica served a state the writer was never in"
                );
                deepest = deepest.max(ids.len());
                reads += 1;
                drop(snapshot);
                tokio::task::yield_now().await;
            }
            (reads, deepest)
        }
    };

    let (_, (reads, deepest)) = tokio::join!(writing, reading);
    assert!(reads > 0, "the reader never ran");
    // Not an assertion about lag, just that the reader was not looking at an
    // empty database the whole time — otherwise it checked nothing.
    assert!(
        deepest > 0,
        "the reader never observed a single write in {reads} reads"
    );

    backend.close().await.unwrap();
}

/// Concurrent readers through the pool do not interfere with each other.
///
/// The pool hands out snapshots and routes by tenant affinity; several readers
/// holding snapshots at once is the ordinary case and the one where shared
/// state between them would show up.
#[tokio::test]
async fn many_concurrent_readers_each_get_a_usable_snapshot() {
    const READERS: usize = 12;

    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let backend = Arc::new(
        SlateStore::open(PATH, Arc::clone(&object_store))
            .await
            .unwrap(),
    );
    let writer = common::record_store_shared(Arc::clone(&backend));
    let table = common::users();
    let root = SecurityContext::superuser();

    let txn = writer.begin().await.unwrap();
    for id in 0..10u64 {
        txn.insert(
            &root,
            &table,
            &common::user(common::TENANT_A, id, &format!("u{id}@x.com"), None, 30),
        )
        .await
        .unwrap();
    }
    let token = txn.commit().await.unwrap().expect("rows were written");

    let replicas: Vec<Arc<dyn KvReadStore>> = vec![
        replica("c1", Arc::clone(&object_store)).await,
        replica("c2", Arc::clone(&object_store)).await,
    ];
    let pool = Arc::new(
        ReplicaPool::new(replicas, writer.catalog().clone(), common::security())
            .with_writer(Arc::clone(&backend) as Arc<dyn KvReadStore>),
    );

    let mut watermark = ReadWatermark::new();
    watermark.observe(token);
    let freshness = watermark.freshness();

    let mut tasks = Vec::new();
    for n in 0..READERS {
        let pool = Arc::clone(&pool);
        tasks.push(tokio::spawn(async move {
            let table = common::users();
            let root = SecurityContext::superuser();
            // Half route by tenant, half do not, so affinity and round-robin
            // are both in play at once.
            let tenant = (n % 2 == 0)
                .then(|| ReplicaPool::tenant_of(&table, &common::pk(common::TENANT_A, 1)))
                .flatten();
            let snapshot = pool
                .snapshot(freshness, tenant.as_ref())
                .await
                .expect("a replica caught up to the token should serve it");
            let rows = snapshot
                .query(&root, &table, Expr::True, ScanOrder::Ascending)
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            rows.len()
        }));
    }

    for task in tasks {
        let seen = task.await.unwrap();
        assert_eq!(
            seen, 10,
            "a reader waiting on a token saw {seen} rows instead of all ten"
        );
    }

    backend.close().await.unwrap();
}

/// **Does `manifest_poll_interval` govern what a `DbReader` sees?** Yes, and
/// this is the experiment `examples/deployed/README.md` asked for.
///
/// # The question
///
/// Two measurements disagreed. `slate-serverd`'s `process.rs` records a
/// ten-second poll against a 250 ms `catch_up` making 64 of 64
/// read-your-writes reads fall through to the writer — lag, exactly as the
/// interval predicts. The deployed example then set a 55-second poll against a
/// 60-second `catch_up` and observed **no** lag at all, on eight unpinned reads
/// at two dataset sizes. The README left that open, with two candidate
/// explanations: either the window is much narrower than the configuration
/// suggests, or `manifest_poll_interval` no longer governs a `DbReader`.
///
/// # The experiment
///
/// The same write, two readers, differing in nothing but the interval. Both
/// are opened *after* a first commit, so both start current and neither is
/// racing its own startup. Then a second commit lands, and the two are asked
/// what they can see.
///
/// # The answer
///
/// The interval governs. The eager reader, at a 20 ms poll, sees the second
/// commit in **7.8-11.2 ms** across five runs; the lazy one, at 300 s, has not
/// moved two seconds later. (Two seconds is the floor below; 100x the eager
/// time would be about one second, so the floor is what actually runs.)
/// So the deployed example's null result is a fact about *that* example rather
/// than about SlateDB — its unpinned reads happen well after the load, and a
/// 55-second interval has long since fired by then. The README's second
/// candidate explanation is withdrawn; the first is the right one, and it is
/// narrower still than "narrow": it is not a window at all, it is elapsed time
/// since the reader's last poll.
#[tokio::test]
async fn the_manifest_poll_interval_is_what_a_replica_can_see() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let writer = writer(Arc::clone(&object_store)).await;
    let table = common::users();
    let root = SecurityContext::superuser();

    // A first commit, so both readers open against a manifest that exists and
    // neither is answering about an empty database.
    let txn = writer.begin().await.unwrap();
    txn.insert(
        &root,
        &table,
        &common::user(common::TENANT_A, 1, "first@x.com", None, 30),
    )
    .await
    .unwrap();
    let first = txn.commit().await.unwrap().expect("the first commit wrote");

    let eager_reader = replica("eager", Arc::clone(&object_store)).await;
    // Long enough that it cannot fire during this test, and *not* derived from
    // the sleep below: a poll interval a test tuned to its own timing would
    // prove only that the test was tuned.
    let lazy_reader = Arc::new(
        SlateReader::open_with(
            "lazy",
            PATH,
            Arc::clone(&object_store),
            ReplicaMode::Following,
            DbReaderOptions {
                manifest_poll_interval: Duration::from_secs(300),
                // SlateDB refuses a lifetime under twice the interval, and
                // reports the interval *doubled* when it does — `lifetime=900s,
                // interval=1200s` for a 600-second interval, which is what
                // makes the message confusing enough to be worth a line here.
                // Four times over, so the margin is not the thing under test.
                checkpoint_lifetime: Duration::from_secs(1200),
                ..DbReaderOptions::default()
            },
        )
        .await
        .expect("open the lazy replica"),
    );
    for reader in [&eager_reader, &lazy_reader] {
        reader
            .wait_for_sequence(first.sequence(), Duration::from_secs(10))
            .await
            .expect("both readers start current");
    }
    // Where the lazy reader is *before* the second commit, so the assertion
    // below is "it did not move" rather than a guess at what number that is:
    // `visible_sequence` is only promised to be at or past a token's sequence,
    // not equal to it.
    let lazy_before = SlateReader::visible_sequence(lazy_reader.as_ref());

    // The second commit, which is the one the two readers will disagree about.
    let txn = writer.begin().await.unwrap();
    txn.insert(
        &root,
        &table,
        &common::user(common::TENANT_A, 2, "second@x.com", None, 31),
    )
    .await
    .unwrap();
    let second = txn
        .commit()
        .await
        .unwrap()
        .expect("the second commit wrote");
    assert!(
        second.sequence() > first.sequence(),
        "the second commit must advance the sequence for this to test anything"
    );

    let started = std::time::Instant::now();
    eager_reader
        .wait_for_sequence(second.sequence(), Duration::from_secs(10))
        .await
        .expect("a 20 ms poll sees a commit quickly");
    let eager_took = started.elapsed();

    // The lazy reader is given the same budget the eager one just used, times
    // a hundred, and still must not have moved. A bounded wait rather than the
    // full 600 seconds: the claim is "the interval governs", and a reader that
    // has not budged after 100x the time its eager twin needed establishes
    // that without the test taking ten minutes.
    let budget = (eager_took * 100).max(Duration::from_secs(2));
    // The window has to be long enough for "it did not move" to mean anything.
    // Without this line the test passes with a budget of *zero* — a mutation
    // setting it to `Duration::ZERO` survived, because a reader that cannot
    // advance also cannot advance in no time at all, and the assertion below
    // was true for a reason that had nothing to do with the poll interval.
    assert!(
        budget >= eager_took * 50,
        "the window is {budget:?} against an eager catch-up of {eager_took:?}, which is \
         not enough for its absence to be evidence of anything"
    );
    tokio::time::sleep(budget).await;
    let lazy_visible = SlateReader::visible_sequence(lazy_reader.as_ref());
    assert!(
        lazy_visible < second.sequence(),
        "the lazy reader reached sequence {lazy_visible} after {budget:?}, so a \
         600-second `manifest_poll_interval` did not hold it back — which is the \
         hypothesis this test exists to falsify. The eager reader took {eager_took:?}"
    );
    assert_eq!(
        lazy_visible, lazy_before,
        "and it is exactly where it was, rather than somewhere in between"
    );

    // And the rows agree with the sequences: the lazy reader has the first
    // user and not the second, which is what a stale replica *means*. Asserted
    // because a sequence that stalls while the data arrives anyway would be a
    // different and much worse bug.
    let stale = RecordStore::new(
        Arc::clone(&lazy_reader),
        writer.catalog().clone(),
        common::security(),
    );
    let snapshot = stale.snapshot().await.unwrap();
    assert!(
        snapshot
            .get(&root, &table, &common::pk(common::TENANT_A, 1))
            .await
            .unwrap()
            .is_some(),
        "the first user, which the lazy reader was current for"
    );
    assert_eq!(
        snapshot
            .get(&root, &table, &common::pk(common::TENANT_A, 2))
            .await
            .unwrap(),
        None,
        "and not the second, which landed after its last poll"
    );

    eager_reader.close().await.unwrap();
    lazy_reader.close().await.unwrap();
    writer.backend().close().await.unwrap();
}
