//! The lease: what it promises, and proof the promise is testable.
//!
//! A lease is easy to write and hard to test, because the interesting states
//! are the ones two processes reach at the same moment. Two things make them
//! reachable here.
//!
//! **The clock is injected.** Every property worth having is about expiry, and
//! a test that establishes them by sleeping is slow when it passes and flaky
//! when the machine is loaded. With a clock the test can stand exactly one
//! millisecond either side of a term ending.
//!
//! **The property runs against a second, deliberately wrong implementation.**
//! `OverwritingLease` does what a lease looks like it should do — read the
//! object, decide, write the object — and it passes every single-threaded test
//! anyone would think to write. The shared harness below is the one that tells
//! them apart, and the test asserting that the wrong one *fails* it is the
//! reason to believe the harness proves anything about the right one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use async_trait::async_trait;
use common::{TestClock, lease_at};
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt as _, path::Path};
use slate_server::lease::{Lease, LeaseError, Term};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TERM: Duration = Duration::from_secs(10);

fn object_store() -> Arc<dyn ObjectStore> {
    Arc::new(InMemory::new())
}

// --- the shared property ---------------------------------------------------

/// The sequence that separates a compare-and-set lease from a read-then-write
/// one, with no concurrency at all.
///
/// It is written as five ordinary steps because the failure it looks for is not
/// a race: it is that a read-then-write implementation has no way to notice
/// that the object changed under it. Making the test deterministic means the
/// difference is a fact rather than a matter of scheduling luck.
///
/// Returns the step that went wrong, so the negative test can say which.
async fn a_replaced_holder_finds_out(
    incumbent: &dyn Lease,
    challenger: &dyn Lease,
    clock: &TestClock,
) -> Result<(), String> {
    let first = incumbent
        .acquire()
        .await
        .map_err(|e| format!("the incumbent could not take a free lease: {e}"))?;

    match challenger.acquire().await {
        Err(LeaseError::Held { .. }) => {}
        Ok(term) => {
            return Err(format!(
                "the challenger took a live lease: generation {} while {} held generation {}",
                term.generation, first.holder, first.generation
            ));
        }
        Err(other) => return Err(format!("the challenger got an odd error: {other}")),
    }

    // The incumbent stops renewing — it crashed, or it was paused — and its
    // term runs out.
    clock.advance(TERM + Duration::from_millis(1));

    let second = challenger
        .acquire()
        .await
        .map_err(|e| format!("the challenger could not take an expired lease: {e}"))?;
    if second.generation <= first.generation {
        return Err(format!(
            "generation did not advance: {} then {}",
            first.generation, second.generation
        ));
    }

    // The incumbent comes back and heartbeats, believing it still holds the
    // lease. This is the moment the whole design turns on.
    match incumbent.renew().await {
        Err(LeaseError::Lost { .. }) => Ok(()),
        Ok(term) => Err(format!(
            "the replaced holder renewed successfully into generation {}: two writers now \
             believe they hold the lease",
            term.generation
        )),
        Err(other) => Err(format!("the incumbent got an odd error: {other}")),
    }
}

#[tokio::test]
async fn the_object_store_lease_tells_a_replaced_holder_it_was_replaced() {
    let store = object_store();
    let clock = TestClock::new();
    let a = lease_at(Arc::clone(&store), "node-a", Arc::clone(&clock), TERM);
    let b = lease_at(store, "node-b", Arc::clone(&clock), TERM);

    if let Err(why) = a_replaced_holder_finds_out(&a, &b, &clock).await {
        panic!("{why}");
    }
}

/// The negative control, and the reason the test above means anything.
///
/// `OverwritingLease` is what a lease looks like before anyone thinks about the
/// gap between the read and the write. It satisfies "only one holder at a time"
/// as long as nothing ever contends, which is every test written by whoever
/// wrote it.
#[tokio::test]
async fn a_read_then_write_lease_does_not_and_that_is_the_point() {
    let store = object_store();
    let clock = TestClock::new();
    let a = OverwritingLease::new(Arc::clone(&store), "node-a", Arc::clone(&clock));
    let b = OverwritingLease::new(store, "node-b", Arc::clone(&clock));

    let outcome = a_replaced_holder_finds_out(&a, &b, &clock).await;
    let why = outcome.expect_err(
        "a read-then-write lease passed the property, which would mean the property \
         does not test the thing it exists to test",
    );
    assert!(
        why.contains("renewed successfully"),
        "it should fail at the renewal, not earlier: {why}"
    );
}

// --- properties of the real one -------------------------------------------

#[tokio::test]
async fn exactly_one_of_eight_racing_clients_wins() {
    // Every client reads no lease and tries to create one. `PutMode::Create` is
    // what makes "first" mean something; without it all eight would think they
    // had it.
    let store = object_store();
    let clock = TestClock::new();
    let clients: Vec<_> = (0..8)
        .map(|n| {
            Arc::new(lease_at(
                Arc::clone(&store),
                &format!("node-{n}"),
                Arc::clone(&clock),
                TERM,
            ))
        })
        .collect();

    let attempts = clients.iter().map(|client| {
        let client = Arc::clone(client);
        tokio::spawn(async move { client.acquire().await.map(|term| term.holder) })
    });
    let mut winners = Vec::new();
    for attempt in attempts {
        if let Ok(Ok(holder)) = attempt.await {
            winners.push(holder);
        }
    }

    assert_eq!(
        winners.len(),
        1,
        "eight clients raced and {} of them believe they hold the lease: {winners:?}",
        winners.len()
    );
}

#[tokio::test]
async fn a_generation_is_never_reused_across_a_chain_of_handovers() {
    // A generation that repeated would make two distinct terms indistinguishable
    // in a log, which is the one thing the number is for.
    let store = object_store();
    let clock = TestClock::new();
    let mut seen = Vec::new();

    for round in 0..4 {
        let client = lease_at(
            Arc::clone(&store),
            &format!("node-{round}"),
            Arc::clone(&clock),
            TERM,
        );
        let term = client.acquire().await.expect("the lease is free by now");
        seen.push(term.generation);
        clock.advance(TERM + Duration::from_millis(1));
    }

    assert_eq!(seen, vec![1, 2, 3, 4], "generations: {seen:?}");
}

#[tokio::test]
async fn renewing_keeps_the_generation_and_moves_the_expiry() {
    // A renewal extends a term; it does not start a new one. If it raised the
    // generation, "which term was that write under" would have no answer.
    let store = object_store();
    let clock = TestClock::new();
    let client = lease_at(store, "node-a", Arc::clone(&clock), TERM);

    let first = client.acquire().await.unwrap();
    clock.advance(TERM / 3);
    let renewed = client.renew().await.unwrap();

    assert_eq!(renewed.generation, first.generation);
    assert!(
        renewed.expires_at > first.expires_at,
        "renewing did not move the expiry: {:?} then {:?}",
        first.expires_at,
        renewed.expires_at
    );
}

#[tokio::test]
async fn a_live_lease_cannot_be_taken_and_an_expired_one_can() {
    let store = object_store();
    let clock = TestClock::new();
    let a = lease_at(Arc::clone(&store), "node-a", Arc::clone(&clock), TERM);
    let b = lease_at(store, "node-b", Arc::clone(&clock), TERM);

    a.acquire().await.unwrap();

    // One millisecond before the end.
    clock.advance(TERM - Duration::from_millis(1));
    match b.acquire().await {
        Err(LeaseError::Held { holder, .. }) => assert_eq!(holder, "node-a"),
        other => panic!("a live lease was taken: {other:?}"),
    }

    // And two milliseconds later, past it.
    clock.advance(Duration::from_millis(2));
    let taken = b.acquire().await.expect("an expired lease is available");
    assert_eq!(taken.holder, "node-b");
}

#[tokio::test]
async fn releasing_lets_a_successor_take_over_without_waiting_out_the_term() {
    // The whole reason to release: a planned handover should not cost a term of
    // downtime.
    let store = object_store();
    let clock = TestClock::new();
    let a = lease_at(Arc::clone(&store), "node-a", Arc::clone(&clock), TERM);
    let b = lease_at(store, "node-b", Arc::clone(&clock), TERM);

    a.acquire().await.unwrap();
    assert!(
        b.acquire().await.is_err(),
        "premise: the lease is live before the release"
    );

    a.release().await.expect("release");
    let taken = b
        .acquire()
        .await
        .expect("a released lease is free immediately");
    assert_eq!(taken.holder, "node-b");
}

#[tokio::test]
async fn a_late_release_cannot_remove_a_successors_lease() {
    // This is why release writes an expired term instead of deleting the
    // object: `object_store` has no conditional delete, so an unconditional one
    // would take out whoever holds the lease now. A node slow enough to be
    // replaced before its own release lands is exactly the node calling it.
    let store = object_store();
    let clock = TestClock::new();
    let a = lease_at(Arc::clone(&store), "node-a", Arc::clone(&clock), TERM);
    let b = lease_at(Arc::clone(&store), "node-b", Arc::clone(&clock), TERM);

    a.acquire().await.unwrap();
    clock.advance(TERM + Duration::from_millis(1));
    let successor = b.acquire().await.unwrap();

    // Now the old node finally gets round to standing down.
    a.release().await.expect("a late release is not an error");

    let observed = b
        .observe()
        .await
        .unwrap()
        .expect("the successor's lease must still be there");
    assert_eq!(observed.holder, "node-b");
    assert_eq!(observed.generation, successor.generation);
    assert!(
        b.renew().await.is_ok(),
        "the successor lost its lease to a predecessor's release"
    );
}

#[tokio::test]
async fn a_lease_object_that_is_not_a_lease_is_reported_not_overwritten() {
    // The path is operator-configured. Quietly stamping over an unrecognised
    // object there is how a typo becomes a data-loss incident.
    let store = object_store();
    let path = Path::from("leases/writer");
    store
        .put(&path, "this is somebody else's file".into())
        .await
        .unwrap();

    let clock = TestClock::new();
    let client = lease_at(Arc::clone(&store), "node-a", clock, TERM);

    match client.acquire().await {
        Err(LeaseError::Malformed { .. }) => {}
        other => panic!("expected a Malformed error, got {other:?}"),
    }

    let after = store.get(&path).await.unwrap().bytes().await.unwrap();
    assert_eq!(
        after.as_ref(),
        b"this is somebody else's file",
        "the lease overwrote an object it did not understand"
    );
}

#[tokio::test]
async fn nothing_is_held_before_anything_is_acquired() {
    let client = lease_at(object_store(), "node-a", TestClock::new(), TERM);
    assert!(client.observe().await.unwrap().is_none());
    assert!(client.held().is_none());
    assert!(matches!(client.renew().await, Err(LeaseError::NotHeld)));
    assert!(matches!(client.release().await, Err(LeaseError::NotHeld)));
}

// --- the deliberately wrong implementation --------------------------------

/// A lease that reads, decides, and writes — with no condition on the write.
///
/// Kept as small as it can be while still being a plausible thing to write. It
/// exists only to fail [`a_replaced_holder_finds_out`], and it is in the test
/// file rather than the crate so that nobody can reach for it by accident.
#[derive(Debug)]
struct OverwritingLease {
    store: Arc<dyn ObjectStore>,
    path: Path,
    holder: String,
    clock: Arc<TestClock>,
    held: Mutex<Option<Term>>,
}

impl OverwritingLease {
    fn new(store: Arc<dyn ObjectStore>, holder: &str, clock: Arc<TestClock>) -> Self {
        Self {
            store,
            // Its own path, so it cannot be confused with the real one's object.
            path: Path::from("leases/naive"),
            holder: holder.to_owned(),
            clock,
            held: Mutex::new(None),
        }
    }

    async fn read(&self) -> Option<Term> {
        let bytes = self.store.get(&self.path).await.ok()?.bytes().await.ok()?;
        let text = String::from_utf8(bytes.to_vec()).ok()?;
        let mut parts = text.split('|');
        Some(Term {
            generation: parts.next()?.parse().ok()?,
            holder: parts.next()?.to_owned(),
            expires_at: UNIX_EPOCH + Duration::from_millis(parts.next()?.parse().ok()?),
        })
    }

    async fn write(&self, term: &Term) {
        let millis = term
            .expires_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let body = format!("{}|{}|{millis}", term.generation, term.holder);
        self.store.put(&self.path, body.into()).await.unwrap();
    }

    fn now(&self) -> SystemTime {
        <TestClock as slate_server::lease::Clock>::now(&self.clock)
    }
}

#[async_trait]
impl Lease for OverwritingLease {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        let now = self.now();
        let current = self.read().await;
        if let Some(current) = &current
            && current.holder != self.holder
            && current.expires_at > now
        {
            return Err(LeaseError::Held {
                holder: current.holder.clone(),
                expires_at: current.expires_at,
            });
        }
        let term = Term {
            generation: current.map_or(0, |c| c.generation) + 1,
            holder: self.holder.clone(),
            expires_at: now + TERM,
        };
        self.write(&term).await;
        *self.held.lock().unwrap() = Some(term.clone());
        Ok(term)
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        let Some(current) = self.held.lock().unwrap().clone() else {
            return Err(LeaseError::NotHeld);
        };
        let term = Term {
            generation: current.generation,
            holder: self.holder.clone(),
            expires_at: self.now() + TERM,
        };
        self.write(&term).await;
        *self.held.lock().unwrap() = Some(term.clone());
        Ok(term)
    }

    async fn release(&self) -> Result<(), LeaseError> {
        if self.held.lock().unwrap().take().is_none() {
            return Err(LeaseError::NotHeld);
        }
        let _ = self.store.delete(&self.path).await;
        Ok(())
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(self.read().await)
    }

    fn held(&self) -> Option<Term> {
        self.held.lock().unwrap().clone()
    }

    fn holder(&self) -> &str {
        &self.holder
    }
}
