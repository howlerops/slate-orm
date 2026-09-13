//! A head node over a real SlateDB, replaced by another one.
//!
//! Everything else in this crate fences with a fake. This does it for real: two
//! `SlateStore`s over one object store, which is exactly what a second head
//! node starting up amounts to. It is the only test here that exercises the
//! thing `topology.md` says is missing from the library — nothing elects, so
//! two writers would take turns fencing each other — against the storage engine
//! that actually does the fencing.
//!
//! Three claims, in the order they matter:
//!
//! 1. The node that is fenced stops accepting writes and says so as
//!    `UNAVAILABLE`, so a client retries somewhere else.
//! 2. It carries on answering reads, from the replica. This is the whole reason
//!    reads never go to the writer, and the control is the read that *does* ask
//!    for the writer: it fails, which is what proves the replica is carrying
//!    the other one.
//! 3. It gives up the lease, so its replacement does not have to wait out a
//!    term before it is allowed to write.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, catalog, doc, docs_query, drain, security};
use slate_kernel::KvReadStore;
use slate_server::convert::row_to_proto;
use slate_server::leadership::{Leadership, StepDown};
use slate_server::lease::{Lease, ObjectStoreLease};
use slate_server::proto as pb;
use slate_server::{Head, HeadConfig, MetadataIdentity};
use slate_slatedb::{ReplicaMode, SlateReader, SlateStore};
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;
use std::time::Duration;
use tonic::Code;

const PATH: &str = "/records";
const LEASE: &str = "leases/writer";

/// Long enough that nothing in this test expires by accident: the point is the
/// fence, not the clock.
const TERM: Duration = Duration::from_secs(300);

fn insert(id: u64) -> pb::InsertRequest {
    pb::InsertRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        rows: vec![row_to_proto(&doc(id, "kind-a", 10, None))],
        upsert: false,
        schema: Some(common::claim("docs")),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_fence_steps_the_head_node_down_and_hands_over_its_lease() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());

    // Node A: the incumbent.
    let writer = Arc::new(
        SlateStore::open(PATH, Arc::clone(&object_store))
            .await
            .expect("open the writer"),
    );
    let replica: Arc<dyn KvReadStore> = Arc::new(
        SlateReader::open(
            "replica-0",
            PATH,
            Arc::clone(&object_store),
            ReplicaMode::Following,
        )
        .await
        .expect("open a replica"),
    );

    let lease = Arc::new(
        ObjectStoreLease::with_holder(Arc::clone(&object_store), LEASE, "node-a".to_owned())
            .with_term_length(TERM),
    );
    let leadership = Leadership::new(Arc::clone(&lease) as Arc<dyn Lease>);
    assert!(leadership.campaign().await, "the lease starts free");

    let serving = common::serve(Head::new(
        HeadConfig::new(catalog(), security()),
        Arc::clone(&writer),
        vec![Arc::clone(&replica)],
        Arc::clone(&leadership),
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    ))
    .await;
    let mut client = serving.client().await;

    client
        .insert(app(insert(1)))
        .await
        .expect("the leader can write");

    // Premises, so the assertions after the takeover mean something: both read
    // paths work while node A owns the database.
    let (_, before_replica) = drain(
        client
            .query(app(query(None)))
            .await
            .expect("a read through the pool")
            .into_inner(),
    )
    .await;
    assert_eq!(before_replica.expect("served_by").replica, "replica-0");
    client
        .query(app(query(Some(latest()))))
        .await
        .expect("premise: a read on the writer works while it owns the database");

    // Node B starts. Opening a second `SlateStore` over the same path is what a
    // replacement head node does, and it fences the first.
    let successor = SlateStore::open(PATH, Arc::clone(&object_store))
        .await
        .expect("open the successor");

    // 1. The fenced node refuses writes, and tells the client to go elsewhere.
    let refused = client
        .insert(app(insert(2)))
        .await
        .expect_err("a fenced writer cannot commit");
    assert_eq!(refused.code(), Code::Unavailable, "{refused:?}");

    let standing = client
        .leadership(app(pb::LeadershipRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        standing.standing,
        pb::leadership_status::Standing::SteppedDown as i32,
        "the node did not notice it had been fenced"
    );
    assert_eq!(standing.stepped_down_because, StepDown::Fenced.reason());

    // 2. It still answers reads, and the control says why: the writer path is
    //    genuinely dead, so the replica is what is carrying them.
    let writer_read = client
        .query(app(query(Some(latest()))))
        .await
        .expect_err("the control: a read on the fenced writer must not work");
    assert_eq!(writer_read.code(), Code::Unavailable, "{writer_read:?}");

    let (_, served_by) = drain(
        client
            .query(app(query(None)))
            .await
            .expect("a fenced node still serves reads")
            .into_inner(),
    )
    .await;
    assert_eq!(
        served_by.expect("served_by").replica,
        "replica-0",
        "reads stopped going to the replica once the writer was fenced"
    );

    // 3. The lease is free, so node B can be recognised as the writer at once
    //    rather than after a term.
    let theirs =
        ObjectStoreLease::with_holder(Arc::clone(&object_store), LEASE, "node-b".to_owned())
            .with_term_length(TERM);
    let taken = theirs
        .acquire()
        .await
        .expect("the successor should not have to wait out node A's term");
    assert_eq!(taken.holder, "node-b");
    assert!(
        taken.generation > 1,
        "the generation must advance across a handover, was {}",
        taken.generation
    );

    let _ = successor.close().await;
}

/// A transaction opened before the takeover fails at commit, and the node steps
/// down on it.
///
/// The case a writer cannot defend against on its own: the transaction was
/// legitimate when it started, and the client had every reason to believe it
/// would land. What the head node owes it is a clear answer and a state change,
/// not a silent failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transaction_opened_before_the_takeover_is_refused_at_commit() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let writer = Arc::new(
        SlateStore::open(PATH, Arc::clone(&object_store))
            .await
            .unwrap(),
    );
    let replica: Arc<dyn KvReadStore> = Arc::new(
        SlateReader::open(
            "replica-0",
            PATH,
            Arc::clone(&object_store),
            ReplicaMode::Following,
        )
        .await
        .unwrap(),
    );

    let lease = Arc::new(
        ObjectStoreLease::with_holder(Arc::clone(&object_store), LEASE, "node-a".to_owned())
            .with_term_length(TERM),
    );
    let leadership = Leadership::new(lease);
    assert!(leadership.campaign().await);

    let serving = common::serve(Head::new(
        HeadConfig::new(catalog(), security()),
        Arc::clone(&writer),
        vec![replica],
        Arc::clone(&leadership),
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    ))
    .await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .unwrap()
        .into_inner()
        .transaction;
    client
        .insert(app(pb::InsertRequest {
            transaction: transaction.clone(),
            ..insert(1)
        }))
        .await
        .expect("buffering a write does not touch the object store");

    let successor = SlateStore::open(PATH, Arc::clone(&object_store))
        .await
        .unwrap();

    let error = client
        .commit(app(pb::CommitRequest { transaction }))
        .await
        .expect_err("the transaction was fenced");
    assert_eq!(error.code(), Code::Unavailable, "{error:?}");
    assert!(
        leadership.standing().has_stepped_down(),
        "a fence discovered at commit should step the node down too"
    );

    let _ = successor.close().await;
}

/// A second node comes up, serves reads, and does **not** fence the first.
///
/// This is the test the previous design could not have. `Head::new` took a
/// writer store, so a node that lost the campaign had to open a `SlateStore` to
/// build a head at all — and opening one is the fence. The only two outcomes
/// available were "kill the healthy leader" and "refuse to start", and
/// `slate-serverd` picked the second, which contradicted this crate's own
/// documentation about a node that is not the writer carrying on with reads.
///
/// So the assertion that matters is not that the follower reads. It is the one
/// after it: the leader **writes again**, afterwards. A test that only checked
/// reads would pass against the broken design, because the fenced leader's
/// replica would go on answering perfectly well.
///
/// A read is not evidence either — the test above is the proof, a fenced
/// SlateDB can still be read from — which is why the leader's second act here
/// is an insert.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_only_head_serves_reads_and_does_not_fence_the_leader() {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());

    // --- node A: opens the database, wins the lease, serves everything ------
    let writer = Arc::new(
        SlateStore::open(PATH, Arc::clone(&object_store))
            .await
            .expect("open the writer"),
    );
    let leaders_lease = Arc::new(
        ObjectStoreLease::with_holder(Arc::clone(&object_store), LEASE, "node-a".to_owned())
            .with_term_length(TERM),
    );
    let leading = Leadership::new(leaders_lease as Arc<dyn Lease>);
    assert!(leading.campaign().await, "the lease starts free");

    let node_a = common::serve(Head::new(
        HeadConfig::new(catalog(), security()),
        Arc::clone(&writer),
        Vec::new(),
        Arc::clone(&leading),
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    ))
    .await;
    let mut a = node_a.client().await;
    let landed = a
        .insert(app(insert(1)))
        .await
        .expect("the leader can write")
        .into_inner()
        .sequence
        .expect("a single-statement write commits, so it has a sequence");

    // --- node B: loses the campaign, and opens no writer at all -------------
    let followers_lease = Arc::new(
        ObjectStoreLease::with_holder(Arc::clone(&object_store), LEASE, "node-b".to_owned())
            .with_term_length(TERM),
    );
    let following = Leadership::new(followers_lease as Arc<dyn Lease>);
    assert!(
        !following.campaign().await,
        "node A holds the lease, so node B must lose"
    );

    // The line that used to be impossible. A `SlateReader` claims nothing; a
    // `SlateStore` here would be the fence.
    let replica: Arc<dyn KvReadStore> = Arc::new(
        SlateReader::open(
            "replica-b",
            PATH,
            Arc::clone(&object_store),
            ReplicaMode::Following,
        )
        .await
        .expect("open a replica"),
    );
    let node_b = common::serve(Head::<SlateStore>::read_only(
        HeadConfig::new(catalog(), security()),
        vec![replica],
        Arc::clone(&following),
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    ))
    .await;
    let mut b = node_b.client().await;

    // 1. It reads, and it reads the leader's data. `at_least` is the row's own
    //    commit sequence, so this is read-your-writes across two nodes rather
    //    than a sleep that is either flaky or slow.
    let (rows, served_by) = drain(
        b.query(app(query(Some(pb::Freshness {
            level: Some(pb::freshness::Level::AtLeast(landed)),
        }))))
        .await
        .expect("a node with no writer must still serve reads")
        .into_inner(),
    )
    .await;
    assert_eq!(common::doc_ids(&rows), vec![1]);
    assert_eq!(
        served_by.expect("served_by").replica,
        "replica-b",
        "the read must have come from this node's own replica"
    );

    // 2. It refuses writes, and says where to send them.
    let refused = b
        .insert(app(insert(2)))
        .await
        .expect_err("a node with no writer must refuse writes");
    assert_eq!(refused.code(), Code::Unavailable, "{refused:?}");
    assert_eq!(
        refused
            .metadata()
            .get(slate_server::LEADER_KEY)
            .expect("the refusal should name the leader")
            .to_str()
            .unwrap(),
        "node-a"
    );

    // 3. The control that separates "no writer" from "a writer it will not
    //    use": `Latest` can only be served by the writer, and this pool has
    //    none, so it is refused rather than quietly served by the replica.
    let latest_read = b
        .query(app(query(Some(latest()))))
        .await
        .expect_err("a pool with no writer cannot serve `Latest`");
    assert_eq!(latest_read.code(), Code::Unavailable, "{latest_read:?}");

    // 4. **The assertion this test exists for.** Node A is still the writer.
    //    Against the design this replaces, node B could not have been built
    //    without opening a second `SlateStore`, and this insert would fail as
    //    `UNAVAILABLE` with node A stepped down.
    a.insert(app(insert(3)))
        .await
        .expect("node B must not have fenced the leader");
    assert!(
        leading.standing().is_leader(),
        "the leader stopped being the leader, which means something fenced it"
    );

    let _ = writer.close().await;
}

fn query(freshness: Option<pb::Freshness>) -> pb::QueryRequest {
    pb::QueryRequest {
        transaction: String::new(),
        query: Some(docs_query()),
        freshness,
    }
}

fn latest() -> pb::Freshness {
    pb::Freshness {
        level: Some(pb::freshness::Level::Latest(pb::Unit::Unit as i32)),
    }
}
