//! Who may write, and what happens when that changes underneath the node.
//!
//! Two claims are worth more than the rest and both are checked end to end,
//! through the wire, rather than against `Leadership` on its own:
//!
//! **A node that is not the leader refuses writes and says where to go.** The
//! refusal is `UNAVAILABLE` rather than `FAILED_PRECONDITION` because the
//! request should be retried, just not here, and gRPC clients act on that
//! distinction automatically.
//!
//! **After a fence the refusal is local.** `handover.rs` established that a
//! fenced writer fails at `begin`, so a node that merely forwarded the storage
//! error would still be making a round trip into SlateDB for every write it was
//! never going to accept. `being_fenced_stops_the_node_touching_the_store`
//! counts the calls, which is the only way to tell the two apart from outside.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use async_trait::async_trait;
use common::{HeldByAnother, TestClock, app, catalog, doc, docs_query, drain, lease_at, security};
use object_store::ObjectStore;
use object_store::memory::InMemory;
use slate_kernel::error::Result as KernelResult;
use slate_kernel::memory::MemoryStore;
use slate_kernel::store::{KvSnapshot, KvTransaction};
use slate_kernel::{KernelError, KvReadStore, KvStore};
use slate_server::leadership::{Leadership, Standing, StepDown};
use slate_server::lease::{Lease, LeaseError, Term};
use slate_server::proto as pb;
use slate_server::{Head, HeadConfig, MetadataIdentity};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use tonic::Code;

/// A store that can be fenced, exactly the way SlateDB fences: at `begin`, for
/// reads as well as writes, and permanently.
#[derive(Debug)]
struct Fenceable {
    inner: Arc<MemoryStore>,
    fenced: AtomicBool,
    /// How many times the store was asked to open anything. The head node is
    /// supposed to stop asking once it has stepped down.
    opens: AtomicUsize,
}

impl Fenceable {
    fn over(inner: Arc<MemoryStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fenced: AtomicBool::new(false),
            opens: AtomicUsize::new(0),
        })
    }

    fn fence(&self) {
        self.fenced.store(true, Ordering::SeqCst);
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    fn check(&self) -> KernelResult<()> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        if self.fenced.load(Ordering::SeqCst) {
            return Err(KernelError::WriterFenced);
        }
        Ok(())
    }
}

#[async_trait]
impl KvStore for Fenceable {
    async fn begin(&self) -> KernelResult<Box<dyn KvTransaction + Send + '_>> {
        self.check()?;
        self.inner.begin().await
    }
}

#[async_trait]
impl KvReadStore for Fenceable {
    async fn snapshot(&self) -> KernelResult<Box<dyn KvSnapshot + Send + '_>> {
        // A fenced writer cannot read either; see `handover.rs`.
        self.check()?;
        self.inner.snapshot().await
    }

    fn visible_sequence(&self) -> Option<u64> {
        self.inner.visible_sequence()
    }

    fn replica_name(&self) -> &str {
        "the-writer"
    }
}

/// A replica that shares the writer's data and cannot be fenced.
///
/// Real replicas lag; this one does not, because these tests are about who may
/// write rather than about freshness — `freshness.rs` has the lagging one.
#[derive(Debug)]
struct Replica(Arc<MemoryStore>);

#[async_trait]
impl KvReadStore for Replica {
    async fn snapshot(&self) -> KernelResult<Box<dyn KvSnapshot + Send + '_>> {
        self.0.snapshot().await
    }

    fn visible_sequence(&self) -> Option<u64> {
        self.0.visible_sequence()
    }

    fn replica_name(&self) -> &str {
        "the-replica"
    }
}

/// A lease that counts how often it is asked for anything.
#[derive(Debug, Default)]
struct CountingLease {
    acquires: AtomicUsize,
    releases: AtomicUsize,
}

#[async_trait]
impl Lease for CountingLease {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        let generation = self.acquires.fetch_add(1, Ordering::SeqCst) as u64 + 1;
        Ok(Term {
            generation,
            holder: "this-node".to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        self.acquire().await
    }

    async fn release(&self) -> Result<(), LeaseError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(None)
    }

    fn held(&self) -> Option<Term> {
        None
    }

    fn holder(&self) -> &str {
        "this-node"
    }
}

/// A lease whose renewal fails the way a slow object store makes it fail:
/// not "somebody else has it", just "no answer".
#[derive(Debug)]
struct RenewalKeepsFailing;

#[async_trait]
impl Lease for RenewalKeepsFailing {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        Ok(Term {
            generation: 1,
            holder: "this-node".to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        Err(LeaseError::Backend(Box::new(std::io::Error::other(
            "the object store did not answer",
        ))))
    }

    async fn release(&self) -> Result<(), LeaseError> {
        Ok(())
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(None)
    }

    fn held(&self) -> Option<Term> {
        None
    }

    fn holder(&self) -> &str {
        "this-node"
    }
}

/// A lease that has been taken by somebody else since we last renewed.
#[derive(Debug)]
struct TakenFromUnderUs;

#[async_trait]
impl Lease for TakenFromUnderUs {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        Ok(Term {
            generation: 1,
            holder: "this-node".to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        Err(LeaseError::Lost {
            holder: "the-other-node".to_owned(),
            generation: 1,
        })
    }

    async fn release(&self) -> Result<(), LeaseError> {
        Ok(())
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(None)
    }

    fn held(&self) -> Option<Term> {
        None
    }

    fn holder(&self) -> &str {
        "this-node"
    }
}

fn head_over(
    writer: Arc<Fenceable>,
    replicas: Vec<Arc<dyn KvReadStore>>,
    leadership: Arc<Leadership>,
) -> Head<Fenceable> {
    Head::new(
        HeadConfig::new(catalog(), security()),
        writer,
        replicas,
        leadership,
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    )
}

fn insert_one(id: u64) -> pb::InsertRequest {
    pb::InsertRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        rows: vec![slate_server::convert::row_to_proto(&doc(
            id, "kind-a", 10, None,
        ))],
        upsert: false,
    }
}

// --- following ------------------------------------------------------------

#[tokio::test]
async fn a_follower_refuses_writes_and_names_the_leader() {
    let backing = Arc::new(MemoryStore::new());
    let writer = Fenceable::over(Arc::clone(&backing));
    let leadership = Leadership::new(Arc::new(HeldByAnother));
    assert!(!leadership.campaign().await, "the lease is held elsewhere");

    let serving = common::serve(head_over(writer, Vec::new(), leadership)).await;
    let mut client = serving.client().await;

    let error = client
        .insert(app(insert_one(1)))
        .await
        .expect_err("a follower must not accept a write");

    assert_eq!(
        error.code(),
        Code::Unavailable,
        "the client should retry elsewhere, so UNAVAILABLE rather than FAILED_PRECONDITION"
    );
    let leader = error
        .metadata()
        .get(slate_server::LEADER_KEY)
        .expect("the refusal should say who to ask instead");
    assert_eq!(leader.to_str().unwrap(), "the-other-node");
}

#[tokio::test]
async fn a_follower_still_serves_reads() {
    // The point of the read/write split. A node that is not the writer is still
    // a perfectly good reader, and a deployment that took it out of rotation
    // would be losing capacity for no reason.
    let backing = Arc::new(MemoryStore::new());
    seed(&backing, 3).await;

    let writer = Fenceable::over(Arc::clone(&backing));
    let leadership = Leadership::new(Arc::new(HeldByAnother));
    leadership.campaign().await;

    let replica: Arc<dyn KvReadStore> = Arc::new(Replica(Arc::clone(&backing)));
    let serving = common::serve(head_over(writer, vec![replica], leadership)).await;
    let mut client = serving.client().await;

    let stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(docs_query()),
            freshness: None,
        }))
        .await
        .expect("a follower serves reads")
        .into_inner();
    let (rows, served_by) = drain(stream).await;

    assert_eq!(rows.len(), 3);
    assert_eq!(
        served_by.expect("served_by").replica,
        "the-replica",
        "a read should have gone to the replica, not the writer"
    );
}

// --- being fenced ---------------------------------------------------------

#[tokio::test]
async fn being_fenced_stops_the_node_touching_the_store() {
    let backing = Arc::new(MemoryStore::new());
    let writer = Fenceable::over(Arc::clone(&backing));
    let lease = Arc::new(CountingLease::default());
    let leadership = Leadership::new(Arc::clone(&lease) as Arc<dyn Lease>);
    assert!(leadership.campaign().await);

    let serving = common::serve(head_over(Arc::clone(&writer), Vec::new(), leadership)).await;
    let mut client = serving.client().await;

    client.insert(app(insert_one(1))).await.expect("premise");

    // Another writer takes over.
    writer.fence();

    let error = client
        .insert(app(insert_one(2)))
        .await
        .expect_err("a fenced writer cannot commit");
    assert_eq!(error.code(), Code::Unavailable);

    let after_first_refusal = writer.opens();
    for id in 3..8 {
        let error = client.insert(app(insert_one(id))).await.expect_err("still");
        assert_eq!(error.code(), Code::Unavailable);
    }

    // The refusal is local from here on. Without this the node would be making
    // a round trip into a store that will never answer again, once per request,
    // for as long as it is up.
    assert_eq!(
        writer.opens(),
        after_first_refusal,
        "five more writes reached the store after the node had already been fenced"
    );

    let status = client
        .leadership(app(pb::LeadershipRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        status.standing,
        pb::leadership_status::Standing::SteppedDown as i32
    );
    assert_eq!(status.stepped_down_because, StepDown::Fenced.reason());
}

#[tokio::test]
async fn a_fenced_node_keeps_serving_reads_from_a_replica() {
    // This is the whole reason a fenced head node is not simply shut down, and
    // the reason reads never go to the writer in the first place: a takeover is
    // an interruption, not a drain, so only the replica path survives it.
    let backing = Arc::new(MemoryStore::new());
    seed(&backing, 4).await;

    let writer = Fenceable::over(Arc::clone(&backing));
    let leadership = Leadership::new(Arc::new(CountingLease::default()));
    assert!(leadership.campaign().await);

    let replica: Arc<dyn KvReadStore> = Arc::new(Replica(Arc::clone(&backing)));
    let serving = common::serve(head_over(Arc::clone(&writer), vec![replica], leadership)).await;
    let mut client = serving.client().await;

    writer.fence();
    assert!(client.insert(app(insert_one(99))).await.is_err(), "premise");

    let stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(docs_query()),
            freshness: None,
        }))
        .await
        .expect("a fenced node still reads")
        .into_inner();
    let (rows, served_by) = drain(stream).await;

    assert_eq!(rows.len(), 4, "reads stopped when the writer was fenced");
    assert_eq!(served_by.expect("served_by").replica, "the-replica");
}

#[tokio::test]
async fn campaigning_after_a_fence_never_touches_the_lease_again() {
    // Being fenced is terminal for the process. A node that campaigned again
    // would win the lease, open a store that is permanently fenced, and serve
    // nothing while looking healthy.
    let lease = Arc::new(CountingLease::default());
    let leadership = Leadership::new(Arc::clone(&lease) as Arc<dyn Lease>);

    assert!(leadership.campaign().await);
    let before = lease.acquires.load(Ordering::SeqCst);

    leadership.fenced().await;
    for _ in 0..5 {
        assert!(!leadership.campaign().await, "a fenced node stays down");
    }

    assert_eq!(
        lease.acquires.load(Ordering::SeqCst),
        before,
        "a fenced node went back to the lease"
    );
    assert!(matches!(
        leadership.standing(),
        Standing::SteppedDown {
            reason: StepDown::Fenced
        }
    ));
}

#[tokio::test]
async fn stepping_down_releases_the_lease_exactly_once() {
    let lease = Arc::new(CountingLease::default());
    let leadership = Leadership::new(Arc::clone(&lease) as Arc<dyn Lease>);
    leadership.campaign().await;

    leadership.fenced().await;
    leadership.fenced().await;
    leadership.resign().await;

    assert_eq!(
        lease.releases.load(Ordering::SeqCst),
        1,
        "a repeated step-down should not keep writing to the lease"
    );
}

#[tokio::test]
async fn a_fence_hands_the_lease_over_without_waiting_out_the_term() {
    // The operational payoff of releasing on step-down: the node that fenced us
    // does not have to wait a whole term to be recognised as the leader.
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let clock = TestClock::new();
    let term = Duration::from_secs(30);

    let ours = Arc::new(lease_at(
        Arc::clone(&object_store),
        "node-a",
        Arc::clone(&clock),
        term,
    ));
    let theirs = lease_at(object_store, "node-b", clock, term);

    let leadership = Leadership::new(ours);
    assert!(leadership.campaign().await);
    assert!(
        theirs.acquire().await.is_err(),
        "premise: the lease is live while we hold it"
    );

    leadership.fenced().await;

    let taken = theirs
        .acquire()
        .await
        .expect("the successor should not have to wait out the term");
    assert_eq!(taken.holder, "node-b");
    assert_eq!(taken.generation, 2);
}

// --- renewing -------------------------------------------------------------

#[tokio::test]
async fn a_storage_error_on_renewal_does_not_hand_over_the_database() {
    // A failed renewal on a healthy lease is a transient storage error, and the
    // term has two thirds of its life left by construction. Standing down here
    // would hand the database over every time the object store hiccuped.
    let leadership = Leadership::new(Arc::new(RenewalKeepsFailing));
    assert!(leadership.campaign().await);

    for _ in 0..5 {
        assert!(leadership.renew().await, "still the leader");
    }
    assert!(leadership.is_leader());
}

#[tokio::test]
async fn finding_the_lease_in_someone_elses_hands_steps_down() {
    // Weaker evidence than a fence — no write has failed yet — but a node whose
    // lease has moved on is one write away from finding out the hard way.
    let leadership = Leadership::new(Arc::new(TakenFromUnderUs));
    assert!(leadership.campaign().await);
    assert!(!leadership.renew().await);
    assert!(matches!(
        leadership.standing(),
        Standing::SteppedDown {
            reason: StepDown::LeaseLost
        }
    ));
}

// --- helpers --------------------------------------------------------------

async fn seed(backing: &Arc<MemoryStore>, count: u64) {
    let store = common::store(Arc::clone(backing));
    let context = slate_kernel::SecurityContext::superuser();
    let table = common::docs();
    let rows: Vec<_> = (0..count).map(|id| doc(id, "kind-a", 10, None)).collect();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&context, &table, &rows).await.unwrap();
    txn.commit().await.unwrap();
}
