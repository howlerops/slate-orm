//! Holding the writer role, and giving it up.
//!
//! [`Lease`] answers "may I write?". This answers the harder operational
//! question that follows: what a head node does with the answer, and what it
//! does when the answer changes underneath it.
//!
//! # Three standings, and only one way out of the last
//!
//! A node is a [`Standing::Follower`], a [`Standing::Leader`], or has
//! [`Standing::SteppedDown`]. The first two alternate. The third is terminal
//! for the process.
//!
//! That asymmetry is deliberate and comes from what `handover.rs` measured: a
//! fenced SlateDB writer cannot commit *or read* — `begin` itself fails — and
//! it never recovers. A node that treated fencing as a bad moment and
//! campaigned again would win the lease, open a store that is permanently
//! fenced, and serve nothing while looking healthy. So being fenced puts the
//! node out of the writer business until it is restarted, which is also when it
//! gets a fresh SlateDB handle.
//!
//! # It keeps serving reads
//!
//! `topology.md` says a head node should shut down on `WriterFenced`. This one
//! does not, and the difference is the read/write split: writes go to the
//! leader's store, reads go through the replica pool, and the pool does not
//! care who the leader is. A node that has stepped down still answers every
//! read it could answer a second earlier. Shutting down would drop those
//! connections for no gain — the interruption `handover.rs` describes is to
//! reads *on the writer*, and there are none once writes are refused.
//!
//! What it must not do is keep accepting writes, and the refusal has to be
//! local rather than a forwarded storage error: after a fence the writer's
//! `begin` fails, so every write would cost a pointless round trip to a store
//! that will never answer. [`Leadership::is_leader`] is checked first.
//!
//! # Believing the fence over the lease
//!
//! [`Leadership::fenced`] steps down even when the lease still looks perfectly
//! valid — which, in the case that matters, it does. A process paused past its
//! expiry comes back with a lease it believes in and a store that has been
//! fenced; the lease is wrong and the fence is right. See the [`crate::lease`]
//! module docs for why this cannot be fixed at the lease.

use crate::lease::{Lease, LeaseError, Term};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Where a node stands with respect to writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Not the leader. Reads are served; writes are refused.
    Follower {
        /// Who the lease said held it, when the last attempt found out.
        leader: Option<String>,
    },
    /// Holds the lease, and will accept writes.
    Leader {
        /// The term held.
        term: Term,
    },
    /// Was the leader and has stopped. Terminal for this process.
    SteppedDown {
        /// Why.
        reason: StepDown,
    },
}

impl Standing {
    /// Whether writes may be attempted.
    #[must_use]
    pub const fn is_leader(&self) -> bool {
        matches!(self, Self::Leader { .. })
    }

    /// Whether this node has permanently given up the writer role.
    #[must_use]
    pub const fn has_stepped_down(&self) -> bool {
        matches!(self, Self::SteppedDown { .. })
    }
}

/// Why a node stopped being the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDown {
    /// The storage layer fenced it: another writer took over.
    ///
    /// The authoritative one. Whatever the lease says, this node's store is
    /// finished.
    Fenced,
    /// A renewal found the lease in someone else's hands.
    ///
    /// Weaker than being fenced — no write has failed yet — but a node whose
    /// lease has moved on is one fence away from finding out the hard way, and
    /// stopping first is the whole reason for renewing.
    LeaseLost,
    /// The node was asked to stand down, for a rolling restart.
    Resigned,
}

impl StepDown {
    /// A short reason, for the `Leadership` RPC and for logs.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Fenced => "fenced by another writer",
            Self::LeaseLost => "the lease was taken by another node",
            Self::Resigned => "resigned",
        }
    }
}

/// How often a leader renews, and how often a follower tries again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    /// Gap between renewals.
    pub renew_every: Duration,
    /// Gap between campaign attempts while following.
    pub campaign_every: Duration,
}

impl Cadence {
    /// The cadence for a lease of `term_length`.
    ///
    /// Renewing at a third of the term means two consecutive renewals can fail
    /// — a retried request, a slow object store — and the term still has a
    /// third of its life left. Renewing at a half would leave no margin for the
    /// second failure, and renewing much more often spends requests to buy
    /// nothing: the term does not get safer, only more expensive.
    ///
    /// Campaigning at the same rate is not derived from anything: a follower's
    /// polling interval trades takeover latency against request count, and
    /// there is no measurement here to prefer one number.
    #[must_use]
    pub fn for_term(term_length: Duration) -> Self {
        let renew_every = term_length / 3;
        Self {
            renew_every,
            campaign_every: renew_every,
        }
    }
}

/// A node's claim on the writer role.
///
/// Cheap to clone the handle to (`Arc<Leadership>`) and cheap to interrogate:
/// [`Leadership::is_leader`] reads a watch channel rather than the lease, so it
/// can sit on the hot path of every write.
#[derive(Debug)]
pub struct Leadership {
    lease: Arc<dyn Lease>,
    standing: watch::Sender<Standing>,
}

impl Leadership {
    /// A node that is following `lease` and has not yet campaigned.
    #[must_use]
    pub fn new(lease: Arc<dyn Lease>) -> Arc<Self> {
        let (standing, _) = watch::channel(Standing::Follower { leader: None });
        Arc::new(Self { lease, standing })
    }

    /// The lease this node campaigns on.
    #[must_use]
    pub fn lease(&self) -> &Arc<dyn Lease> {
        &self.lease
    }

    /// Where this node stands.
    #[must_use]
    pub fn standing(&self) -> Standing {
        self.standing.borrow().clone()
    }

    /// Whether writes may be attempted here.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        self.standing.borrow().is_leader()
    }

    /// The term held, if any.
    #[must_use]
    pub fn term(&self) -> Option<Term> {
        match &*self.standing.borrow() {
            Standing::Leader { term } => Some(term.clone()),
            _ => None,
        }
    }

    /// Watch for changes, so a caller can react to a takeover without polling.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Standing> {
        self.standing.subscribe()
    }

    /// Try once to become the leader.
    ///
    /// Returns whether this node is the leader afterwards. A node that has
    /// stepped down never campaigns again and returns `false` without touching
    /// the lease; see the module docs.
    pub async fn campaign(&self) -> bool {
        if self.standing.borrow().has_stepped_down() {
            return false;
        }
        match self.lease.acquire().await {
            Ok(term) => {
                self.standing.send_replace(Standing::Leader { term });
                true
            }
            Err(LeaseError::Held { holder, .. }) => {
                self.standing.send_replace(Standing::Follower {
                    leader: Some(holder),
                });
                false
            }
            Err(_) => {
                // A storage failure is not evidence that someone else holds
                // the lease, but it is also not permission to write: the last
                // thing known for certain is that this node is not confirmed
                // leader, so it follows and tries again.
                self.standing
                    .send_replace(Standing::Follower { leader: None });
                false
            }
        }
    }

    /// Extend the term, stepping down if it has moved on.
    ///
    /// Returns whether this node is still the leader.
    pub async fn renew(&self) -> bool {
        if !self.is_leader() {
            return false;
        }
        match self.lease.renew().await {
            Ok(term) => {
                self.standing.send_replace(Standing::Leader { term });
                true
            }
            Err(LeaseError::Lost { .. } | LeaseError::NotHeld) => {
                self.step_down(StepDown::LeaseLost).await;
                false
            }
            Err(_) => {
                // A failed renewal on a healthy lease is a transient storage
                // error, and the term has two thirds of its life left by
                // construction. Standing down here would hand the database over
                // every time the object store hiccuped.
                true
            }
        }
    }

    /// Stop being the writer because the storage layer said so.
    ///
    /// Call this on [`KernelError::WriterFenced`](slate_kernel::KernelError::WriterFenced)
    /// and nothing else. It is terminal.
    pub async fn fenced(&self) {
        self.step_down(StepDown::Fenced).await;
    }

    /// Stand down deliberately, for a rolling restart.
    ///
    /// Also terminal: this process's SlateDB handle is fine, but a node that
    /// resigned and then re-campaigned would fence whoever took over, which is
    /// the churn the lease exists to prevent.
    pub async fn resign(&self) {
        self.step_down(StepDown::Resigned).await;
    }

    async fn step_down(&self, reason: StepDown) {
        if self.standing.borrow().has_stepped_down() {
            return;
        }
        self.standing.send_replace(Standing::SteppedDown { reason });

        // Best effort, and deliberately after the standing is published: the
        // node must stop accepting writes whether or not the release lands, and
        // releasing first would leave a window where a successor could take the
        // lease while this node still believed it was the leader.
        //
        // Releasing at all is what turns a takeover from "wait out the term"
        // into "immediately", which for a fenced node matters — the fence
        // already means another writer is live and the lease is the only thing
        // still saying otherwise.
        let _ = self.lease.release().await;
    }
}

/// Run the campaign-and-renew loop until the node steps down.
///
/// Split out from [`Leadership`] rather than spawned by it so that a test can
/// drive the state machine a step at a time, and so that a deployment can put
/// the loop on whatever runtime and supervision it already has.
pub async fn maintain(leadership: Arc<Leadership>, cadence: Cadence) {
    loop {
        let standing = leadership.standing();
        match standing {
            Standing::SteppedDown { .. } => return,
            Standing::Leader { .. } => {
                tokio::time::sleep(cadence.renew_every).await;
                leadership.renew().await;
            }
            Standing::Follower { .. } => {
                if !leadership.campaign().await {
                    tokio::time::sleep(cadence.campaign_every).await;
                }
            }
        }
    }
}
