//! Routing reads across replicas.
//!
//! A pool answers two questions per read: *which* replica, and *whether it is
//! allowed to serve this read yet*.
//!
//! # Which replica: tenant affinity
//!
//! The obvious answer is round-robin, and it is the wrong default here. Reads
//! come from object storage, so a cache miss is a network round trip and a hit
//! is free — the ratio between them dominates read latency far more than
//! balance does. Because the keyspace is tenant-prefixed, one tenant's working
//! set is a contiguous range, so sending a tenant's reads consistently to the
//! same replica keeps that range in that replica's block cache. Spreading them
//! evenly instead gives every replica a cold copy of everything.
//!
//! This is a direct payoff from making tenant scoping physical: the same key
//! prefix that makes a tenant's data an isolated range also makes it a cacheable
//! one. Reads with no tenant to key on fall back to round-robin.
//!
//! Placement uses rendezvous hashing rather than modulo, so losing a replica
//! remaps only that replica's share of tenants instead of reshuffling every
//! tenant onto a cold cache.
//!
//! # Whether it may serve: the read token
//!
//! See [`crate::token`]. A read carrying [`Freshness::AtLeast`] may only be
//! served by a view that has reached that sequence; the pool prefers a replica
//! already there, waits briefly on the closest one otherwise, and reports
//! staleness rather than quietly serving an older view.

use crate::error::{KernelError, Result};
use crate::limits::ExecutionLimits;
use crate::record::RecordSnapshot;
use crate::store::KvReadStore;
use crate::token::Freshness;
use slate_schema::{Catalog, TableDef};
use slate_tuple::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::stats::Statistics;

/// How a pool decides where a read goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingPolicy {
    /// How long to wait for a replica to catch up to a required sequence
    /// before giving up on it.
    ///
    /// Waiting is usually right: the expected wait is under one manifest poll
    /// interval, and the alternative — sending the read to the writer — spends
    /// the scarce resource to save the abundant one. The cap keeps a stalled
    /// replica from turning into a stalled request.
    pub catch_up: Duration,
    /// Whether to route by tenant affinity. Turning it off gives round-robin,
    /// which balances better and caches worse.
    pub tenant_affinity: bool,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            catch_up: Duration::from_millis(250),
            tenant_affinity: true,
        }
    }
}

/// A set of read replicas, and the catalog and rules they serve under.
///
/// Holds `dyn KvReadStore`, so a writer — which is also readable — can sit in
/// the same pool as the fallback for reads that cannot tolerate lag.
pub struct ReplicaPool {
    replicas: Vec<Arc<dyn KvReadStore>>,
    writer: Option<Arc<dyn KvReadStore>>,
    catalog: Catalog,
    security: SecurityCatalog,
    statistics: Statistics,
    limits: ExecutionLimits,
    policy: RoutingPolicy,
    next: AtomicUsize,
}

impl core::fmt::Debug for ReplicaPool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReplicaPool")
            .field(
                "replicas",
                &self
                    .replicas
                    .iter()
                    .map(|r| r.replica_name())
                    .collect::<Vec<_>>(),
            )
            .field("has_writer", &self.writer.is_some())
            .field("policy", &self.policy)
            .finish()
    }
}

impl ReplicaPool {
    /// Build a pool over `replicas`.
    #[must_use]
    pub fn new(
        replicas: Vec<Arc<dyn KvReadStore>>,
        catalog: Catalog,
        security: SecurityCatalog,
    ) -> Self {
        Self {
            replicas,
            writer: None,
            catalog,
            security,
            statistics: Statistics::new(),
            limits: ExecutionLimits::default(),
            policy: RoutingPolicy::default(),
            next: AtomicUsize::new(0),
        }
    }

    /// Refuse reads through this pool that would exceed `limits`.
    ///
    /// A pooled read must have the same ceiling as a direct one, or which node
    /// served it would decide whether it was refused.
    #[must_use]
    pub fn with_limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Supply table statistics for the planner. See [`Statistics`].
    #[must_use]
    pub fn with_statistics(mut self, statistics: Statistics) -> Self {
        self.statistics = statistics;
        self
    }

    /// Add the writer as a fallback.
    ///
    /// Required to serve [`Freshness::Latest`], and used when no replica can
    /// reach a required sequence in time. Without it those reads fail rather
    /// than silently being served stale.
    #[must_use]
    pub fn with_writer(mut self, writer: Arc<dyn KvReadStore>) -> Self {
        self.writer = Some(writer);
        self
    }

    /// Override the routing policy.
    #[must_use]
    pub const fn with_policy(mut self, policy: RoutingPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Does `context` hold a grant for `action` on `table`?
    ///
    /// The check itself rather than a `security()` accessor, for the reason
    /// [`ReplicaPool::snapshot_from`] gives at length about `catalog()`: handing
    /// out the component invites a caller to assemble its own view of the
    /// security state, and a second assembly is a second thing to keep right.
    /// A caller that only wants the answer gets the answer.
    ///
    /// This does not replace the planner's check — that one is what protects
    /// the rows, and it runs whether or not this was called. This exists so a
    /// server can refuse *before* doing the work in front of the planner, which
    /// is where a caller with no grant was able to observe things about a table
    /// it cannot read.
    ///
    /// # Errors
    ///
    /// [`KernelError::AccessDenied`] when no role grants the action.
    pub fn authorize(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
    ) -> Result<()> {
        self.security.authorize(context, table, action)
    }

    /// The catalog this pool serves.
    #[must_use]
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Open a read view meeting `freshness`, routed by `affinity`.
    ///
    /// `affinity` is the tenant to key placement on; see
    /// [`ReplicaPool::tenant_of`] for getting it from a row's key values.
    ///
    /// Use [`ReplicaPool::snapshot_from`] if the caller has to say where the
    /// read went; this one throws that away.
    pub async fn snapshot(
        &self,
        freshness: Freshness,
        affinity: Option<&Value>,
    ) -> Result<RecordSnapshot<'_>> {
        self.snapshot_from(freshness, affinity)
            .await
            .map(|(snapshot, _)| snapshot)
    }

    /// Open a read view, and hand back the store that will serve it.
    ///
    /// The pairing is the whole point. [`ReplicaPool::snapshot`] gives a view
    /// and no way to say where it came from, and asking afterwards with
    /// [`ReplicaPool::route`] is not the same question: on the round-robin path
    /// the counter has already moved, so the second call names a replica that
    /// served nothing. Anything that reports routing onwards — the head node
    /// stamps every read response with it, because a single replica quietly
    /// serving the whole fleet is otherwise invisible — has to get the view and
    /// the name out of one decision. This is that call.
    ///
    /// The alternative was `security()` and `statistics()` accessors alongside
    /// [`ReplicaPool::catalog`], leaving each caller to assemble its own
    /// [`RecordSnapshot`]. That closes the smaller half of the problem: the
    /// state stops being stated twice, but the assembly is still copied out,
    /// and the day a snapshot needs a fourth component every caller that built
    /// its own is serving reads without it. Two accessors is also more added
    /// surface than one method, for a caller that then has to write the same
    /// four-argument constructor the pool already writes.
    ///
    /// The store comes back rather than only its name because the name is not
    /// all a caller reports — `served_by` on the wire carries the view's
    /// visible sequence too — and because returning the same
    /// `&Arc<dyn KvReadStore>` that [`ReplicaPool::route`] returns keeps one
    /// type for "the replica that served this" instead of inventing a second
    /// that exists only to be converted. It does leave the caller able to open
    /// another snapshot on that store, which is harmless: the hazard being
    /// removed here is deciding *where* twice, not reading twice from the one
    /// place already decided.
    pub async fn snapshot_from(
        &self,
        freshness: Freshness,
        affinity: Option<&Value>,
    ) -> Result<(RecordSnapshot<'_>, &Arc<dyn KvReadStore>)> {
        let store = self.route(freshness, affinity).await?;
        let snapshot = RecordSnapshot::over_with_limits(
            store.snapshot().await?,
            &self.catalog,
            &self.security,
            &self.statistics,
            self.limits,
        );
        Ok((snapshot, store))
    }

    /// Choose the store that will serve a read.
    ///
    /// Exposed so a caller can log or meter the decision — but only the
    /// decision it is about to act on. Calling this to find out where an
    /// already-open view came from asks the pool to decide again, and on the
    /// round-robin path it decides differently; [`ReplicaPool::snapshot_from`]
    /// is what pairs a view with the store that opened it.
    pub async fn route(
        &self,
        freshness: Freshness,
        affinity: Option<&Value>,
    ) -> Result<&Arc<dyn KvReadStore>> {
        if let Freshness::Latest = freshness {
            return self.writer.as_ref().ok_or(KernelError::NoReplicaAvailable {
                reason: "the read requires the writer, but no writer is in the pool",
            });
        }

        let order = self.preference_order(affinity);
        if order.is_empty() {
            return self.writer.as_ref().ok_or(KernelError::NoReplicaAvailable {
                reason: "the pool has no replicas and no writer",
            });
        }

        let Freshness::AtLeast(token) = freshness else {
            // Nothing to prove; take the most preferred replica.
            return order
                .first()
                .copied()
                .ok_or(KernelError::NoReplicaAvailable {
                    reason: "the pool has no replicas",
                });
        };

        // Prefer a replica that is already caught up, in affinity order, so the
        // common case costs no waiting and keeps its cache locality.
        for store in &order {
            if store
                .visible_sequence()
                .is_some_and(|visible| token.satisfied_by(visible))
            {
                return Ok(store);
            }
        }

        // Otherwise wait on the one closest to catching up: it is the one most
        // likely to arrive within the budget.
        if let Some(closest) = order
            .iter()
            .max_by_key(|store| store.visible_sequence().unwrap_or(0))
            && closest
                .wait_for_sequence(token.sequence(), self.policy.catch_up)
                .await
                .is_ok()
        {
            return Ok(closest);
        }

        // Falling back to the writer costs the scarce resource, but serving a
        // read that is knowably too old would break the promise the token makes.
        let writer = self
            .writer
            .as_ref()
            .ok_or(KernelError::NoReplicaAvailable {
                reason: "no replica caught up in time and no writer is in the pool",
            })?;

        // And the writer is held to that promise like everything else. It is
        // normally the most advanced node in the pool, which is why it is the
        // fallback — but "normally" is not "provably". A token minted by a
        // previous writer names a sequence this one need not have replayed,
        // which is precisely the handover this pool exists to survive. Serving
        // that read returns rows without the write the caller is holding a
        // token for, and returns them silently, which is the worse half.
        //
        // A writer that reports no sequence at all is served, because the
        // default `wait_for_sequence` refuses outright and every store that
        // does not track progress would stop answering `AtLeast` reads
        // entirely. That is the store declaring it cannot prove the promise
        // rather than the pool declining to check, and it is the one branch
        // here that takes freshness on trust.
        match writer.visible_sequence() {
            Some(visible) if token.satisfied_by(visible) => Ok(writer),
            None => Ok(writer),
            Some(_) => {
                writer
                    .wait_for_sequence(token.sequence(), self.policy.catch_up)
                    .await?;
                Ok(writer)
            }
        }
    }

    /// Replicas in descending preference order for `affinity`.
    fn preference_order(&self, affinity: Option<&Value>) -> Vec<&Arc<dyn KvReadStore>> {
        if self.replicas.is_empty() {
            return Vec::new();
        }
        match affinity.filter(|_| self.policy.tenant_affinity) {
            Some(tenant) => {
                let key = slate_tuple::encode(core::slice::from_ref(tenant));
                let mut ranked: Vec<(u64, &Arc<dyn KvReadStore>)> = self
                    .replicas
                    .iter()
                    .map(|store| (rendezvous_score(&key, store.replica_name()), store))
                    .collect();
                // Ties break on name so every process in the fleet agrees.
                ranked.sort_by(|a, b| {
                    b.0.cmp(&a.0)
                        .then(a.1.replica_name().cmp(b.1.replica_name()))
                });
                ranked.into_iter().map(|(_, store)| store).collect()
            }
            None => {
                // Nothing to key on, so spread the load instead.
                let start = self.next.fetch_add(1, Ordering::Relaxed);
                (0..self.replicas.len())
                    .filter_map(|offset| self.replicas.get((start + offset) % self.replicas.len()))
                    .collect()
            }
        }
    }

    /// The tenant value to route on for a row of `table`, given its key values.
    ///
    /// `None` when the table is not tenant-scoped, in which case there is no
    /// affinity to exploit and routing falls back to round-robin.
    #[must_use]
    pub fn tenant_of(table: &TableDef, primary_key: &[Value]) -> Option<Value> {
        crate::keys::tenant_value(table, primary_key)
    }
}

/// Rendezvous ("highest random weight") score for a key on one replica.
///
/// Rendezvous rather than modulo so that losing a replica remaps only the
/// tenants that were on it. FNV-1a rather than the standard library's hasher
/// because placement must be identical in every process across the fleet, and
/// `DefaultHasher`'s output is explicitly not guaranteed stable between
/// compiler releases.
///
/// The avalanche step at the end is not optional. Rendezvous compares whole
/// hash values, and FNV-1a mixes its high bits weakly for short inputs — with
/// one-character replica names it skewed the split to 13%/37% instead of 25%
/// each. The finaliser spreads that back out.
fn rendezvous_score(key: &[u8], replica: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    // The separator keeps ("ab", "c") from colliding with ("a", "bc").
    for byte in key.iter().chain(b"\xff".iter()).chain(replica.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    avalanche(hash)
}

/// The MurmurHash3 64-bit finaliser: mixes every input bit into every output
/// bit, which is what comparing whole hash values requires.
const fn avalanche(mut hash: u64) -> u64 {
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    hash ^= hash >> 33;
    hash
}
