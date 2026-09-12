//! Read routing: where a read goes, and when a replica may serve it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use async_trait::async_trait;
use slate_kernel::error::Result;
use slate_kernel::store::{KvReadStore, KvSnapshot};
use slate_kernel::{
    Freshness, KernelError, ReadToken, ReadWatermark, RecordSnapshot, RecordStore, ReplicaPool,
    RoutingPolicy, SecurityCatalog, SecurityContext, memory::MemoryStore,
};
use slate_schema::{Catalog, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

/// A replica whose lag the test controls.
struct FakeReplica {
    name: String,
    sequence: AtomicU64,
    /// Sequence it jumps to if someone waits on it; `None` means it never
    /// catches up.
    catches_up_to: Option<u64>,
    waits: AtomicUsize,
    backing: MemoryStore,
}

impl FakeReplica {
    fn at(name: &str, sequence: u64) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            sequence: AtomicU64::new(sequence),
            catches_up_to: None,
            waits: AtomicUsize::new(0),
            backing: MemoryStore::new(),
        })
    }

    fn catching_up_to(name: &str, sequence: u64, target: u64) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            sequence: AtomicU64::new(sequence),
            catches_up_to: Some(target),
            waits: AtomicUsize::new(0),
            backing: MemoryStore::new(),
        })
    }
}

#[async_trait]
impl KvReadStore for FakeReplica {
    async fn snapshot(&self) -> Result<Box<dyn KvSnapshot + Send + '_>> {
        self.backing.snapshot().await
    }

    fn visible_sequence(&self) -> Option<u64> {
        Some(self.sequence.load(Ordering::SeqCst))
    }

    async fn wait_for_sequence(&self, sequence: u64, _timeout: Duration) -> Result<()> {
        self.waits.fetch_add(1, Ordering::SeqCst);
        match self.catches_up_to {
            Some(target) if target >= sequence => {
                self.sequence.store(target, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(KernelError::ReplicaTooStale {
                replica: self.name.clone(),
                required: sequence,
                visible: self.sequence.load(Ordering::SeqCst),
            }),
        }
    }

    fn replica_name(&self) -> &str {
        &self.name
    }
}

fn table() -> TableDef {
    TableDef::builder("t", TableId(1))
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        // Carries which replica the row was written into; see `stocked`.
        .column("marker", ValueType::U64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

fn pool(replicas: Vec<Arc<dyn KvReadStore>>) -> ReplicaPool {
    ReplicaPool::new(
        replicas,
        Catalog::from_tables([table()]).expect("catalog"),
        SecurityCatalog::new(),
    )
}

fn tenant(id: u64) -> Value {
    Value::U64(id)
}

async fn chosen(pool: &ReplicaPool, freshness: Freshness, affinity: Option<&Value>) -> String {
    pool.route(freshness, affinity)
        .await
        .expect("a store should be chosen")
        .replica_name()
        .to_owned()
}

/// The point of affinity: one tenant always lands on one replica, so its range
/// stays in that replica's block cache.
#[tokio::test]
async fn a_tenant_always_routes_to_the_same_replica() {
    let pool = pool(vec![
        FakeReplica::at("a", 10),
        FakeReplica::at("b", 10),
        FakeReplica::at("c", 10),
    ]);

    for id in 0..50u64 {
        let first = chosen(&pool, Freshness::Any, Some(&tenant(id))).await;
        for _ in 0..5 {
            assert_eq!(
                chosen(&pool, Freshness::Any, Some(&tenant(id))).await,
                first
            );
        }
    }
}

/// Affinity has to spread tenants, or it is just a hot spot with extra steps.
#[tokio::test]
async fn tenants_spread_across_the_replicas() {
    let names = ["a", "b", "c", "d"];
    let pool = pool(
        names
            .iter()
            .map(|n| FakeReplica::at(n, 10) as Arc<dyn KvReadStore>)
            .collect(),
    );

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for id in 0..1000u64 {
        *counts
            .entry(chosen(&pool, Freshness::Any, Some(&tenant(id))).await)
            .or_default() += 1;
    }

    assert_eq!(counts.len(), names.len(), "some replica got nothing");
    for (name, count) in &counts {
        assert!(
            (200..300).contains(count),
            "replica {name} got {count} of 1000, which is not a reasonable share"
        );
    }
}

/// Rendezvous hashing rather than modulo: losing a replica must move only the
/// tenants that were on it, not reshuffle every cache in the fleet.
#[tokio::test]
async fn losing_a_replica_moves_only_its_own_tenants() {
    let full = pool(vec![
        FakeReplica::at("a", 10),
        FakeReplica::at("b", 10),
        FakeReplica::at("c", 10),
        FakeReplica::at("d", 10),
    ]);
    let reduced = pool(vec![
        FakeReplica::at("a", 10),
        FakeReplica::at("b", 10),
        FakeReplica::at("c", 10),
    ]);

    let mut moved = 0;
    let mut were_on_d = 0;
    for id in 0..1000u64 {
        let before = chosen(&full, Freshness::Any, Some(&tenant(id))).await;
        let after = chosen(&reduced, Freshness::Any, Some(&tenant(id))).await;
        if before == "d" {
            were_on_d += 1;
            continue;
        }
        if before != after {
            moved += 1;
        }
    }

    assert!(
        were_on_d > 0,
        "the test needs some tenants to have been on d"
    );
    assert_eq!(
        moved, 0,
        "{moved} tenants moved that had no reason to; caches would go cold"
    );
}

#[tokio::test]
async fn without_a_tenant_reads_are_spread_round_robin() {
    let pool = pool(vec![
        FakeReplica::at("a", 10),
        FakeReplica::at("b", 10),
        FakeReplica::at("c", 10),
    ]);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..9 {
        seen.insert(chosen(&pool, Freshness::Any, None).await);
    }
    assert_eq!(seen.len(), 3, "round-robin should touch every replica");
}

/// Read-your-writes: a replica that has not reached the token must not serve
/// the read, even when affinity points at it.
#[tokio::test]
async fn a_lagging_replica_does_not_serve_a_token_it_cannot_meet() {
    let token = ReadToken::new(100);

    // Give every replica the same lag except one, then check that whichever
    // tenant we ask for, the caught-up replica is the one chosen.
    for id in 0..20u64 {
        let pool = pool(vec![
            FakeReplica::at("behind-1", 10),
            FakeReplica::at("behind-2", 42),
            FakeReplica::at("current", 100),
        ]);
        assert_eq!(
            chosen(&pool, Freshness::AtLeast(token), Some(&tenant(id))).await,
            "current",
            "a stale replica served a read that required sequence 100"
        );
    }
}

/// If waiting gets a replica there, that is better than spending the writer.
#[tokio::test]
async fn a_replica_that_can_catch_up_is_waited_for() {
    let closest = FakeReplica::catching_up_to("closest", 90, 100);
    let pool = pool(vec![
        FakeReplica::at("far-behind", 1),
        Arc::clone(&closest) as Arc<dyn KvReadStore>,
    ])
    .with_writer(FakeReplica::at("writer", u64::MAX));

    assert_eq!(
        chosen(&pool, Freshness::AtLeast(ReadToken::new(100)), None).await,
        "closest"
    );
    assert_eq!(
        closest.waits.load(Ordering::SeqCst),
        1,
        "should have waited on the replica nearest to catching up"
    );
}

#[tokio::test]
async fn the_writer_is_the_fallback_and_is_required_for_latest() {
    let stale = || {
        vec![
            FakeReplica::at("a", 1) as Arc<dyn KvReadStore>,
            FakeReplica::at("b", 2),
        ]
    };

    // No replica can reach the token and there is no writer: fail rather than
    // silently serve something older than promised.
    let no_writer = pool(stale());
    let outcome = no_writer
        .route(Freshness::AtLeast(ReadToken::new(999)), None)
        .await
        .map(|store| store.replica_name().to_owned());
    assert!(
        matches!(outcome, Err(KernelError::NoReplicaAvailable { .. })),
        "expected no replica available, got {outcome:?}"
    );

    // With a writer, fall back to it.
    let with_writer = pool(stale()).with_writer(FakeReplica::at("writer", u64::MAX));
    assert_eq!(
        chosen(&with_writer, Freshness::AtLeast(ReadToken::new(999)), None).await,
        "writer"
    );

    // `Latest` always means the writer, and needs one.
    assert_eq!(
        chosen(&with_writer, Freshness::Latest, None).await,
        "writer"
    );
    let no_writer = pool(stale());
    assert!(
        no_writer.route(Freshness::Latest, None).await.is_err(),
        "Latest without a writer must fail rather than pick a replica"
    );
}

#[tokio::test]
async fn affinity_can_be_turned_off() {
    let pool = pool(vec![
        FakeReplica::at("a", 10),
        FakeReplica::at("b", 10),
        FakeReplica::at("c", 10),
    ])
    .with_policy(RoutingPolicy {
        tenant_affinity: false,
        ..RoutingPolicy::default()
    });

    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..9 {
        seen.insert(chosen(&pool, Freshness::Any, Some(&tenant(7))).await);
    }
    assert_eq!(seen.len(), 3, "with affinity off, one tenant should spread");
}

/// Monotonic reads: threading the watermark means a later read can never be
/// served by a view behind one the caller already saw.
#[test]
fn the_watermark_only_moves_forward() {
    let mut watermark = ReadWatermark::new();
    assert_eq!(watermark.freshness(), Freshness::Any);

    watermark.observe(ReadToken::new(10));
    watermark.observe(ReadToken::new(4));
    assert_eq!(watermark.highest(), Some(ReadToken::new(10)));

    watermark.observe_commit(None);
    assert_eq!(watermark.highest(), Some(ReadToken::new(10)));

    watermark.observe_commit(Some(ReadToken::new(25)));
    assert_eq!(
        watermark.freshness(),
        Freshness::AtLeast(ReadToken::new(25))
    );
}

#[test]
fn a_token_is_met_only_by_a_view_that_reached_it() {
    let token = ReadToken::new(50);
    assert!(!token.satisfied_by(49));
    assert!(token.satisfied_by(50));
    assert!(token.satisfied_by(51));
}

/// Routing needs the tenant, and only a tenant-scoped table has one.
#[test]
fn tenant_affinity_comes_from_the_key() {
    let scoped = table();
    assert_eq!(
        ReplicaPool::tenant_of(&scoped, &[Value::U64(7), Value::U64(1)]),
        Some(Value::U64(7))
    );

    let unscoped = TableDef::builder("u", TableId(2))
        .column("id", ValueType::U64)
        .primary_key(["id"])
        .build()
        .unwrap();
    assert_eq!(ReplicaPool::tenant_of(&unscoped, &[Value::U64(1)]), None);
}

// --- what a routed read reports about itself ------------------------------

/// The one row every stocked replica holds, at the same key on each.
const THE_KEY: [Value; 2] = [Value::U64(1), Value::U64(1)];

/// A replica holding one row that says which replica it is.
///
/// Distinguishable contents are the whole design of the tests below. Replicas
/// that answer identically make the assertion unfalsifiable: a reported name
/// belonging to a store that served nothing reads exactly like a correct one,
/// and only rows that differ can tell them apart.
async fn stocked(name: &str, marker: u64) -> Arc<FakeReplica> {
    let replica = FakeReplica::at(name, 10);
    let store = RecordStore::new(
        replica.backing.clone(),
        Catalog::from_tables([table()]).expect("catalog"),
        SecurityCatalog::new(),
    );
    let txn = store.begin().await.expect("begin");
    txn.insert(
        &SecurityContext::superuser(),
        &table(),
        &Row::new(vec![Value::U64(1), Value::U64(1), Value::U64(marker)]),
    )
    .await
    .expect("insert");
    txn.commit().await.expect("commit");
    replica
}

/// The marker in the row this view can actually read.
async fn marker_read_through(view: &RecordSnapshot<'_>) -> u64 {
    let row = view
        .get(&SecurityContext::superuser(), &table(), &THE_KEY)
        .await
        .expect("the read should succeed")
        .expect("every stocked replica holds the row");
    match row.values()[2] {
        Value::U64(marker) => marker,
        ref other => panic!("marker column held {other:?}"),
    }
}

/// The property `snapshot_from` exists for: the store it names is the store
/// whose bytes came back, not merely one that could plausibly have served.
///
/// Every replica holds the same key with a different marker, so a name that
/// does not belong to the view is visible in the rows. Round-robin is the case
/// that matters — it is the one where routing twice gives two answers — so the
/// read carries no tenant.
#[tokio::test]
async fn the_store_a_snapshot_names_is_the_store_it_read_from() {
    let names = ["a", "b", "c", "d"];
    let mut replicas: Vec<Arc<dyn KvReadStore>> = Vec::new();
    for (marker, name) in names.iter().enumerate() {
        replicas.push(stocked(name, marker as u64).await);
    }
    let pool = pool(replicas);

    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..12 {
        let (view, store) = pool
            .snapshot_from(Freshness::Any, None)
            .await
            .expect("a store should be chosen");
        let marker = marker_read_through(&view).await;
        assert_eq!(
            names[marker as usize],
            store.replica_name(),
            "the snapshot read replica {} and reported {}",
            names[marker as usize],
            store.replica_name()
        );
        seen.insert(store.replica_name().to_owned());
    }
    assert_eq!(
        seen.len(),
        names.len(),
        "every read landed on {seen:?}; a pool that never moved would satisfy the assertion above without proving anything"
    );
}

/// Why the pairing has to come out of one call.
///
/// This is the bug `snapshot_from` removes, stated as behaviour: asking the
/// pool a second time where a read should go is a *new* decision, and on the
/// round-robin path it is a different one. Anything that opened a view and then
/// asked would be reporting a replica that served nothing.
#[tokio::test]
async fn asking_where_a_read_went_after_the_fact_answers_about_somewhere_else() {
    let pool = pool(vec![
        FakeReplica::at("a", 10),
        FakeReplica::at("b", 10),
        FakeReplica::at("c", 10),
    ]);

    let served = {
        let (_view, store) = pool
            .snapshot_from(Freshness::Any, None)
            .await
            .expect("a store should be chosen");
        store.replica_name().to_owned()
    };
    let asked_again = chosen(&pool, Freshness::Any, None).await;

    assert_ne!(
        served, asked_again,
        "route() happened to repeat itself, so this test is no longer describing the hazard"
    );
}

/// Affinity is the path where routing twice would have gone unnoticed: it is
/// stable, so the wrong call gives the right answer until the day the pool
/// falls back to round-robin. Pinning it here says the pairing holds on both
/// paths, not just the one that would have caught a mistake.
#[tokio::test]
async fn a_tenanted_read_also_names_the_store_it_read_from() {
    let names = ["a", "b", "c", "d"];
    let mut replicas: Vec<Arc<dyn KvReadStore>> = Vec::new();
    for (marker, name) in names.iter().enumerate() {
        replicas.push(stocked(name, marker as u64).await);
    }
    let pool = pool(replicas);

    let mut seen = std::collections::BTreeSet::new();
    for id in 0..20u64 {
        let (view, store) = pool
            .snapshot_from(Freshness::Any, Some(&tenant(id)))
            .await
            .expect("a store should be chosen");
        let marker = marker_read_through(&view).await;
        assert_eq!(
            names[marker as usize],
            store.replica_name(),
            "the snapshot read replica {} and reported {}",
            names[marker as usize],
            store.replica_name()
        );
        seen.insert(store.replica_name().to_owned());
    }
    assert!(
        seen.len() > 1,
        "twenty tenants all landed on {seen:?}; the assertion above never had to distinguish anything"
    );
}
