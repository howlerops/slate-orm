//! Where a read goes, and whether it is allowed to be served there.
//!
//! # Read-your-writes, and the control that makes it mean something
//!
//! `a_commit_token_is_what_makes_a_write_readable` runs the *same query twice*
//! against the same node: once asking for any replica, once carrying the
//! sequence the commit returned. The first sees nothing, because the replica in
//! the pool is genuinely behind. The second sees the row.
//!
//! The first half is the control. A test that only asserted the second half
//! would pass just as happily against a pool with no lag at all, where the
//! token does nothing and nobody would find out until a replica fell behind in
//! production. Requiring the stale answer to *be* stale is what says the token
//! is carrying the weight.
//!
//! # Saying where a read went
//!
//! `served_by` is only worth carrying if it is true, and the tests that pin it
//! use replicas with *different contents* on purpose. Four mirrors of one
//! backing cannot tell a truthful name from a plausible one: with the same rows
//! everywhere, a name read off a second routing decision — the round-robin
//! counter having moved on in between — looks exactly like a name read off the
//! first. Rows that say which replica wrote them are what makes the difference
//! visible.
//!
//! # Affinity
//!
//! The kernel tests rendezvous placement. What is tested here is the head
//! node's part: which value it routes on. It uses the *principal's* tenant, not
//! one dug out of the filter, because on a tenant-scoped table the security
//! layer forces the principal's tenant onto the predicate anyway — so it is the
//! tenant whose key range the read will actually touch. A tenant taken from the
//! filter could name a range the caller is not allowed to reach.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use async_trait::async_trait;
use common::{app, app_in, catalog, doc, docs_query, drain, security, user};
use slate_kernel::error::Result as KernelResult;
use slate_kernel::memory::MemoryStore;
use slate_kernel::store::KvSnapshot;
use slate_kernel::{KernelError, KvReadStore};
use slate_server::convert::{row_from_proto, row_to_proto, value_to_proto};
use slate_server::leadership::Leadership;
use slate_server::proto as pb;
use slate_server::{Head, HeadConfig, MetadataIdentity};
use slate_tuple::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tonic::Code;

/// A replica that is empty and will never catch up.
///
/// Extreme on purpose. A replica one write behind and a replica forty behind
/// fail a freshness check identically, and the empty one makes the difference
/// between the two answers unmistakable in the assertion.
#[derive(Debug)]
struct Frozen {
    name: &'static str,
    empty: MemoryStore,
}

impl Frozen {
    fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            empty: MemoryStore::new(),
        })
    }
}

#[async_trait]
impl KvReadStore for Frozen {
    async fn snapshot(&self) -> KernelResult<Box<dyn KvSnapshot + Send + '_>> {
        self.empty.snapshot().await
    }

    fn visible_sequence(&self) -> Option<u64> {
        Some(0)
    }

    async fn wait_for_sequence(&self, sequence: u64, _timeout: Duration) -> KernelResult<()> {
        Err(KernelError::ReplicaTooStale {
            replica: self.name.to_owned(),
            required: sequence,
            visible: 0,
        })
    }

    fn replica_name(&self) -> &str {
        self.name
    }
}

/// A replica that has everything the writer has, under a name of its own.
#[derive(Debug)]
struct Mirror {
    name: String,
    backing: Arc<MemoryStore>,
}

impl Mirror {
    fn new(name: &str, backing: Arc<MemoryStore>) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            backing,
        })
    }
}

#[async_trait]
impl KvReadStore for Mirror {
    async fn snapshot(&self) -> KernelResult<Box<dyn KvSnapshot + Send + '_>> {
        self.backing.snapshot().await
    }

    fn visible_sequence(&self) -> Option<u64> {
        self.backing.visible_sequence()
    }

    fn replica_name(&self) -> &str {
        &self.name
    }
}

async fn leader_with(
    writer: Arc<MemoryStore>,
    replicas: Vec<Arc<dyn KvReadStore>>,
) -> common::Serving {
    let leadership = Leadership::new(Arc::new(common::AlwaysLeader::default()));
    assert!(leadership.campaign().await);
    let head = Head::new(
        HeadConfig::new(catalog(), security()),
        writer,
        replicas,
        leadership,
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    );
    common::serve(head).await
}

fn query(freshness: Option<pb::Freshness>) -> pb::QueryRequest {
    pb::QueryRequest {
        transaction: String::new(),
        query: Some(docs_query()),
        freshness,
    }
}

#[tokio::test]
async fn a_commit_token_is_what_makes_a_write_readable() {
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(Arc::clone(&writer), vec![Frozen::new("stale-replica")]).await;
    let mut client = serving.client().await;

    let sequence = client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(1, "kind-a", 5, None))],
            upsert: false,
        }))
        .await
        .unwrap()
        .into_inner()
        .sequence
        .expect("a committed write returns its sequence");

    // The control. The replica really is behind, so the same query without a
    // token really does miss the write.
    let (stale, stale_from) =
        drain(client.query(app(query(None))).await.unwrap().into_inner()).await;
    assert!(
        stale.is_empty(),
        "the frozen replica returned rows, so this test proves nothing about tokens"
    );
    assert_eq!(stale_from.expect("served_by").replica, "stale-replica");

    // And with the token, the pool refuses to serve it there.
    let (fresh, fresh_from) = drain(
        client
            .query(app(query(Some(pb::Freshness {
                level: Some(pb::freshness::Level::AtLeast(sequence)),
            }))))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert_eq!(fresh.len(), 1, "read-your-writes did not hold");
    assert_ne!(
        fresh_from.expect("served_by").replica,
        "stale-replica",
        "a replica that cannot reach the sequence served the read anyway"
    );
}

#[tokio::test]
async fn asking_for_the_latest_goes_to_the_writer() {
    // The only view that can see a write before it is flushed. Nothing else can
    // serve it, and the pool says so rather than substituting a replica.
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(Arc::clone(&writer), vec![Frozen::new("stale-replica")]).await;
    let mut client = serving.client().await;

    client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(1, "kind-a", 5, None))],
            upsert: false,
        }))
        .await
        .unwrap();

    let (rows, served_by) = drain(
        client
            .query(app(query(Some(pb::Freshness {
                level: Some(pb::freshness::Level::Latest(true)),
            }))))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;

    assert_eq!(rows.len(), 1);
    assert_ne!(served_by.expect("served_by").replica, "stale-replica");
}

#[tokio::test]
async fn a_point_read_honours_its_token_too() {
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(Arc::clone(&writer), vec![Frozen::new("stale-replica")]).await;
    let mut client = serving.client().await;

    let sequence = client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(4, "kind-a", 5, None))],
            upsert: false,
        }))
        .await
        .unwrap()
        .into_inner()
        .sequence
        .unwrap();

    let get = |freshness: Option<pb::Freshness>| pb::GetRequest {
        transaction: String::new(),
        table: "docs".to_owned(),
        primary_key: Some(pb::Row {
            values: vec![value_to_proto(&Value::U64(4))],
        }),
        freshness,
    };

    let stale = client.get(app(get(None))).await.unwrap().into_inner();
    assert!(!stale.found, "the control: the replica has not caught up");

    let fresh = client
        .get(app(get(Some(pb::Freshness {
            level: Some(pb::freshness::Level::AtLeast(sequence)),
        }))))
        .await
        .unwrap()
        .into_inner();
    assert!(
        fresh.found,
        "read-your-writes did not hold for a point read"
    );
}

#[tokio::test]
async fn one_tenants_reads_always_land_on_one_replica() {
    // The payoff of tenant affinity is cache locality, which only exists if the
    // placement is stable. The control below is the round-robin case: without
    // it, "always the same replica" would also be satisfied by a pool that sent
    // everything to one replica for every caller.
    let writer = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&writer));
        let context = slate_kernel::SecurityContext::superuser();
        let table = common::users();
        let txn = store.begin().await.unwrap();
        for tenant in 1..=6_u64 {
            txn.insert(
                &context,
                &table,
                &user(tenant, 1, 7, &format!("t{tenant}@x")),
            )
            .await
            .unwrap();
        }
        txn.commit().await.unwrap();
    }

    let replicas: Vec<Arc<dyn KvReadStore>> = (0..4)
        .map(|n| Mirror::new(&format!("replica-{n}"), Arc::clone(&writer)) as Arc<dyn KvReadStore>)
        .collect();
    let serving = leader_with(Arc::clone(&writer), replicas).await;
    let mut client = serving.client().await;

    let users_query = |tenant: u64| {
        app_in(
            pb::QueryRequest {
                transaction: String::new(),
                query: Some(pb::Query {
                    table: "users".to_owned(),
                    ..docs_query()
                }),
                freshness: None,
            },
            7,
            tenant,
        )
    };

    let mut per_tenant = Vec::new();
    for tenant in 1..=6_u64 {
        let mut seen = BTreeSet::new();
        for _ in 0..5 {
            let (_, served_by) = drain(
                client
                    .query(users_query(tenant))
                    .await
                    .unwrap()
                    .into_inner(),
            )
            .await;
            seen.insert(served_by.expect("served_by").replica);
        }
        assert_eq!(
            seen.len(),
            1,
            "tenant {tenant} was spread over {seen:?}, so its range is cached nowhere"
        );
        per_tenant.extend(seen);
    }

    let distinct: BTreeSet<_> = per_tenant.into_iter().collect();
    assert!(
        distinct.len() > 1,
        "every tenant landed on {distinct:?}; that is stable but it is not placement"
    );
}

#[tokio::test]
async fn a_read_with_no_tenant_to_key_on_spreads() {
    // The control for the test above. `docs` is not tenant-scoped, so there is
    // no locality to preserve and the pool falls back to round-robin.
    let writer = Arc::new(MemoryStore::new());
    let replicas: Vec<Arc<dyn KvReadStore>> = (0..4)
        .map(|n| Mirror::new(&format!("replica-{n}"), Arc::clone(&writer)) as Arc<dyn KvReadStore>)
        .collect();
    let serving = leader_with(Arc::clone(&writer), replicas).await;
    let mut client = serving.client().await;

    let mut seen = BTreeSet::new();
    for _ in 0..8 {
        let (_, served_by) =
            drain(client.query(app(query(None))).await.unwrap().into_inner()).await;
        seen.insert(served_by.expect("served_by").replica);
    }

    assert_eq!(
        seen.len(),
        4,
        "eight untenanted reads visited {seen:?}; round-robin should have visited all four"
    );
}

#[tokio::test]
async fn a_write_never_goes_to_a_replica() {
    // Stated as a test because it is the one routing mistake that would be
    // silent: a write served by a replica has nowhere to go and would have to
    // fail, but a write served by the *wrong writer* would not.
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(
        Arc::clone(&writer),
        vec![Mirror::new("replica-0", Arc::new(MemoryStore::new()))],
    )
    .await;
    let mut client = serving.client().await;

    client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(1, "kind-a", 5, None))],
            upsert: false,
        }))
        .await
        .unwrap();

    assert!(
        !writer.is_empty(),
        "the write did not reach this node's writer store"
    );
}

/// A token no view in the pool has reached is refused, the writer included.
///
/// This test is older than the behaviour it now checks, and the gap between
/// the two is the point. Its name always said "refused rather than served
/// stale"; what it actually asserted was only that the *frozen replica* did
/// not serve it, which the writer fallback satisfied by answering at sequence
/// 0. So an impossible freshness came back as rows, and the assertion that was
/// supposed to catch that passed. A Python client asking the same question
/// from outside found it; `at_least = 2^40` against a writer at sequence 2
/// returned every row and no error.
///
/// The writer is normally the most advanced node, which is the whole reason it
/// is the fallback — but a token minted by a *previous* writer names a
/// sequence this one need not have replayed, which is exactly the handover
/// this pool exists to survive.
#[tokio::test]
async fn a_freshness_no_view_can_meet_is_refused_rather_than_served_stale() {
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(Arc::clone(&writer), vec![Frozen::new("stale-replica")]).await;
    let mut client = serving.client().await;

    let refused = client
        .query(app(query(Some(pb::Freshness {
            level: Some(pb::freshness::Level::AtLeast(u64::MAX)),
        }))))
        .await;
    let status = refused.expect_err("an unreachable sequence returned rows");
    assert_eq!(
        status.code(),
        Code::Unavailable,
        "refused, but not as something a later attempt could satisfy: {status:?}"
    );

    // The control, without which the above passes against a node that refuses
    // every read. A sequence the *writer* has reached and the frozen replica
    // has not: write a row first, then ask for the sequence that write
    // produced. `AtLeast(0)` would not do — the replica frozen at zero
    // satisfies it, correctly, and the control would be asserting nothing.
    client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(1, "kind-a", 5, None))],
            upsert: false,
        }))
        .await
        .expect("the write");
    let reached = writer
        .visible_sequence()
        .expect("the writer tracks a sequence");
    assert!(reached > 0, "the write did not advance the writer");

    let (_, served_by) = drain(
        client
            .query(app(query(Some(pb::Freshness {
                level: Some(pb::freshness::Level::AtLeast(reached)),
            }))))
            .await
            .expect("a sequence the writer has reached is servable")
            .into_inner(),
    )
    .await;
    assert_ne!(
        served_by.expect("served_by").replica,
        "stale-replica",
        "a freshness only the writer can meet was served by a replica at zero"
    );
}

#[tokio::test]
async fn a_transactional_read_says_it_came_from_the_writer() {
    // A client metering routing should be able to see that these reads did not
    // go through the pool, because they are the ones a takeover interrupts.
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(
        Arc::clone(&writer),
        vec![Mirror::new("replica-0", writer.clone())],
    )
    .await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .unwrap()
        .into_inner()
        .transaction;

    let (_, served_by) = drain(
        client
            .query(app(pb::QueryRequest {
                transaction: transaction.clone(),
                query: Some(docs_query()),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert_eq!(
        served_by.expect("served_by").replica,
        "writer (in transaction)"
    );

    client
        .rollback(app(pb::RollbackRequest { transaction }))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_unknown_freshness_level_does_not_become_a_request_for_the_writer() {
    // `latest: false` is what a client that zeroed the message sends.
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(Arc::clone(&writer), vec![Frozen::new("stale-replica")]).await;
    let mut client = serving.client().await;

    let (_, served_by) = drain(
        client
            .query(app(query(Some(pb::Freshness {
                level: Some(pb::freshness::Level::Latest(false)),
            }))))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert_eq!(
        served_by.expect("served_by").replica,
        "stale-replica",
        "a zeroed freshness message was read as a demand for the writer"
    );
}

/// Reading with a transaction handle that is not ours must not fall back to a
/// non-transactional read: that would turn an authorisation failure into a
/// successful, subtly different answer.
#[tokio::test]
async fn a_read_naming_an_unknown_transaction_fails_rather_than_falling_back() {
    let writer = Arc::new(MemoryStore::new());
    let serving = leader_with(
        Arc::clone(&writer),
        vec![Mirror::new("replica-0", writer.clone())],
    )
    .await;
    let mut client = serving.client().await;

    let error = client
        .query(app(pb::QueryRequest {
            transaction: "3f2504e0-4f89-41d3-9a0c-0305e82c3301".to_owned(),
            query: Some(docs_query()),
            freshness: None,
        }))
        .await
        .expect_err("an unknown transaction");
    assert_eq!(error.code(), Code::NotFound);
}

/// A replica holding one `docs` row whose `kind` is that replica's own name.
///
/// The point is distinguishability; see the module docs.
async fn signed(name: &str) -> Arc<Mirror> {
    let backing = Arc::new(MemoryStore::new());
    let store = common::store(Arc::clone(&backing));
    let txn = store.begin().await.unwrap();
    txn.insert(
        &slate_kernel::SecurityContext::superuser(),
        &common::docs(),
        &doc(1, name, 1, None),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    Mirror::new(name, backing)
}

/// The `kind` of the single row a response carried: the name of the replica
/// that really answered.
fn who_answered(rows: &[pb::Row]) -> String {
    assert_eq!(rows.len(), 1, "each replica holds exactly one row");
    match &row_from_proto(&rows[0]).unwrap().values()[1] {
        Value::Str(kind) => kind.clone(),
        other => panic!("kind column held {other:?}"),
    }
}

/// The replica a response names must be the one whose rows it returned.
///
/// This is the guarantee the head node buys by routing and opening the view in
/// one call. Reporting it from a second routing decision would name the *next*
/// replica in the rotation, which is a lie that reads as plausible: the right
/// shape, an existing replica, and a number an operator would act on.
#[tokio::test]
async fn a_response_names_the_replica_whose_rows_it_returned() {
    let writer = Arc::new(MemoryStore::new());
    let mut replicas: Vec<Arc<dyn KvReadStore>> = Vec::new();
    for n in 0..4 {
        replicas.push(signed(&format!("replica-{n}")).await);
    }
    let serving = leader_with(Arc::clone(&writer), replicas).await;
    let mut client = serving.client().await;

    // `docs` is not tenant-scoped, so these spread round-robin: the path where
    // asking twice gives two answers.
    let mut seen = BTreeSet::new();
    for _ in 0..12 {
        let (rows, served_by) =
            drain(client.query(app(query(None))).await.unwrap().into_inner()).await;
        let named = served_by.expect("served_by").replica;
        let answered = who_answered(&rows);
        assert_eq!(
            answered, named,
            "a streamed query returned {answered}'s row under the name {named}"
        );
        seen.insert(named);
    }
    assert_eq!(
        seen.len(),
        4,
        "every read landed on {seen:?}; a pool that never moved would satisfy the assertion above without proving anything"
    );

    // The point read takes the other path — no spawned task, no stream — and
    // has the same promise to keep.
    for _ in 0..12 {
        let answer = client
            .get(app(pb::GetRequest {
                transaction: String::new(),
                table: "docs".to_owned(),
                primary_key: Some(pb::Row {
                    values: vec![value_to_proto(&Value::U64(1))],
                }),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner();
        let named = answer.served_by.expect("served_by").replica;
        let row = answer.row.expect("every replica holds the row");
        let answered = who_answered(core::slice::from_ref(&row));
        assert_eq!(
            answered, named,
            "a point read returned {answered}'s row under the name {named}"
        );
    }
}
