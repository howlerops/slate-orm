//! The head node: the gRPC surface, and where each request goes.
//!
//! # Two destinations, chosen per request
//!
//! A write goes to this node's own writer store, and only if this node holds
//! the lease. A read goes through the [`ReplicaPool`], to whichever replica
//! satisfies its freshness. That split is not an optimisation, it is what makes
//! a handover survivable: a fenced writer cannot even open a read transaction,
//! so a head node that served reads from its writer would go dark the moment it
//! was replaced. See `handover.rs` and `topology.md`.
//!
//! The one exception is a read inside a transaction, which has to be served by
//! the transaction — that is what a transaction is. Those reads do go to the
//! writer, and they do fail on a takeover. A client that wants to keep reading
//! through a handover reads outside a transaction.
//!
//! # One call decides where a read goes and opens it
//!
//! Every read response carries `served_by` — routing is the thing an operator
//! most needs to see, and a replica quietly serving everything is invisible
//! without it. Naming it truthfully means the routing decision and the view
//! have to come out of the same call, because asking [`ReplicaPool::route`] a
//! second time advances the round-robin counter and answers about a replica
//! that served nothing. [`ReplicaPool::snapshot_from`] is that call.
//!
//! This node therefore keeps no catalog, security catalog or statistics of its
//! own for reads: it reads them out of the pool it already routes through.
//! [`HeadConfig`] states them once, on the way in. The previous shape had the
//! head hold a second copy and assemble the [`RecordSnapshot`] itself, with
//! nothing but care keeping the two statements equal — the same shape that
//! already cost this project a latency fixture and a cost model that disagreed
//! about rows per block.
//!
//! # Refusing before spending a round trip
//!
//! Leadership is checked from a watch channel before any write touches storage.
//! After a fence the writer's `begin` fails anyway — but it fails after a call
//! into SlateDB, and a node that has stepped down should not be making those.

use crate::auth::Authenticator;
use crate::convert::{
    GroupedSource, MultiRead, aggregate_from_proto_query, chain_plan_to_proto,
    explanation_to_proto, freshness_from_proto, group_to_proto, grouped_explanation_to_proto,
    join_explanation_to_proto, join_from_proto, multi_row_to_proto, primary_key_from_proto,
    query_from_proto, row_from_proto, row_to_proto, row_to_proto_split, two_tables,
    value_from_proto, value_to_proto,
};
use crate::convert::{Space, assignments_from_proto, expr_named};
use crate::fingerprint;
use crate::leadership::{Leadership, Standing};
use crate::proto as pb;
use crate::proto::records_server::{Records, RecordsServer};
use crate::session::{
    GroupedExplanation, Limits, MultiCursor, MultiExplanation, MultiRow, Sessions,
};
use crate::status::reason_of;
use crate::status::{from_kernel, redirect};
use slate_kernel::{
    Action, ExecutionLimits, Expr, Freshness, Group, KernelError, KvReadStore, KvStore, Query,
    ReadToken, RecordSnapshot, RecordStore, RecordTransaction, ReplicaPool, RoutingPolicy, Scalar,
    SecurityCatalog, SecurityContext, Statistics,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::Value;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status};

/// What a read served from within a transaction reports as its view.
///
/// Not a replica name, because it is not a replica: it is the writer's own
/// transaction snapshot. Named rather than left blank so that a client
/// gathering routing statistics can see the difference.
const IN_TRANSACTION: &str = "writer (in transaction)";

/// Told how many rows each standalone write actually touched.
///
/// The metrics layer around this server sees a method, a status and a
/// duration — never a response body — so "the purge ran" is observable and
/// "the purge erased nine thousand rows" is not. That is the gap this closes,
/// and it closes it for every write rather than for the purge that prompted
/// it: a purge-shaped counter would have been a special case of exactly this
/// hook, and building the special case first is how a general one never
/// arrives.
///
/// Implemented outside this crate, because what to do with the number — a
/// counter, a log line, nothing — is a deployment's business and this crate
/// has no opinion. `None` is the ordinary case and costs a branch per write.
///
/// # When it is told
///
/// **After the write committed, never before.** There are three paths and all
/// three obey it, for three different reasons:
///
/// - [`Head::autocommit`], for a standalone write: a conflicting write is
///   retried and applies more than once while committing once, so counting
///   attempts would report a number no row ever had.
/// - An atomic batch that opens its own transaction: the same retry, one level
///   up, so the counts are returned out of the closure rather than added
///   inside it.
/// - A caller's own transaction, in `session::run`: the caller decides whether
///   any of it lands. The counts accumulate for the transaction's life and are
///   reported on a successful `Commit`; a rollback, an idle timeout, a fence
///   or a client that walked away reports nothing.
///
/// A deployment therefore sees committed rows and only committed rows, which
/// is what makes the number worth alerting on.
pub trait WriteObserver: Send + Sync {
    /// `kind` is the statement (`insert`, `purge_deleted`, …) and `table` is
    /// its table. Both are bounded — by the enum and by the catalog — so a
    /// counter keyed on the pair cannot grow without limit, which is the
    /// failure a label taken from a request would have.
    ///
    /// Called once per statement for a standalone write or an atomic batch,
    /// and once per (statement, table) *pair* for a transaction, which is why
    /// an implementation must add rather than set: a transaction that inserted
    /// into one table twenty times is one call of the sum.
    fn wrote(&self, kind: &'static str, table: &str, affected: u64);
}

/// Everything a head node needs that is not a request.
///
/// A struct rather than a long argument list because the catalog, the security
/// catalog and the statistics each have to reach *both* the writer store and
/// the replica pool. Passing them separately to each is a shape where handing
/// one half a different catalog compiles and is undebuggable. This is the only
/// place any of the three is stated: [`Head::new`] pours it into those two and
/// keeps nothing back.
#[derive(Debug, Clone)]
pub struct HeadConfig {
    /// The tables this node serves.
    pub catalog: Catalog,
    /// The rules it enforces. An empty catalog denies every non-superuser.
    pub security: SecurityCatalog,
    /// What the planner believes about the data.
    pub statistics: Statistics,
    /// Per-request ceilings on the operators that hold unbounded state.
    pub execution: ExecutionLimits,
    /// How reads are spread across replicas.
    pub routing: RoutingPolicy,
    /// What the node will spend on open transactions and stream batches.
    pub limits: Limits,
}

impl HeadConfig {
    /// A configuration serving `catalog` under `security`, with defaults for
    /// the rest.
    #[must_use]
    pub fn new(catalog: Catalog, security: SecurityCatalog) -> Self {
        Self {
            catalog,
            security,
            statistics: Statistics::new(),
            execution: ExecutionLimits::default(),
            routing: RoutingPolicy::default(),
            limits: Limits::default(),
        }
    }

    /// Supply table statistics for the planner.
    #[must_use]
    pub fn with_statistics(mut self, statistics: Statistics) -> Self {
        self.statistics = statistics;
        self
    }

    /// Refuse requests that would exceed `limits`.
    ///
    /// These reach every store this node builds — the writer and each replica
    /// — because a ceiling that applied to only one of them would depend on
    /// which node served the read.
    #[must_use]
    pub fn with_execution_limits(mut self, limits: ExecutionLimits) -> Self {
        self.execution = limits;
        self
    }

    /// Change how reads are routed.
    #[must_use]
    pub const fn with_routing(mut self, routing: RoutingPolicy) -> Self {
        self.routing = routing;
        self
    }

    /// Change the transaction and streaming limits.
    #[must_use]
    pub const fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }
}

/// A head node.
///
/// A pool of replicas, a lease, the rules for who may ask what, and — if this
/// node has one — a writer store. Everything it holds is shared behind `Arc`,
/// because request handlers hand pieces of it to spawned tasks — see
/// [`crate::session`] for why the transaction path has to.
pub struct Head<S> {
    pool: Arc<ReplicaPool>,
    /// `None` on a node built by [`Head::read_only`].
    ///
    /// Optional rather than a second type, because a writerless head differs
    /// from a writing one in exactly one place — [`Head::leader`] — and every
    /// other line of this file is the same code. Two types would be two gRPC
    /// service implementations, and the read path is where the security
    /// filtering lives: a second copy of it is the last thing this crate
    /// wants. The alternative considered and rejected was a
    /// `Head<NoWriter>` with a stub `KvStore`, which puts a store that panics
    /// on write into the type system and only moves the check to run time.
    writer: Option<Arc<RecordStore<Arc<S>>>>,
    leadership: Arc<Leadership>,
    authenticator: Arc<dyn Authenticator>,
    sessions: Arc<Sessions>,
    limits: Limits,
    /// Told what each committed write touched; see [`WriteObserver`].
    ///
    /// A field set after construction rather than a `HeadConfig` entry,
    /// because every existing caller builds that struct literally and a new
    /// field would break each of them for something all but one of them wants
    /// to leave unset.
    writes: Option<Arc<dyn WriteObserver>>,
}

impl<S> Head<S> {
    /// Report every standalone write's row count to `observer`.
    ///
    /// Consuming, so it reads as part of building the node rather than as a
    /// mutation of one already serving — there is no way to attach an observer
    /// to a head that is already handling requests, which keeps "the counters
    /// started late" from being a state anybody has to reason about.
    #[must_use]
    pub fn observing_writes(mut self, observer: Arc<dyn WriteObserver>) -> Self {
        self.writes = Some(observer);
        self
    }
}

impl<S> core::fmt::Debug for Head<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Head")
            .field("standing", &self.leadership.standing())
            .field("writer", &self.writer.is_some())
            .field("open_transactions", &self.sessions.len())
            .field("pool", &self.pool)
            .finish_non_exhaustive()
    }
}

impl<S> Head<S> {
    /// A head node with **no writer store**, reading through `replicas`.
    ///
    /// This is the node `topology.md` describes as a follower and this crate
    /// has always claimed to support: it answers every read a leader answers,
    /// and refuses every write with the leader's name in a `slate-leader`
    /// trailer.
    ///
    /// # Why it has to exist
    ///
    /// Opening a SlateDB writer *fences* whichever writer was there. So a node
    /// that has lost the lease cannot build a [`Head::new`] without killing the
    /// node that won — which meant, until this constructor, that a second node
    /// could only refuse to start. "Reads scale, writes do not" is the whole
    /// shape of this system, and it was not reachable: every replica-serving
    /// process had to bring a writer it must never use.
    ///
    /// # What it cannot do
    ///
    /// It cannot be promoted. A node with no writer store that somehow won the
    /// lease still has nothing to write *to*, and building the store afterwards
    /// is not a matter of a field: it is opening a database, which is the step
    /// that has to happen after the campaign and never before. So a read-only
    /// node should not campaign — [`crate::leadership::follow`] is the loop for
    /// it, which keeps the leader's name fresh for the redirect and never
    /// acquires — and promotion is a restart. [`Head::leader`] refuses with
    /// that sentence rather than panicking, because "won the lease with no
    /// store" is a wiring mistake and an operator needs to be told which one.
    ///
    /// The pool has no writer in it either, so [`Freshness::Latest`] is
    /// refused rather than served from a replica that cannot satisfy it. That
    /// is not new behaviour: `ReplicaPool` already refuses a `Latest` it has no
    /// writer for, and a follower is exactly the node that has none.
    ///
    /// `S` is still a type parameter with nothing to hold it up, so a caller
    /// names the writer type this node *would* have had —
    /// `Head::<SlateStore>::read_only(…)`. That is deliberate: the same binary
    /// builds either kind depending on how the campaign went, and both arms
    /// have to have one type.
    #[must_use]
    pub fn read_only(
        config: HeadConfig,
        replicas: Vec<Arc<dyn KvReadStore>>,
        leadership: Arc<Leadership>,
        authenticator: Arc<dyn Authenticator>,
    ) -> Self {
        let HeadConfig {
            catalog,
            security,
            statistics,
            routing,
            limits,
            execution,
        } = config;

        let pool = ReplicaPool::new(replicas, catalog, security)
            .with_statistics(statistics)
            .with_limits(execution)
            .with_policy(routing);

        Self {
            pool: Arc::new(pool),
            writer: None,
            leadership,
            authenticator,
            sessions: Arc::new(Sessions::new(limits)),
            limits,
            writes: None,
        }
    }

    /// Whether this node has a writer store at all.
    ///
    /// `false` for a node built by [`Head::read_only`]. Not the same question
    /// as [`Leadership::is_leader`]: a node can have a writer and not hold the
    /// lease, and that is the ordinary state of a leader that has just been
    /// replaced.
    #[must_use]
    pub const fn has_writer(&self) -> bool {
        self.writer.is_some()
    }
}

impl<S: KvStore + KvReadStore> Head<S> {
    /// A head node over `writer`, reading through `replicas`.
    ///
    /// The writer joins the pool as well, as the fallback for a read no replica
    /// can serve: [`Freshness::Latest`] has nowhere else to go, and a pool
    /// without a writer refuses it rather than substituting a stale view.
    ///
    /// Only a node that holds the lease should build one of these, because
    /// building one means having opened the writer store, and opening a
    /// SlateDB writer fences whoever held it. A node that lost the campaign
    /// wants [`Head::read_only`].
    #[must_use]
    pub fn new(
        config: HeadConfig,
        writer: Arc<S>,
        replicas: Vec<Arc<dyn KvReadStore>>,
        leadership: Arc<Leadership>,
        authenticator: Arc<dyn Authenticator>,
    ) -> Self {
        let HeadConfig {
            catalog,
            security,
            statistics,
            routing,
            limits,
            execution,
        } = config;

        let pool = ReplicaPool::new(replicas, catalog.clone(), security.clone())
            .with_statistics(statistics.clone())
            .with_limits(execution)
            .with_policy(routing)
            .with_writer(Arc::clone(&writer) as Arc<dyn KvReadStore>);
        let store = RecordStore::new(Arc::clone(&writer), catalog, security)
            .with_statistics(statistics)
            .with_limits(execution);

        Self {
            pool: Arc::new(pool),
            writer: Some(Arc::new(store)),
            leadership,
            authenticator,
            sessions: Arc::new(Sessions::new(limits)),
            limits,
            writes: None,
        }
    }

    /// A table this node serves, by id.
    ///
    /// A consistency check rather than a lookup — the name was resolved
    /// against this same catalog a moment ago — but the alternative is an
    /// index into a catalog that has to be assumed to match.
    fn definition(&self, id: TableId) -> Result<&TableDef, Status> {
        self.pool
            .catalog()
            .table(id)
            .ok_or_else(|| from_kernel(&KernelError::UnknownTable(id)))
    }

    /// Every table of a multi-table read, in request order.
    fn definitions(&self, ids: &[TableId]) -> Result<Vec<&TableDef>, Status> {
        ids.iter().map(|id| self.definition(*id)).collect()
    }

    /// Where this node stands with respect to writing.
    #[must_use]
    pub fn leadership(&self) -> &Arc<Leadership> {
        &self.leadership
    }

    /// How many transactions are open.
    #[must_use]
    pub fn open_transactions(&self) -> usize {
        self.sessions.len()
    }

    /// Wrap this head node as a tonic service.
    #[must_use]
    pub fn into_service(self) -> RecordsServer<Self> {
        RecordsServer::new(self)
    }

    // --- shared request machinery -----------------------------------------

    fn context<T>(&self, request: &Request<T>) -> Result<SecurityContext, Status> {
        self.authenticator.authenticate(request.metadata())
    }

    fn table(&self, name: &str) -> Result<&TableDef, Status> {
        self.pool.catalog().table_by_name(name).ok_or_else(|| {
            // `NOT_FOUND` rather than `INVALID_ARGUMENT`: the request is
            // well-formed, this catalog simply has no such table.
            Status::new(Code::NotFound, format!("no table named `{name}`"))
        })
    }

    /// The table, if this caller holds a grant for `action` on it.
    ///
    /// The kernel authorises again inside the planner, and that is the check
    /// that actually protects the rows — this one is in front of everything
    /// the handler does *before* reaching the kernel, which is where the
    /// disclosure was. A caller with no grant used to get as far as
    /// `fingerprint::check`, and so could confirm a guessed `(name, type,
    /// key)` layout one 64-bit fingerprint at a time against a table they
    /// cannot read. Now they get `PERMISSION_DENIED` and learn nothing about
    /// the shape.
    ///
    /// The action must be the same one the kernel will check, or this refuses
    /// something the kernel would have allowed. That is why it is passed in
    /// rather than inferred.
    ///
    /// Table *existence* is still disclosed: an unknown name answers
    /// `NOT_FOUND` and a known one with no grant answers `PERMISSION_DENIED`.
    /// Deliberate, and the same choice Postgres makes. Collapsing the two
    /// would hide a table's existence from someone who cannot use it anyway,
    /// at the cost of making every genuine misconfiguration — the overwhelmingly
    /// common case — indistinguishable from a typo.
    fn authorized_table(
        &self,
        context: &SecurityContext,
        name: &str,
        action: Action,
    ) -> Result<&TableDef, Status> {
        let table = self.table(name)?;
        self.pool
            .authorize(context, table, action)
            .map_err(|error| from_kernel(&error))?;
        Ok(table)
    }

    /// Authorise every table a multi-table read names, before it is converted.
    ///
    /// `join_from_proto` and `aggregate_from_proto_query` take a `Catalog`
    /// rather than a `SecurityContext`, so they resolve and convert with no
    /// idea who is asking — and the handlers called them first. That is
    /// finding 8 on four more handlers than the ones it was written about:
    /// converting an input resolves its `ColumnRef`s against the table's
    /// width, and the refusal states the width. Measured on `join`, from a
    /// role granted nothing: "the projection names column 99 of table
    /// `users`, which has 4 columns".
    ///
    /// The names are read off the wire, because a converted read is the thing
    /// that cannot be produced safely yet. Every input, not the first: a join
    /// reads all of them and the kernel checks each, so checking one would
    /// leave the others' widths readable.
    fn authorize_join_inputs(
        &self,
        context: &SecurityContext,
        wire: &pb::JoinQuery,
        action: Action,
    ) -> Result<(), Status> {
        for input in &wire.inputs {
            if let Some(query) = input.query.as_ref() {
                self.authorized_table(context, &query.table, action)?;
            }
        }
        Ok(())
    }

    /// [`Head::authorize_join_inputs`] for an aggregate, over either source.
    ///
    /// Both arms rather than the one that is set: the converter refuses a
    /// request setting both, and doing that refusal *after* this one would
    /// make "set exactly one" a way to choose which table gets checked.
    fn authorize_aggregate_inputs(
        &self,
        context: &SecurityContext,
        wire: &pb::AggregateQuery,
        action: Action,
    ) -> Result<(), Status> {
        if let Some(input) = wire.input.as_ref() {
            self.authorized_table(context, &input.table, action)?;
        }
        if let Some(join) = wire.join.as_ref() {
            self.authorize_join_inputs(context, join, action)?;
        }
        Ok(())
    }

    /// The writer, if this node may use it.
    fn leader(&self) -> Result<&Arc<RecordStore<Arc<S>>>, Status> {
        match self.leadership.standing() {
            // Holding the lease and having no store is a wiring mistake — a
            // node built by `Head::read_only` was left campaigning — and it is
            // one an operator has to be *told*, because from the outside it
            // looks like a leader refusing writes. `UNAVAILABLE` with no
            // `slate-leader` trailer, because this node holds the lease and
            // there is nowhere better to send the request until somebody
            // restarts something.
            Standing::Leader { .. } => self.writer.as_ref().ok_or_else(|| {
                redirect(
                    "this node holds the lease but was started read-only, so it has no writer \
                     store: it cannot be promoted without a restart, and it should not have been \
                     campaigning — see `leadership::follow`",
                    None,
                )
            }),
            Standing::Follower { leader } => {
                Err(redirect("this node is not the writer", leader.as_deref()))
            }
            Standing::SteppedDown { reason } => Err(redirect(
                format!("this node stepped down: {}", reason.reason()),
                None,
            )),
        }
    }

    /// The tenant a read should be routed by.
    ///
    /// The principal's tenant, not one dug out of the filter: on a
    /// tenant-scoped table the security layer forces the principal's tenant
    /// onto the predicate anyway, so it is the tenant whose key range the read
    /// will actually touch. A tenant taken from the filter could name a range
    /// the read is not permitted to reach, and would send the read to a replica
    /// caching somebody else's data.
    fn affinity(table: &TableDef, context: &SecurityContext) -> Option<Value> {
        Self::affinity_over(&[table], context)
    }

    /// The tenant a read over several tables should be routed by.
    ///
    /// The same rule, applied to the set: if *any* input is tenant-scoped, the
    /// principal's tenant is the prefix the read will touch on that input, and
    /// keeping it on one replica is what keeps that range warm. A join of a
    /// tenant-scoped table to a shared lookup table still routes by tenant,
    /// because the tenant-scoped side is the one whose working set is large
    /// enough for locality to matter.
    fn affinity_over(tables: &[&TableDef], context: &SecurityContext) -> Option<Value> {
        tables
            .iter()
            .any(|table| table.tenant_column().is_some())
            .then(|| context.principal().tenant.clone())
            .flatten()
    }

    /// Open the view that will serve a read, and say truthfully where it came
    /// from.
    ///
    /// One call into the pool, deliberately: see the module docs and
    /// [`ReplicaPool::snapshot_from`].
    /// One step of a relationship path, resolved against the catalog.
    ///
    /// Every field is an answer to "which table, and which column": the whole
    /// of a relationship, once the catalog has been consulted, is which rows
    /// come back and which two columns line up.
    ///
    /// The two directions are mirror images and that is why this is one
    /// function rather than two. `CHILDREN` reads the rows holding the foreign
    /// key, matched on that key, from parents identified by their primary key;
    /// `PARENTS` reads the rows the key points at, matched on their primary
    /// key, from children identified by the key they hold. Swap
    /// `(rows_from, match_on)` with `(source, source_column)` and one becomes
    /// the other.
    fn resolve_relation(&self, relation: &pb::Relation) -> Result<ResolvedStep<'_>, Status> {
        // Resolve the relationship against the catalog's foreign keys, not
        // against a separate declaration. See `Relation` in the proto for why
        // there is no separate declaration to resolve against.
        let child = self.table(&relation.table)?;
        let Some(key) = child
            .foreign_keys()
            .iter()
            .find(|k| k.name() == relation.foreign_key)
        else {
            return Err(Status::new(
                Code::InvalidArgument,
                format!(
                    "`{}` has no foreign key named `{}`; it has {}",
                    child.name(),
                    relation.foreign_key,
                    if child.foreign_keys().is_empty() {
                        "none".to_owned()
                    } else {
                        child
                            .foreign_keys()
                            .iter()
                            .map(|k| format!("`{}`", k.name()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ),
            ));
        };

        let parent = self.pool.catalog().table(key.parent()).ok_or_else(|| {
            Status::new(
                Code::Internal,
                format!("`{}` names a parent that is not in the catalog", key.name()),
            )
        })?;

        // The relating column is the one key column that is *not* the tenant.
        //
        // A tenant-scoped table almost always has a composite key —
        // `(tenant_id, id)` — and refusing every composite key would refuse the
        // commonest multi-tenant schema there is. It does not need refusing,
        // because the tenant is not part of what relates two rows: the security
        // catalog forces `tenant_column = principal.tenant` onto every read of
        // such a table, so the tenant is already pinned before the filter below
        // is applied, and matching on it again would be a tautology.
        //
        // What genuinely cannot work is a key with two *non-tenant* columns: the
        // filter compares one column against one list, and a real composite
        // needs `(a, b) IN [(…), (…)]`, which the kernel has no operator for.
        // Refused by name rather than by relating on the first column alone,
        // which would match every row whose first key part agreed — more rows
        // than were asked for, with no error.
        fn relating(columns: &[Ordinal], table: &TableDef) -> Option<Ordinal> {
            let tenant = table.tenant_column();
            let mut rest = columns.iter().filter(|c| Some(**c) != tenant);
            let only = rest.next()?;
            rest.next().is_none().then_some(*only)
        }
        fn besides_the_tenant(columns: &[Ordinal], table: &TableDef) -> usize {
            columns
                .iter()
                .filter(|c| Some(**c) != table.tenant_column())
                .count()
        }

        let Some(child_column) = relating(key.columns(), child) else {
            return Err(Status::new(
                Code::InvalidArgument,
                format!(
                    "`{}` relates on {} columns besides the tenant; a relationship is \
                     resolved with one column against one list, and a genuine composite \
                     key needs a tuple comparison the kernel does not have",
                    relation.foreign_key,
                    besides_the_tenant(key.columns(), child)
                ),
            ));
        };
        let Some(parent_column) = relating(parent.primary_key(), parent) else {
            return Err(Status::new(
                Code::InvalidArgument,
                format!(
                    "`{}` has {} primary key columns besides the tenant; relating on a \
                     composite key is not supported",
                    parent.name(),
                    besides_the_tenant(parent.primary_key(), parent)
                ),
            ));
        };

        Ok(match relation.direction() {
            pb::relation::Direction::Children => ResolvedStep {
                rows_from: child,
                match_on: child_column,
                source: parent,
                source_column: parent_column,
            },
            pb::relation::Direction::Parents => ResolvedStep {
                rows_from: parent,
                match_on: parent_column,
                source: child,
                source_column: child_column,
            },
            pb::relation::Direction::Unspecified => {
                return Err(Status::new(
                    Code::InvalidArgument,
                    "no direction given: CHILDREN reads the rows holding the foreign \
                     key, PARENTS reads the rows it points at",
                ));
            }
        })
    }

    async fn read_view(
        &self,
        freshness: Freshness,
        affinity: Option<&Value>,
    ) -> Result<(RecordSnapshot<'_>, pb::ServedBy), Status> {
        let (view, store) = self
            .pool
            .snapshot_from(freshness, affinity)
            .await
            .map_err(|error| from_kernel(&error))?;
        Ok((view, served_by(store.as_ref())))
    }

    /// Run a write as its own transaction, retrying a conflict.
    ///
    /// Conflicts are ordinary here — a unique index is enforced by two writers
    /// colliding on one key — so the retry loop belongs on this path rather
    /// than in every client. [`with_retries`] supplies it, with the store's own
    /// policy and its jittered backoff.
    ///
    /// The operation is a [`Write`] rather than a closure. `RecordStore::transact`
    /// takes an `AsyncFn(&RecordTransaction<'_>)`, which is the nicer API and
    /// which cannot be used from inside a `#[tonic::async_trait]` method: the
    /// trait's futures are boxed with a `Send` bound, and the compiler cannot
    /// prove `Send` for a higher-ranked future built from a closure over a
    /// borrowed transaction. It reports `Send is not general enough` at the
    /// handler, several frames from the cause. An enum of the three write
    /// shapes has concrete lifetimes and no such problem.
    /// One operation, decoded and authorized, before anything is written.
    ///
    /// `at` is the operation's index, so a refusal says which one — a batch of
    /// fifty whose message is "no such column" and nothing else is a message
    /// that costs the caller a bisection.
    fn decode_operation(
        &self,
        context: &SecurityContext,
        operation: &pb::BatchOperation,
        at: usize,
    ) -> Result<Decoded<'_>, Status> {
        use pb::batch_operation::Of;
        let named = |status: Status| -> Status {
            Status::new(
                status.code(),
                format!("operation {at}: {}", status.message()),
            )
        };
        let Some(of) = operation.of.as_ref() else {
            return Err(Status::new(
                Code::InvalidArgument,
                format!("operation {at} is empty"),
            ));
        };
        // Refused rather than ignored: a caller who set it is asking for the
        // batch to join their transaction, and the request-level
        // `transaction` field is where that is said.
        let carried = match of {
            Of::Insert(r) => &r.transaction,
            Of::Update(r) => &r.transaction,
            Of::Delete(r) => &r.transaction,
            Of::DeleteWhere(r) => &r.transaction,
            Of::UpdateWhere(r) => &r.transaction,
        };
        if !carried.is_empty() {
            return Err(Status::new(
                Code::InvalidArgument,
                format!(
                    "operation {at} sets `transaction`, which has no meaning inside a batch; \
                     put it on the `BatchRequest` instead, with ALL_OR_NOTHING"
                ),
            ));
        }

        match of {
            Of::Insert(r) => {
                let table = self
                    .authorized_table(context, &r.table, Action::Insert)
                    .map_err(named)?;
                fingerprint::check(table, r.schema.as_ref()).map_err(named)?;
                let rows = r
                    .rows
                    .iter()
                    .map(row_from_proto)
                    .collect::<Result<Vec<Row>, Status>>()
                    .map_err(named)?;
                Ok(Decoded::Insert {
                    table,
                    rows,
                    upsert: r.upsert,
                })
            }
            Of::Update(r) => {
                let table = self
                    .authorized_table(context, &r.table, Action::Update)
                    .map_err(named)?;
                fingerprint::check(table, r.schema.as_ref()).map_err(named)?;
                let (rows, expected) = update_rows(&r.rows, &r.expected).map_err(named)?;
                Ok(Decoded::Update {
                    table,
                    rows,
                    expected,
                })
            }
            Of::Delete(r) => {
                let table = self
                    .authorized_table(context, &r.table, Action::Delete)
                    .map_err(named)?;
                fingerprint::check(table, r.schema.as_ref()).map_err(named)?;
                let (keys, expected) =
                    delete_keys(&r.primary_keys, &r.expected, table).map_err(named)?;
                Ok(Decoded::Delete {
                    table,
                    keys,
                    expected,
                })
            }
            Of::DeleteWhere(r) => {
                let table = self
                    .authorized_table(context, &r.table, Action::Delete)
                    .map_err(named)?;
                fingerprint::check(table, r.schema.as_ref()).map_err(named)?;
                let predicate = predicate_from_proto(r.filter.as_ref(), table).map_err(named)?;
                Ok(Decoded::DeleteWhere {
                    table,
                    predicate,
                    returning: r.returning,
                })
            }
            Of::UpdateWhere(r) => {
                let table = self
                    .authorized_table(context, &r.table, Action::Update)
                    .map_err(named)?;
                fingerprint::check(table, r.schema.as_ref()).map_err(named)?;
                let predicate = predicate_from_proto(r.filter.as_ref(), table).map_err(named)?;
                let assignments = assignments_from_proto(&r.assignments, table).map_err(named)?;
                if assignments.is_empty() {
                    return Err(Status::new(
                        Code::InvalidArgument,
                        format!("operation {at}: an update needs at least one assignment"),
                    ));
                }
                Ok(Decoded::UpdateWhere {
                    table,
                    predicate,
                    assignments,
                    returning: r.returning,
                })
            }
        }
    }

    /// Every operation in one transaction: all of them land, or none does.
    async fn batch_atomically(
        &self,
        context: &SecurityContext,
        decoded: Vec<Decoded<'_>>,
        transaction: &str,
    ) -> Result<Response<pb::BatchResponse>, Status> {
        if !transaction.is_empty() {
            // The caller's own transaction. The batch does not commit — that
            // is the caller's to do — so there is no sequence to report yet,
            // which is the same rule every other write inside a transaction
            // follows.
            // The per-operation session methods that already exist, rather
            // than a new `Command` carrying a `Decoded`: the session actor
            // takes owned values and a `TableId`, and `Decoded` borrows a
            // `&TableDef` from the catalog. Reusing the five methods keeps a
            // batched write and a lone write on one path through the actor.
            for operation in decoded {
                // Read before the match, which consumes the operation.
                let at_most = operation.at_most(self.limits.max_returned_rows);
                match operation {
                    Decoded::Insert {
                        table,
                        rows,
                        upsert,
                    } => {
                        self.sessions
                            .insert(transaction, context, table.id(), rows, upsert)
                            .await?;
                    }
                    Decoded::Update {
                        table,
                        rows,
                        expected,
                    } => {
                        self.sessions
                            .update(transaction, context, table.id(), rows, expected)
                            .await?;
                    }
                    Decoded::Delete {
                        table,
                        keys,
                        expected,
                    } => {
                        self.sessions
                            .delete(transaction, context, table.id(), keys, expected)
                            .await?;
                    }
                    Decoded::DeleteWhere {
                        table, predicate, ..
                    } => {
                        self.sessions
                            .delete_where(transaction, context, table.id(), predicate, at_most)
                            .await?;
                    }
                    Decoded::UpdateWhere {
                        table,
                        predicate,
                        assignments,
                        ..
                    } => {
                        self.sessions
                            .update_where(
                                transaction,
                                context,
                                table.id(),
                                predicate,
                                assignments,
                                at_most,
                            )
                            .await?;
                    }
                }
            }
            return Ok(Response::new(pb::BatchResponse {
                results: Vec::new(),
                sequence: None,
            }));
        }

        let writer = self.leader()?;
        let decoded = &decoded;
        let returnable = self.limits.max_returned_rows;
        let outcome = writer
            .transact_boxed_tracked(move |txn| {
                Box::pin(async move {
                    // Collected and returned rather than counted here: the
                    // closure is `Fn` because it runs once per retry, and a
                    // conflicting batch that applies twice and commits once
                    // would otherwise report every row twice.
                    let mut wrote = Vec::with_capacity(decoded.len());
                    for operation in decoded {
                        wrote.push(operation.apply(txn, context, returnable).await?);
                    }
                    Ok(wrote)
                })
            })
            .await;
        match outcome {
            Ok((wrote, token)) => {
                if let Some(observer) = &self.writes {
                    for (kind, table, affected) in &wrote {
                        observer.wrote(kind, table, *affected);
                    }
                }
                Ok(Response::new(pb::BatchResponse {
                    // No per-operation results: they all happened. Reporting a
                    // list of successes would invite a caller to check it, and the
                    // only thing it could ever say is "yes" for every entry.
                    results: Vec::new(),
                    sequence: token.map(ReadToken::sequence),
                }))
            }
            Err(error) => {
                if matches!(error, KernelError::WriterFenced) {
                    self.leadership.fenced().await;
                }
                Err(from_kernel(&error))
            }
        }
    }

    /// Each operation on its own, each reported on its own.
    async fn batch_independently(
        &self,
        context: &SecurityContext,
        decoded: Vec<Decoded<'_>>,
    ) -> Result<Response<pb::BatchResponse>, Status> {
        let mut results = Vec::with_capacity(decoded.len());
        let mut sequence = None;
        for operation in &decoded {
            match self
                .autocommit(context, operation.as_write(self.limits.max_returned_rows))
                .await
            {
                Ok((written, token)) => {
                    if let Some(token) = token {
                        // The last one that committed, so a caller can read
                        // its own writes. Later ones overwrite earlier, which
                        // is what "last" means and is monotonic because the
                        // writer is.
                        sequence = Some(token.sequence());
                    }
                    results.push(pb::BatchResult {
                        of: Some(pb::batch_result::Of::Ok(returning(
                            written,
                            None,
                            operation.returning(),
                            operation.table(),
                        ))),
                    });
                }
                // A failure is a result, not the end of the batch. That is
                // the whole difference from ALL_OR_NOTHING, and it is why
                // `results` exists at all.
                Err(status) => results.push(pb::BatchResult {
                    of: Some(pb::batch_result::Of::Error(pb::BatchError {
                        code: status.code() as i32,
                        message: status.message().to_owned(),
                        reason: reason_of(&status),
                        // The whole blob, not a re-encoding of part of it: a
                        // client decodes a batched refusal with the same
                        // function it uses for a lone one, so the two cannot
                        // come to disagree.
                        details: status.details().to_vec(),
                    })),
                }),
            }
        }
        Ok(Response::new(pb::BatchResponse { results, sequence }))
    }

    /// Apply a batch, the way its `atomicity` says to.
    ///
    /// The two paths share the decoding and differ in everything after it,
    /// which is the point: an independent batch is N autocommits with N
    /// results, and an atomic one is one transaction with none. Writing them
    /// as one loop with a flag was the first attempt and it produced a
    /// function whose every other line was `if atomic`, which is the shape
    /// that lets one path quietly acquire the other's behaviour.
    async fn run_batch(
        &self,
        context: &SecurityContext,
        request: pb::BatchRequest,
        atomicity: Atomicity,
    ) -> Result<Response<pb::BatchResponse>, Status> {
        // Decoded up front, all of it, before anything is written. A batch
        // that fails to decode its ninth operation must not have applied its
        // first eight — under `INDEPENDENT` that is exactly what a lazy decode
        // would do, and the caller would see a malformed-request error next to
        // eight rows that had already landed.
        let mut decoded = Vec::with_capacity(request.operations.len());
        for (at, operation) in request.operations.iter().enumerate() {
            decoded.push(self.decode_operation(context, operation, at)?);
        }

        match atomicity {
            Atomicity::AllOrNothing => {
                self.batch_atomically(context, decoded, &request.transaction)
                    .await
            }
            Atomicity::Independent => {
                if !request.transaction.is_empty() {
                    return Err(Status::new(
                        Code::InvalidArgument,
                        "an INDEPENDENT batch cannot run inside a transaction: \
                         \"independent operations, all of which roll back together\" is two \
                         contradictory requests, and the caller means one of them",
                    ));
                }
                self.batch_independently(context, decoded).await
            }
        }
    }

    /// The ceiling a predicate write's match is held to.
    ///
    /// `None` when the caller did not ask for its rows back, whatever the
    /// configured limit is: the limit is on the *answer*, and a caller that
    /// wants a million rows gone and does not want to see them is asking for
    /// something this node can deliver.
    fn returnable(&self, returning: bool) -> Option<usize> {
        returning.then_some(self.limits.max_returned_rows).flatten()
    }

    async fn autocommit(
        &self,
        context: &SecurityContext,
        write: Write<'_>,
    ) -> Result<(Written, Option<ReadToken>), Status> {
        let writer = self.leader()?;
        // Borrowed, not moved: the closure is `Fn` because it runs once per
        // retry, so each attempt's future takes a reference rather than the
        // batch itself.
        let write = &write;
        let outcome = writer
            .transact_boxed_tracked(move |transaction| {
                Box::pin(async move { write.apply(transaction, context).await })
            })
            .await;

        match outcome {
            Ok(outcome) => {
                // After the transaction committed, never before: a write that
                // conflicted and was retried applies more than once and
                // commits once, and counting attempts would report a number no
                // row ever had.
                if let Some(observer) = &self.writes {
                    let (kind, table) = write.labels();
                    observer.wrote(kind, table.name(), outcome.0.affected);
                }
                Ok(outcome)
            }
            Err(error) => {
                if matches!(error, KernelError::WriterFenced) {
                    self.leadership.fenced().await;
                }
                Err(from_kernel(&error))
            }
        }
    }
}

/// One single-statement write.
///
/// Exists because a closure cannot be used here; see [`Head::autocommit`].
enum Write<'a> {
    Insert {
        table: &'a TableDef,
        rows: &'a [Row],
        /// Replace a row already at the key rather than refusing it.
        upsert: bool,
    },
    Update {
        table: &'a TableDef,
        rows: &'a [Row],
        /// The rows as the caller last saw them, or empty for an unconditional
        /// update. Checked to be either empty or exactly as long as `rows`
        /// before this is built, so the apply below can zip without a length
        /// check it would have no sensible error for.
        expected: &'a [Row],
    },
    Delete {
        table: &'a TableDef,
        keys: &'a [Vec<Value>],
        /// The rows as the caller last saw them, or empty for an unconditional
        /// delete. Checked to be either empty or exactly as long as `keys`
        /// before this is built.
        expected: &'a [Row],
    },
    /// Delete every row a predicate selects. One statement, not a query and a
    /// round trip per key.
    DeleteWhere {
        table: &'a TableDef,
        predicate: &'a Expr,
        /// The ceiling on the match, or `None`. Set only when the caller asked
        /// for the rows back: the write itself is not what is being bounded,
        /// the response is.
        at_most: Option<usize>,
    },
    /// Erase, for good, every row a soft delete retired before an instant.
    PurgeDeleted {
        table: &'a TableDef,
        /// Seconds since the epoch; strictly before.
        before: i64,
        /// The ceiling on the match, or `None`. Unlike the two predicate
        /// writes, this bounds the *write* rather than the response: a purge
        /// answers with a count and never with rows, so the only thing a
        /// ceiling can protect here is the data.
        at_most: Option<usize>,
    },
    /// Assign to columns of every row a predicate selects.
    UpdateWhere {
        table: &'a TableDef,
        predicate: &'a Expr,
        assignments: &'a [(Ordinal, Scalar)],
        at_most: Option<usize>,
    },
}

/// What a write did: how many rows, and which ones when the caller asked.
///
/// The rows are carried even when `returning` was not set, and dropped at the
/// handler rather than here. The kernel has them either way — every row a
/// predicate write touches had to be read to be written — so not collecting
/// them would save nothing, and a second shape for the no-returning case would
/// be two paths to keep in agreement for no gain.
struct Written {
    affected: u64,
    rows: Vec<Row>,
}

impl Write<'_> {
    /// This statement's name and its table, for a [`WriteObserver`]'s labels.
    ///
    /// `&'static str` from a match rather than anything derived: a label has
    /// to be bounded, and the compiler refusing to build until a new variant
    /// is named here is the cheapest way to keep it so.
    const fn labels(&self) -> (&'static str, &TableDef) {
        match self {
            Self::Insert {
                table,
                upsert: false,
                ..
            } => ("insert", table),
            Self::Insert {
                table,
                upsert: true,
                ..
            } => ("upsert", table),
            Self::Update { table, .. } => ("update", table),
            Self::Delete { table, .. } => ("delete", table),
            Self::DeleteWhere { table, .. } => ("delete_where", table),
            Self::UpdateWhere { table, .. } => ("update_where", table),
            Self::PurgeDeleted { table, .. } => ("purge_deleted", table),
        }
    }

    /// Apply it, returning how many rows it acted on and which they were.
    async fn apply(
        &self,
        transaction: &RecordTransaction<'_>,
        context: &SecurityContext,
    ) -> Result<Written, KernelError> {
        let touched = |rows: Vec<Row>| Written {
            affected: rows.len() as u64,
            rows,
        };
        let counted = |affected: u64| Written {
            affected,
            rows: Vec::new(),
        };
        match self {
            // `insert_many` overlaps the duplicate-key reads across the batch,
            // so a thousand rows cost one wave of round trips rather than a
            // thousand.
            Self::Insert {
                table,
                rows,
                upsert: false,
            } => transaction
                .insert_many(context, table, rows)
                .await
                .map(|()| counted(rows.len() as u64)),
            Self::Insert {
                table,
                rows,
                upsert: true,
            } => transaction
                .upsert_many(context, table, rows)
                .await
                .map(|()| counted(rows.len() as u64)),
            // `update_many` reads whether each row exists in one wave, the
            // same as `insert_many`, and refuses the whole batch if any of
            // them is missing rather than applying a prefix.
            Self::Update {
                table,
                rows,
                expected,
            } => {
                if expected.is_empty() {
                    transaction.update_many(context, table, rows).await?;
                } else {
                    // A conditional update is a row at a time, and there is no
                    // `update_many_if_unchanged` to reach for. Batching it
                    // would mean `update_many` taking a parallel slice of
                    // expected rows and deciding what to do when one of them
                    // fails half way through the wave — and the answer is
                    // "refuse the statement", which is what the `?` here
                    // already does, one round trip per row later. The caller
                    // who wants the wave sends an unconditional update; the
                    // caller who wants the check pays for it. Recorded rather
                    // than assumed away.
                    for (row, was) in rows.iter().zip(expected.iter()) {
                        transaction
                            .update_if_unchanged(context, table, row, was)
                            .await?;
                    }
                }
                Ok(counted(rows.len() as u64))
            }
            Self::Delete {
                table,
                keys,
                expected,
            } if !expected.is_empty() => {
                // A conditional delete is not counted the way a plain one is.
                // `delete` returns `false` for an absent row and the count
                // below skips it; `delete_if_unchanged` refuses instead, so
                // every key that got this far was there and `affected` is the
                // arity. Reporting a count that could be less than the keys
                // sent would be reporting a state this call already refused.
                for (key, was) in keys.iter().zip(expected.iter()) {
                    transaction
                        .delete_if_unchanged(context, table, key, was)
                        .await?;
                }
                Ok(counted(keys.len() as u64))
            }
            Self::Delete { table, keys, .. } => {
                let mut affected = 0;
                // No `delete_many`, and not for want of noticing: a
                // delete walks a foreign-key closure, and two keys in one
                // batch can reach the same doomed row by different paths. A
                // batch would have to union those closures before writing
                // anything, which is a different piece of work from
                // `update_many`'s one wave of reads — not the same change with
                // a different name.
                for key in *keys {
                    // A row the policy hides deletes as absent, so the count
                    // cannot be used to probe for one.
                    if transaction.delete(context, table, key).await? {
                        affected += 1;
                    }
                }
                Ok(counted(affected))
            }
            Self::DeleteWhere {
                table,
                predicate,
                at_most,
            } => transaction
                .delete_where(context, table, (*predicate).clone(), *at_most)
                .await
                .map(touched),
            Self::PurgeDeleted {
                table,
                before,
                at_most,
            } => transaction
                .purge_deleted(context, table, *before, *at_most)
                .await
                .map(counted),
            Self::UpdateWhere {
                table,
                predicate,
                assignments,
                at_most,
            } => transaction
                .update_where(context, table, (*predicate).clone(), assignments, *at_most)
                .await
                .map(touched),
        }
    }
}

/// One batch operation, decoded and authorized, ready to apply.
///
/// Deliberately the same five shapes as [`Write`], plus the two fields a
/// predicate write needs to build its response. It is not `Write` itself
/// because `Write` borrows its rows from the request and a batch owns them —
/// the request is consumed before the first write lands, so that a batch which
/// fails to decode its last operation has applied none of the others.
enum Decoded<'a> {
    Insert {
        table: &'a TableDef,
        rows: Vec<Row>,
        upsert: bool,
    },
    Update {
        table: &'a TableDef,
        rows: Vec<Row>,
        expected: Vec<Row>,
    },
    Delete {
        table: &'a TableDef,
        keys: Vec<Vec<Value>>,
        expected: Vec<Row>,
    },
    DeleteWhere {
        table: &'a TableDef,
        predicate: Expr,
        returning: bool,
    },
    UpdateWhere {
        table: &'a TableDef,
        predicate: Expr,
        assignments: Vec<(Ordinal, Scalar)>,
        returning: bool,
    },
}

impl<'a> Decoded<'a> {
    /// The borrowed form [`Write::apply`] takes.
    ///
    /// Borrowing rather than converting, so the two paths run the identical
    /// code a lone RPC runs. A second `apply` for batches is the way a batched
    /// insert and a lone one come to disagree about, say, whether `upsert`
    /// replaces.
    fn as_write(&'a self, at_most: Option<usize>) -> Write<'a> {
        match self {
            Self::Insert {
                table,
                rows,
                upsert,
            } => Write::Insert {
                table,
                rows,
                upsert: *upsert,
            },
            Self::Update {
                table,
                rows,
                expected,
            } => Write::Update {
                table,
                rows,
                expected,
            },
            Self::Delete {
                table,
                keys,
                expected,
            } => Write::Delete {
                table,
                keys,
                expected,
            },
            Self::DeleteWhere {
                table, predicate, ..
            } => Write::DeleteWhere {
                table,
                predicate,
                at_most: self.at_most(at_most),
            },
            Self::UpdateWhere {
                table,
                predicate,
                assignments,
                ..
            } => Write::UpdateWhere {
                table,
                predicate,
                assignments,
                at_most: self.at_most(at_most),
            },
        }
    }

    /// Apply it inside a transaction the caller already has.
    ///
    /// Answers the statement's label, its table and how many rows it touched,
    /// which is what an atomic batch needs to tell a [`WriteObserver`] once the
    /// transaction it ran in has committed. The rows themselves are dropped:
    /// an atomic batch reports no per-operation results, which is the whole
    /// difference from the independent one.
    async fn apply(
        &self,
        transaction: &RecordTransaction<'_>,
        context: &SecurityContext,
        at_most: Option<usize>,
    ) -> Result<(&'static str, String, u64), KernelError> {
        let write = self.as_write(at_most);
        let (kind, table) = write.labels();
        let name = table.name().to_owned();
        write
            .apply(transaction, context)
            .await
            .map(|written| (kind, name, written.affected))
    }

    /// The ceiling this operation's match is held to, given the node's.
    ///
    /// `None` unless the operation asked for its rows back: a batched write
    /// that did not is bounded by nothing here, the same as a lone one.
    fn at_most(&self, configured: Option<usize>) -> Option<usize> {
        self.returning().then_some(configured).flatten()
    }

    fn table(&self) -> &'a TableDef {
        match self {
            Self::Insert { table, .. }
            | Self::Update { table, .. }
            | Self::Delete { table, .. }
            | Self::DeleteWhere { table, .. }
            | Self::UpdateWhere { table, .. } => table,
        }
    }

    /// Whether this operation asked for its rows back. Only the two predicate
    /// writes can, which is why the other three answer `false` rather than
    /// carrying a field that is always unset.
    fn returning(&self) -> bool {
        match self {
            Self::DeleteWhere { returning, .. } | Self::UpdateWhere { returning, .. } => *returning,
            _ => false,
        }
    }
}

/// Which guarantee a batch was asked for.
///
/// The wire enum, narrowed to the two that mean something — the unspecified
/// case is refused at the handler and never reaches here, so this type cannot
/// represent it and no code below has to consider it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Atomicity {
    Independent,
    AllOrNothing,
}

/// A write's response, with the rows only if the caller asked for them.
///
/// Dropped here rather than never collected: the kernel has them either way,
/// because every row a predicate write touches had to be read to be written.
/// What `returning` saves is the encoding and the bytes on the wire, which for
/// a large delete is the whole of the cost.
fn returning(
    written: Written,
    sequence: Option<u64>,
    wanted: bool,
    table: &TableDef,
) -> pb::WriteResponse {
    pb::WriteResponse {
        sequence,
        affected: written.affected,
        rows: if wanted {
            let stored = table.columns().len();
            written
                .rows
                .iter()
                .map(|row| row_to_proto_split(row, stored))
                .collect()
        } else {
            Vec::new()
        },
    }
}

/// A predicate write's filter. Absent means every row the caller can see.
///
/// `Expr::True` rather than a refusal: `DELETE FROM t` with no `WHERE` is a
/// real statement, and a protocol cannot tell it from the mistake it resembles.
fn predicate_from_proto(filter: Option<&pb::Expr>, table: &TableDef) -> Result<Expr, Status> {
    match filter {
        None => Ok(Expr::True),
        Some(filter) => {
            let space = Space::input(table, 0, 0);
            expr_named(&space, filter, "the filter")
        }
    }
}

/// An update's rows and the rows it is conditional on.
///
/// `expected` is empty for an ordinary update and otherwise exactly as long as
/// `rows`. The arity is checked *here*, before anything is written, for the
/// same reason the rest of the decoding is: a batch whose third operation is
/// malformed must apply none of the first two.
///
/// The alternative — pairing them up in the apply and stopping at the shorter
/// — turns a caller's mistake into a silent partial condition: send five rows
/// and three expectations and two rows are written unguarded, which is exactly
/// the lost update the field exists to catch. `InvalidArgument` says so
/// instead.
fn update_rows(rows: &[pb::Row], expected: &[pb::Row]) -> Result<(Vec<Row>, Vec<Row>), Status> {
    let rows = rows
        .iter()
        .map(row_from_proto)
        .collect::<Result<Vec<Row>, Status>>()?;
    if !expected.is_empty() && expected.len() != rows.len() {
        return Err(Status::invalid_argument(format!(
            "`expected` must be empty or name one row per update; \
             got {} row(s) and {} expected",
            rows.len(),
            expected.len()
        )));
    }
    let expected = expected
        .iter()
        .map(row_from_proto)
        .collect::<Result<Vec<Row>, Status>>()?;
    Ok((rows, expected))
}

/// A delete's keys and the rows it is conditional on.
///
/// `expected` is empty for an ordinary delete and otherwise exactly as long as
/// `primary_keys`. Checked here, before anything is written, for the reason
/// [`update_rows`] gives: a short `expected` would leave the keys past its end
/// deleted unconditionally, which is the mistake the field exists to catch.
fn delete_keys(
    primary_keys: &[pb::Row],
    expected: &[pb::Row],
    table: &TableDef,
) -> Result<(Vec<Vec<Value>>, Vec<Row>), Status> {
    // Checked against the table's key rather than encoded and looked up: a key
    // of the wrong arity or the wrong integer width used to delete nothing and
    // report `affected: 0`, which is also what a key that was never there
    // reports, and what a key the caller's policy hides reports.
    let keys = primary_keys
        .iter()
        .map(|key| primary_key_from_proto(key, table))
        .collect::<Result<Vec<Vec<Value>>, Status>>()?;
    if !expected.is_empty() && expected.len() != keys.len() {
        return Err(Status::invalid_argument(format!(
            "`expected` must be empty or name one row per key; \
             got {} key(s) and {} expected",
            keys.len(),
            expected.len()
        )));
    }
    let expected = expected
        .iter()
        .map(row_from_proto)
        .collect::<Result<Vec<Row>, Status>>()?;
    Ok((keys, expected))
}

/// The rows of a query, in batches.
type RowStream = Pin<Box<dyn futures::Stream<Item = Result<pb::QueryResponse, Status>> + Send>>;

/// Where a read went, in wire form.
///
/// The sequence is read off the same store that opened the view, so the pair is
/// one view's account of itself rather than two answers stitched together.
fn served_by(store: &dyn KvReadStore) -> pb::ServedBy {
    pb::ServedBy {
        replica: store.replica_name().to_owned(),
        sequence: store.visible_sequence().unwrap_or(0),
    }
}

/// What a read served inside a transaction reports.
fn in_transaction() -> pb::ServedBy {
    pb::ServedBy {
        replica: IN_TRANSACTION.to_owned(),
        sequence: 0,
    }
}

#[tonic::async_trait]
impl<S: KvStore + KvReadStore> Records for Head<S> {
    async fn begin(
        &self,
        request: Request<pb::BeginRequest>,
    ) -> Result<Response<pb::BeginResponse>, Status> {
        let context = self.context(&request)?;
        let writer = Arc::clone(self.leader()?);
        let id = self
            .sessions
            .begin(
                writer,
                Arc::clone(&self.leadership),
                context.principal().clone(),
                self.writes.clone(),
            )
            .await?;
        Ok(Response::new(pb::BeginResponse {
            transaction: id.to_string(),
        }))
    }

    async fn commit(
        &self,
        request: Request<pb::CommitRequest>,
    ) -> Result<Response<pb::CommitResponse>, Status> {
        let context = self.context(&request)?;
        let token = self
            .sessions
            .commit(&request.get_ref().transaction, &context)
            .await?;
        Ok(Response::new(pb::CommitResponse {
            sequence: token.map(ReadToken::sequence),
        }))
    }

    async fn rollback(
        &self,
        request: Request<pb::RollbackRequest>,
    ) -> Result<Response<pb::RollbackResponse>, Status> {
        let context = self.context(&request)?;
        self.sessions
            .rollback(&request.get_ref().transaction, &context)
            .await?;
        Ok(Response::new(pb::RollbackResponse {}))
    }

    async fn insert(
        &self,
        request: Request<pb::InsertRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.authorized_table(&context, &request.table, Action::Insert)?;
        // Before the rows are read, not after: a declaration that disagrees
        // makes every value in every row positionally wrong, and there is
        // nothing to be gained by decoding them first.
        fingerprint::check(table, request.schema.as_ref())?;
        let rows = request
            .rows
            .iter()
            .map(row_from_proto)
            .collect::<Result<Vec<Row>, Status>>()?;
        let upsert = request.upsert;

        if request.transaction.is_empty() {
            let (affected, token) = self
                .autocommit(
                    &context,
                    Write::Insert {
                        table,
                        rows: &rows,
                        upsert,
                    },
                )
                .await?;
            return Ok(Response::new(pb::WriteResponse {
                sequence: token.map(ReadToken::sequence),
                affected: affected.affected,
                rows: Vec::new(),
            }));
        }

        let affected = self
            .sessions
            .insert(&request.transaction, &context, table.id(), rows, upsert)
            .await?;
        Ok(Response::new(pb::WriteResponse {
            sequence: None,
            affected,
            rows: Vec::new(),
        }))
    }

    async fn update(
        &self,
        request: Request<pb::UpdateRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.authorized_table(&context, &request.table, Action::Update)?;
        fingerprint::check(table, request.schema.as_ref())?;
        let (rows, expected) = update_rows(&request.rows, &request.expected)?;

        if request.transaction.is_empty() {
            let (affected, token) = self
                .autocommit(
                    &context,
                    Write::Update {
                        table,
                        rows: &rows,
                        expected: &expected,
                    },
                )
                .await?;
            return Ok(Response::new(pb::WriteResponse {
                sequence: token.map(ReadToken::sequence),
                affected: affected.affected,
                rows: Vec::new(),
            }));
        }

        let affected = self
            .sessions
            .update(&request.transaction, &context, table.id(), rows, expected)
            .await?;
        Ok(Response::new(pb::WriteResponse {
            sequence: None,
            affected,
            rows: Vec::new(),
        }))
    }

    async fn delete(
        &self,
        request: Request<pb::DeleteRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.authorized_table(&context, &request.table, Action::Delete)?;
        fingerprint::check(table, request.schema.as_ref())?;
        // Checked against the table's key rather than encoded and looked up: a
        // key of the wrong arity or the wrong integer width used to delete
        // nothing and report `affected: 0`, which is also what a key that was
        // never there reports, and what a key the caller's policy hides
        // reports.
        let (keys, expected) = delete_keys(&request.primary_keys, &request.expected, table)?;

        if request.transaction.is_empty() {
            let (affected, token) = self
                .autocommit(
                    &context,
                    Write::Delete {
                        table,
                        keys: &keys,
                        expected: &expected,
                    },
                )
                .await?;
            return Ok(Response::new(pb::WriteResponse {
                sequence: token.map(ReadToken::sequence),
                affected: affected.affected,
                rows: Vec::new(),
            }));
        }

        let affected = self
            .sessions
            .delete(&request.transaction, &context, table.id(), keys, expected)
            .await?;
        Ok(Response::new(pb::WriteResponse {
            sequence: None,
            affected,
            rows: Vec::new(),
        }))
    }

    async fn delete_where(
        &self,
        request: Request<pb::DeleteWhereRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        // Defence in depth, and correctly unobservable: `delete_where`
        // authorizes `Delete` again in the kernel, so a mutation weakening
        // this to `Read` survives every test here and the delete still fails.
        // It stays because the kernel's check is the one that must not be the
        // only one, not because this one catches something.
        let table = self.authorized_table(&context, &request.table, Action::Delete)?;
        // Before the predicate is resolved: an ordinal in it is only
        // meaningful against a declaration, and one that resolves against the
        // wrong schema resolves perfectly well.
        fingerprint::check(table, request.schema.as_ref())?;
        let predicate = predicate_from_proto(request.filter.as_ref(), table)?;

        let rows = if request.transaction.is_empty() {
            let (written, token) = self
                .autocommit(
                    &context,
                    Write::DeleteWhere {
                        table,
                        predicate: &predicate,
                        at_most: self.returnable(request.returning),
                    },
                )
                .await?;
            return Ok(Response::new(returning(
                written,
                token.map(ReadToken::sequence),
                request.returning,
                table,
            )));
        } else {
            self.sessions
                .delete_where(
                    &request.transaction,
                    &context,
                    table.id(),
                    predicate,
                    self.returnable(request.returning),
                )
                .await?
        };
        Ok(Response::new(returning(
            Written {
                affected: rows.len() as u64,
                rows,
            },
            None,
            request.returning,
            table,
        )))
    }

    async fn purge_deleted(
        &self,
        request: Request<pb::PurgeDeletedRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        // `Delete` here; the kernel additionally authorizes `ReadDeleted`,
        // which is the check that makes a purge stricter than a delete. Not
        // repeated here, because a second statement of it is a second thing to
        // keep in step and the kernel's is the one that must hold.
        //
        // Defence in depth, and **correctly unobservable**, exactly as
        // `delete_where` records three handlers down: `purge_deleted`
        // authorizes `Delete` again in the kernel, so a mutation weakening
        // this line to `Read` survives the whole suite and the purge still
        // fails — `a_caller_who_may_see_retired_rows_still_cannot_erase_them`
        // goes on passing, refused one layer lower. It stays because the
        // kernel's check must not be the only one, not because this one
        // catches anything a test can see.
        let table = self.authorized_table(&context, &request.table, Action::Delete)?;
        fingerprint::check(table, request.schema.as_ref())?;
        // Zero means "no ceiling", which is the proto3 default and so what a
        // client that zeroed the struct means. A caller who wants to purge
        // nothing passes a `before` in the past, not a ceiling of zero.
        let at_most = (request.at_most > 0).then_some(request.at_most as usize);

        if request.transaction.is_empty() {
            let (written, token) = self
                .autocommit(
                    &context,
                    Write::PurgeDeleted {
                        table,
                        before: request.before,
                        at_most,
                    },
                )
                .await?;
            return Ok(Response::new(pb::WriteResponse {
                sequence: token.map(ReadToken::sequence),
                affected: written.affected,
                rows: Vec::new(),
            }));
        }
        let affected = self
            .sessions
            .purge_deleted(
                &request.transaction,
                &context,
                table.id(),
                request.before,
                at_most,
            )
            .await?;
        Ok(Response::new(pb::WriteResponse {
            // No sequence: a write inside a transaction has none until that
            // transaction commits, which is what every other write here does.
            sequence: None,
            affected,
            rows: Vec::new(),
        }))
    }

    async fn update_where(
        &self,
        request: Request<pb::UpdateWhereRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.authorized_table(&context, &request.table, Action::Update)?;
        fingerprint::check(table, request.schema.as_ref())?;
        let predicate = predicate_from_proto(request.filter.as_ref(), table)?;
        let assignments = assignments_from_proto(&request.assignments, table)?;
        // Refused rather than answered with a no-op: "update these rows to
        // nothing" is not a request anybody makes on purpose, and reporting
        // zero rows written would look like a predicate that matched nothing.
        if assignments.is_empty() {
            return Err(Status::new(
                Code::InvalidArgument,
                "an update needs at least one assignment; a predicate write with none would \
                 report zero rows written and look like a predicate that matched nothing",
            ));
        }

        let rows = if request.transaction.is_empty() {
            let (written, token) = self
                .autocommit(
                    &context,
                    Write::UpdateWhere {
                        table,
                        predicate: &predicate,
                        assignments: &assignments,
                        at_most: self.returnable(request.returning),
                    },
                )
                .await?;
            return Ok(Response::new(returning(
                written,
                token.map(ReadToken::sequence),
                request.returning,
                table,
            )));
        } else {
            self.sessions
                .update_where(
                    &request.transaction,
                    &context,
                    table.id(),
                    predicate,
                    assignments,
                    self.returnable(request.returning),
                )
                .await?
        };
        Ok(Response::new(returning(
            Written {
                affected: rows.len() as u64,
                rows,
            },
            None,
            request.returning,
            table,
        )))
    }

    async fn batch(
        &self,
        request: Request<pb::BatchRequest>,
    ) -> Result<Response<pb::BatchResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();

        // Refused, not defaulted. The two guarantees differ only when
        // something fails, so a caller who never said which they wanted finds
        // out on the day it matters. A zeroed request means neither.
        let atomicity = match pb::Atomicity::try_from(request.atomicity) {
            Ok(pb::Atomicity::Independent) => Atomicity::Independent,
            Ok(pb::Atomicity::AllOrNothing) => Atomicity::AllOrNothing,
            _ => {
                return Err(Status::new(
                    Code::InvalidArgument,
                    "a batch must say `atomicity`: INDEPENDENT applies each operation on its \
                     own and reports each separately, ALL_OR_NOTHING applies them in one \
                     transaction and fails the request if any of them does. They differ only \
                     when something fails, which is why neither is the default",
                ));
            }
        };

        // A batch of nothing is a round trip that asks for nothing, and is
        // likelier a caller whose list came out empty than one who meant it.
        if request.operations.is_empty() {
            return Err(Status::new(
                Code::InvalidArgument,
                "a batch needs at least one operation",
            ));
        }
        if let Some(limit) = self.limits.max_batch_operations
            && request.operations.len() > limit
        {
            return Err(Status::new(
                Code::InvalidArgument,
                format!(
                    "a batch may carry at most {limit} operations; this one carries {}",
                    request.operations.len()
                ),
            ));
        }

        self.run_batch(&context, request, atomicity).await
    }

    async fn get(
        &self,
        request: Request<pb::GetRequest>,
    ) -> Result<Response<pb::GetResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.authorized_table(&context, &request.table, Action::Read)?;
        fingerprint::check(table, request.schema.as_ref())?;
        let Some(wire_key) = &request.primary_key else {
            return Err(Status::new(Code::InvalidArgument, "no primary key given"));
        };
        // See `primary_key_from_proto`: without this a malformed key came back
        // as `found: false`, which is the same answer as a legitimate miss and
        // as a row the caller's policy hides — three different facts with one
        // spelling, one of which is a client bug that would never be found.
        let key = primary_key_from_proto(wire_key, table)?;

        if !request.transaction.is_empty() {
            let row = self
                .sessions
                .get(&request.transaction, &context, table.id(), key)
                .await?;
            return Ok(Response::new(pb::GetResponse {
                found: row.is_some(),
                row: row.as_ref().map(row_to_proto),
                served_by: Some(in_transaction()),
            }));
        }

        let freshness = freshness_from_proto(request.freshness.as_ref())?;
        let affinity = Self::affinity(table, &context);
        let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
        let row = view
            .get(&context, table, &key)
            .await
            .map_err(|e| from_kernel(&e))?;

        Ok(Response::new(pb::GetResponse {
            found: row.is_some(),
            row: row.as_ref().map(row_to_proto),
            served_by: Some(served_by),
        }))
    }

    async fn related(
        &self,
        request: Request<pb::RelatedRequest>,
    ) -> Result<Response<pb::RelatedResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();

        // One shape in, one shape out. Internally there is only ever a path:
        // a `relation` request is a path of one, resolved by the same loop, so
        // that the single-level case cannot drift from the multi-level one.
        // Which field was set decides only how the answer is projected.
        let (steps, levelled) = match (request.relation.as_ref(), request.path.is_empty()) {
            (Some(_), false) => {
                return Err(Status::new(
                    Code::InvalidArgument,
                    "both `relation` and `path` are set; they are two ways to say what \
                     to read and this request says two different things. Send one",
                ));
            }
            (Some(relation), true) => (
                vec![pb::RelatedStep {
                    relation: Some(relation.clone()),
                    schema: request.schema,
                }],
                false,
            ),
            (None, false) => (request.path.clone(), true),
            (None, true) => {
                return Err(Status::new(
                    Code::InvalidArgument,
                    "no relation given: set `relation` for one level, or `path` for \
                     several",
                ));
            }
        };

        // Depth before anything else, because the work this bounds is the work
        // done below. One step is one read, and the count arrives in the
        // request rather than in the source, so an unbounded path is a caller
        // choosing how many times this server goes to storage.
        if let Some(limit) = self.limits.max_relation_depth
            && steps.len() > limit
        {
            return Err(Status::new(
                Code::InvalidArgument,
                format!(
                    "a relationship path of {} steps was asked for and the limit is \
                     {limit}; each step is a read, so the depth is how many reads one \
                     request performs. Shorten the path, or raise \
                     `[limits] max_relation_depth`",
                    steps.len()
                ),
            ));
        }

        // Resolve every step against the catalog before reading anything, so
        // that a path which does not compose is refused without having done
        // half of it. A partial answer to a malformed request is worse than a
        // refusal: the caller cannot tell it from a complete one.
        let mut resolved: Vec<ResolvedStep<'_>> = Vec::with_capacity(steps.len());
        for (at, step) in steps.iter().enumerate() {
            let Some(relation) = step.relation.as_ref() else {
                return Err(Status::new(
                    Code::InvalidArgument,
                    format!("step {at} of the path has no relation"),
                ));
            };
            // Authorised before the relation is resolved, for the reason
            // `query` is: `resolve_relation`'s refusals name the child's
            // foreign keys — "`orders` has no foreign key named `x`; it has
            // …" — and the composition errors below name the tables a path
            // touches. Both are schema, to a caller who may hold no grant.
            //
            // `relation.table` rather than the step's `rows_from`, which is
            // only known after resolving. For a `CHILDREN` step they are the
            // same table; for `PARENTS` the child is the *source* and its rows
            // are not read, so this asks for a grant the read itself does not
            // need. Accepted deliberately: naming a relation is asking about
            // the child's foreign keys, and a caller with no grant on the
            // child has no business being told what they are. The per-step
            // `Read` on `rows_from` below still happens and is what protects
            // the rows.
            self.authorized_table(&context, &relation.table, Action::Read)?;
            let this = self.resolve_relation(relation)?;
            if let Some(previous) = resolved.last()
                && previous.rows_from.id() != this.source.id()
            {
                return Err(Status::new(
                    Code::InvalidArgument,
                    format!(
                        "step {at} of the path reads from `{}`, whose keys come from \
                         `{}`, but step {} returned rows of `{}`. Each step's keys are \
                         columns of the rows the step before it returned, so a path \
                         has to compose",
                        this.rows_from.name(),
                        this.source.name(),
                        at - 1,
                        previous.rows_from.name(),
                    ),
                ));
            }
            resolved.push(this);
        }

        // Authorization and the schema claim, per step: every table this will
        // read is checked before any of them is read. A path that is refused
        // at step three must not have returned step one's rows.
        let mut tables: Vec<&TableDef> = Vec::with_capacity(resolved.len());
        for (step, plan) in steps.iter().zip(&resolved) {
            let table = self.authorized_table(&context, plan.rows_from.name(), Action::Read)?;
            fingerprint::check(table, step.schema.as_ref())?;
            tables.push(table);
        }

        let mut keys = distinct_keys(&request.keys)?;

        // No keys, no read. An `IN ()` matches nothing and still pays for a
        // scan, and the caller with no parents wanted no rows.
        //
        // `served_by` is left unset rather than filled in, which is the whole
        // reason this is not a one-line guard: nothing served this, so naming a
        // replica — or claiming `in_transaction` outside a transaction, which
        // the first version of this did — would be reporting a read that did
        // not happen to a caller who may be using that field to track a
        // watermark.
        if keys.is_empty() {
            return Ok(Response::new(pb::RelatedResponse {
                groups: Vec::new(),
                served_by: None,
                warnings: Vec::new(),
                levels: if levelled {
                    empty_levels(&resolved)
                } else {
                    Vec::new()
                },
            }));
        }

        // One view for every level.
        //
        // Taken once rather than per step, and that is a correctness decision
        // rather than a saving: two levels read from two snapshots can show a
        // child whose parent was deleted between them, which is a tree that
        // never existed. Affinity is computed over every table the path
        // touches, which is what `affinity_over` is for.
        let (view, served_by) = if request.transaction.is_empty() {
            let freshness = freshness_from_proto(request.freshness.as_ref())?;
            let affinity = Self::affinity_over(&tables, &context);
            let (view, by) = self.read_view(freshness, affinity.as_ref()).await?;
            (Some(view), by)
        } else {
            (None, in_transaction())
        };
        let served_by = Some(served_by);

        let mut levels: Vec<pb::related_response::Level> = Vec::with_capacity(resolved.len());
        for (at, (plan, table)) in resolved.iter().zip(&tables).enumerate() {
            // A level with no keys left resolves to nothing, and the levels
            // below it resolve to nothing too. Still emitted, empty, so that
            // `levels` stays index-for-index with the request's `path`.
            if keys.is_empty() {
                levels.push(pb::related_response::Level {
                    key_ordinal: plan.source_column.0 as u32,
                    groups: Vec::new(),
                });
                continue;
            }

            let query = Query::all().filter(Expr::In {
                column: plan.match_on,
                values: keys,
            });
            let rows = if let Some(view) = view.as_ref() {
                let mut cursor = view
                    .execute(&context, table, &query)
                    .await
                    .map_err(|e| from_kernel(&e))?;
                let mut rows = Vec::new();
                while let Some(row) = cursor.next().await.map_err(|e| from_kernel(&e))? {
                    rows.push(row);
                }
                drop(cursor);
                rows
            } else {
                self.sessions
                    .query(&request.transaction, &context, table.id(), query)
                    .await?
            };

            // The next level's keys, before the rows are converted: the column
            // that relates *this* level's rows to the one below is the next
            // step's `source_column`, which is a fact about that step rather
            // than this one.
            keys = match resolved.get(at + 1) {
                Some(next) => {
                    let mut values: Vec<Value> = rows
                        .iter()
                        .filter_map(|row| row.values().get(next.source_column.0).cloned())
                        .collect();
                    values.sort();
                    values.dedup();
                    values
                }
                None => Vec::new(),
            };

            // Grouped by the value that related them, so two parents sharing a
            // key share one group rather than each carrying a copy.
            let mut groups: BTreeMap<Value, Vec<pb::Row>> = BTreeMap::new();
            for row in &rows {
                let Some(key) = row.values().get(plan.match_on.0) else {
                    continue;
                };
                groups
                    .entry(key.clone())
                    .or_default()
                    .push(row_to_proto(row));
            }
            levels.push(pb::related_response::Level {
                key_ordinal: plan.source_column.0 as u32,
                groups: groups
                    .into_iter()
                    .map(|(key, rows)| pb::related_response::Group {
                        key: Some(value_to_proto(&key)),
                        rows,
                    })
                    .collect(),
            });
        }

        // Projected back into the shape the request asked in. A `relation`
        // request gets `groups` and no `levels`; a `path` request gets
        // `levels` and no `groups`. Filling both would ship a one-step path's
        // rows twice.
        let (groups, levels) = if levelled {
            (Vec::new(), levels)
        } else {
            let first = levels
                .into_iter()
                .next()
                .map(|l| l.groups)
                .unwrap_or_default();
            (first, Vec::new())
        };

        Ok(Response::new(pb::RelatedResponse {
            groups,
            served_by,
            warnings: Vec::new(),
            levels,
        }))
    }

    type QueryStream = RowStream;

    async fn query(
        &self,
        request: Request<pb::QueryRequest>,
    ) -> Result<Response<Self::QueryStream>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let Some(wire) = request.query else {
            return Err(Status::new(Code::InvalidArgument, "no query given"));
        };
        // Authorised before the query is converted, not only inside the
        // planner. `query_from_proto` resolves a `ColumnRef` against the
        // table's width — "only the server knows how wide each table is", as
        // the proto puts it — and reported the width in the refusal: a caller
        // holding no grant on `users` got "the projection names column 99 of
        // table `users`, which has 4 columns". That is finding 8's disclosure
        // on a path its fix did not cover, and one request rather than a
        // binary search.
        //
        // `Action::Read` because that is what the planner will check; see
        // `authorized_table` on why the action is passed rather than inferred.
        let table = self.authorized_table(&context, &wire.table, Action::Read)?;
        // Kept, not dropped. An ignored index hint used to be reported by
        // `Explain` alone, so a caller whose hint did nothing had to issue a
        // *different* request and trust the planner had decided the same way
        // on it. The first message of the stream is always sent and already
        // carries `served_by`, so there was a header to put this in all along.
        let (query, warnings) = query_from_proto(&wire, table)?;
        if wire.paged {
            crate::convert::check_paged(&query, table)?;
        }
        let stored = table.columns().len();
        let batch_size = self.limits.rows_per_message.max(1);

        if !request.transaction.is_empty() {
            // A transactional read is answered in one go; see `Sessions::query`
            // for why it cannot stream.
            let limit = query.limit;
            let rows = self
                .sessions
                .query(&request.transaction, &context, table.id(), query)
                .await?;
            // A short page proves there is nothing after it. A full one proves
            // nothing either way and gets a cursor anyway: reading one row
            // further to find out would be paid on every page to save one
            // empty request at the end of a sequence most callers never
            // finish. `Page` in the record layer makes the same trade.
            let cursor = match (wire.paged, limit, rows.last()) {
                (true, Some(limit), Some(row)) if rows.len() >= limit => row
                    .primary_key_values(table)
                    .iter()
                    .map(value_to_proto)
                    .collect(),
                _ => Vec::new(),
            };
            return Ok(Response::new(replay(
                rows,
                stored,
                in_transaction(),
                warnings,
                batch_size,
                cursor,
            )));
        }

        let freshness = freshness_from_proto(request.freshness.as_ref())?;
        let affinity = Self::affinity(table, &context);
        let (sender, receiver) = mpsc::channel(2);
        let (started, start) = oneshot::channel();
        let scan = Scan {
            pool: Arc::clone(&self.pool),
            table: table.id(),
            context,
            paged: wire.paged,
            query,
            warnings,
            batch_size,
            freshness,
            affinity,
        };

        // The view and the cursor both borrow the pool, so they live in a task
        // that owns an `Arc` to it. Same reasoning as `session.rs`.
        //
        // Routing happens in there too, which it did not before: naming the
        // replica truthfully means routing and opening the view are one call,
        // and the view cannot outlive the task. Nothing is lost by it. The task
        // reports back on `started` once it has a cursor, and the handler waits
        // for that, so a routing failure — no replica fresh enough, no writer to
        // fall back to — is still the status of the call. Without `started`
        // every failure before the first row (an access denial, a fenced
        // writer, a predicate the planner refuses) would arrive as the first
        // item of a stream that had already been accepted, which a client reads
        // as a request that succeeded and then broke.
        tokio::spawn(async move {
            scan.run(&sender, started).await;
        });

        match start.await {
            Ok(Ok(())) => Ok(Response::new(Box::pin(ReceiverStream::new(receiver)))),
            Ok(Err(status)) => Err(status),
            Err(_) => Err(Status::new(
                Code::Internal,
                "the query task ended before it started",
            )),
        }
    }

    async fn explain(
        &self,
        request: Request<pb::ExplainRequest>,
    ) -> Result<Response<pb::ExplainResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let Some(wire) = request.query else {
            return Err(Status::new(Code::InvalidArgument, "no query given"));
        };
        // `Action::Explain` rather than `Read`, for the same reason the
        // conversion below needs guarding at all: explaining is its own action
        // (finding 3), and this must check what the kernel checks first or it
        // refuses something the kernel would allow. A caller holding `Explain`
        // and not `Read` still gets no further — the planner checks `Read`
        // after — so this is a strict narrowing of who reaches the converter.
        let table = self.authorized_table(&context, &wire.table, Action::Explain)?;
        let (query, warnings) = query_from_proto(&wire, table)?;

        if !request.transaction.is_empty() {
            let explanation = self
                .sessions
                .explain(&request.transaction, &context, table.id(), query)
                .await?;
            return Ok(Response::new(explanation_to_proto(
                &explanation,
                warnings,
                Some(in_transaction()),
            )));
        }

        let freshness = freshness_from_proto(request.freshness.as_ref())?;
        let affinity = Self::affinity(table, &context);
        let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
        let explanation = view
            .explain(&context, table, &query)
            .map_err(|e| from_kernel(&e))?;

        Ok(Response::new(explanation_to_proto(
            &explanation,
            warnings,
            Some(served_by),
        )))
    }

    type JoinStream = JoinedStream;

    /// A join or a chain: one request shape, two kernel paths.
    ///
    /// Every input is planned and read through the same secured path a
    /// single-table query uses, so each is authorised and each carries its own
    /// row filter. Nothing here reads a row; it reads cursors that have already
    /// applied their policies, which is why a join cannot see what a query
    /// could not.
    ///
    /// One snapshot serves every input. That is what makes `served_by`
    /// singular and honest: the whole result is read at one sequence from one
    /// view, rather than each table being read wherever it happened to be
    /// fresh enough.
    async fn join(
        &self,
        request: Request<pb::JoinRequest>,
    ) -> Result<Response<Self::JoinStream>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let Some(wire) = request.join else {
            return Err(Status::new(Code::InvalidArgument, "no join given"));
        };
        // The warnings — an unusable hint on any input, a build limit that was
        // clamped — travel on the first message of the stream, beside
        // `served_by`. They used to be dropped here on the grounds that a
        // stream has no header; it has one, and it is the message that is
        // always sent even when the result is empty.
        self.authorize_join_inputs(&context, &wire, Action::Read)?;
        let (tables, read, warnings) = join_from_proto(&wire, self.pool.catalog())?;
        let definitions = self.definitions(&tables)?;
        let stored: Vec<usize> = definitions
            .iter()
            .map(|table| table.columns().len())
            .collect();
        let batch_size = self.limits.rows_per_message.max(1);

        // The page size is in **first-input rows**, which is what `limit`
        // counts once `paged` is set. Read off the request rather than off the
        // converted `read`, because paging moves that limit onto the first
        // input and the join stops carrying it.
        let paged = wire.paged;
        let page_size = wire.limit.map(|limit| limit as usize);

        if !request.transaction.is_empty() {
            let page = self
                .sessions
                .multi_read(&request.transaction, &context, tables, read)
                .await?;
            let cursor = match (paged, page_size, page.page) {
                (true, Some(size), (Some(key), read)) if read >= size => {
                    key.iter().map(value_to_proto).collect()
                }
                _ => Vec::new(),
            };
            return Ok(Response::new(replay_joined(
                page.rows,
                &stored,
                in_transaction(),
                warnings,
                batch_size,
                cursor,
            )));
        }

        let freshness = freshness_from_proto(request.freshness.as_ref())?;
        let affinity = Self::affinity_over(&definitions, &context);
        let (sender, receiver) = mpsc::channel(2);
        let (started, start) = oneshot::channel();
        let scan = MultiScan {
            pool: Arc::clone(&self.pool),
            tables,
            context,
            read,
            warnings,
            batch_size,
            freshness,
            affinity,
            paged,
            page_size,
        };

        // Same shape as `Scan`, and for the same reasons: the cursor borrows
        // the view, the view borrows the pool, and the routing decision has to
        // come out of the same call that opens the view. `started` keeps a
        // failure before the first row — an access denial, a nested loop that
        // cannot preserve unmatched rows, a build side too large — the status
        // of the call rather than the first item of a stream the client has
        // already been told succeeded.
        tokio::spawn(async move {
            scan.run(&sender, started).await;
        });

        match start.await {
            Ok(Ok(())) => Ok(Response::new(Box::pin(ReceiverStream::new(receiver)))),
            Ok(Err(status)) => Err(status),
            Err(_) => Err(Status::new(
                Code::Internal,
                "the join task ended before it started",
            )),
        }
    }

    type AggregateStream = GroupStream;

    /// Aggregates over one table, optionally per group.
    ///
    /// Not streamed from a cursor, because the kernel does not have one to
    /// stream: grouping folds every matching row before any group is final, so
    /// the result is a vector by the time it exists. It is still sent in
    /// batches, so a query with a million groups is not one message.
    async fn aggregate(
        &self,
        request: Request<pb::AggregateRequest>,
    ) -> Result<Response<Self::AggregateStream>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let Some(wire) = request.aggregate else {
            return Err(Status::new(Code::InvalidArgument, "no aggregate given"));
        };
        self.authorize_aggregate_inputs(&context, &wire, Action::Read)?;
        // Carried on the first message, as on `Query` and `Join`.
        let (read, warnings) = aggregate_from_proto_query(&wire, self.pool.catalog())?;
        let batch_size = self.limits.rows_per_message.max(1);

        if !request.transaction.is_empty() {
            let groups = self
                .sessions
                .aggregate(&request.transaction, &context, read)
                .await?;
            return Ok(Response::new(replay_groups(
                groups,
                in_transaction(),
                warnings,
                batch_size,
            )));
        }

        let freshness = freshness_from_proto(request.freshness.as_ref())?;
        let grouping = read.grouping();
        match &read.source {
            GroupedSource::Table { table, query } => {
                let table = self.definition(*table)?;
                let affinity = Self::affinity(table, &context);
                let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
                let groups = view
                    .grouped(&context, table, query, &grouping)
                    .await
                    .map_err(|e| from_kernel(&e))?;
                Ok(Response::new(replay_groups(
                    groups, served_by, warnings, batch_size,
                )))
            }
            GroupedSource::Join { left, right, join } => {
                let left = self.definition(*left)?;
                let right = self.definition(*right)?;
                // Affinity from the left side, as a plain join does: both
                // sides are planned and secured separately, and the left is
                // the one the planner reads first.
                let affinity = Self::affinity(left, &context);
                let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
                let groups = view
                    .group_by_join(&context, left, right, join, &grouping)
                    .await
                    .map_err(|e| from_kernel(&e))?;
                Ok(Response::new(replay_groups(
                    groups, served_by, warnings, batch_size,
                )))
            }
            GroupedSource::Chain { tables, chain } => {
                let definitions = self.definitions(tables)?;
                // Affinity from the first table, as a plain chain does: it is
                // the one the planner reads first, and every later step is
                // secured on its own.
                let first = *definitions
                    .first()
                    .ok_or_else(|| Status::new(Code::Internal, "a chain with no tables"))?;
                let affinity = Self::affinity(first, &context);
                let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
                let groups = view
                    .group_by_chain(&context, &definitions, chain, &grouping)
                    .await
                    .map_err(|e| from_kernel(&e))?;
                Ok(Response::new(replay_groups(
                    groups, served_by, warnings, batch_size,
                )))
            }
        }
    }

    async fn explain_join(
        &self,
        request: Request<pb::ExplainJoinRequest>,
    ) -> Result<Response<pb::JoinExplainResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let Some(wire) = request.join else {
            return Err(Status::new(Code::InvalidArgument, "no join given"));
        };
        // `Explain` rather than `Read`, for the reason the single-table
        // `explain` gives: it is what the kernel checks first, so a caller
        // holding neither is told the same thing either way round.
        self.authorize_join_inputs(&context, &wire, Action::Explain)?;
        let (tables, read, warnings) = join_from_proto(&wire, self.pool.catalog())?;
        let definitions = self.definitions(&tables)?;

        if !request.transaction.is_empty() {
            let explanation = self
                .sessions
                .explain_multi(&request.transaction, &context, tables, read.clone())
                .await?;
            return Ok(Response::new(multi_explanation_to_proto(
                &explanation,
                &definitions,
                &read,
                warnings,
                in_transaction(),
            )));
        }

        let freshness = freshness_from_proto(request.freshness.as_ref())?;
        let affinity = Self::affinity_over(&definitions, &context);
        let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
        let explanation = match &read {
            MultiRead::Join(join) => two_tables(&definitions).and_then(|(left, right)| {
                view.explain_join(&context, left, right, join)
                    .map(|plan| MultiExplanation::Join(Box::new(plan)))
            }),
            MultiRead::Chain(chain) => view
                .explain_chain(&context, &definitions, chain)
                .map(|plan| MultiExplanation::Chain(Box::new(plan))),
        }
        .map_err(|e| from_kernel(&e))?;

        Ok(Response::new(multi_explanation_to_proto(
            &explanation,
            &definitions,
            &read,
            warnings,
            served_by,
        )))
    }

    /// The plan a grouped read would run under.
    ///
    /// Separate from `Explain` and `ExplainJoin` because grouping changes the
    /// plan: each input's projection becomes the group keys plus what the
    /// aggregates read. Explaining the underlying `Query` or `JoinQuery`
    /// therefore describes a read the `Aggregate` would not make, and the
    /// difference is precisely the one worth asking about — whether the
    /// grouped read goes index-only.
    async fn explain_aggregate(
        &self,
        request: Request<pb::ExplainAggregateRequest>,
    ) -> Result<Response<pb::AggregateExplainResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let Some(wire) = request.aggregate else {
            return Err(Status::new(Code::InvalidArgument, "no aggregate given"));
        };
        self.authorize_aggregate_inputs(&context, &wire, Action::Explain)?;
        let (read, warnings) = aggregate_from_proto_query(&wire, self.pool.catalog())?;
        let grouping = read.grouping();

        // The tables in request order, which is the order `chain_plan_to_proto`
        // walks. Resolved here rather than per branch because the conversion
        // needs them whichever source this is.
        let definitions = match &read.source {
            GroupedSource::Table { table, .. } => vec![self.definition(*table)?],
            GroupedSource::Join { left, right, .. } => {
                vec![self.definition(*left)?, self.definition(*right)?]
            }
            GroupedSource::Chain { tables, .. } => self.definitions(tables)?,
        };

        if !request.transaction.is_empty() {
            let explanation = self
                .sessions
                .explain_aggregate(&request.transaction, &context, read)
                .await?;
            return Ok(Response::new(grouped_explanation_to_proto(
                &explanation,
                &definitions,
                &grouping,
                warnings,
                in_transaction(),
            )));
        }

        let freshness = freshness_from_proto(request.freshness.as_ref())?;
        let affinity = Self::affinity_over(&definitions, &context);
        let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
        let explanation = match &read.source {
            GroupedSource::Table { table, query } => {
                let table = self.definition(*table)?;
                view.explain_grouped(&context, table, query, &grouping)
                    .map(|plan| GroupedExplanation::Table(Box::new(plan)))
            }
            GroupedSource::Join { left, right, join } => {
                let left = self.definition(*left)?;
                let right = self.definition(*right)?;
                view.explain_grouped_join(&context, left, right, join, &grouping)
                    .map(|plan| GroupedExplanation::Join(Box::new(plan)))
            }
            GroupedSource::Chain { chain, .. } => view
                .explain_grouped_chain(&context, &definitions, chain, &grouping)
                .map(|(narrowed, plan)| {
                    GroupedExplanation::Chain(Box::new(narrowed), Box::new(plan))
                }),
        }
        .map_err(|e| from_kernel(&e))?;

        Ok(Response::new(grouped_explanation_to_proto(
            &explanation,
            &definitions,
            &grouping,
            warnings,
            served_by,
        )))
    }

    async fn leadership(
        &self,
        _request: Request<pb::LeadershipRequest>,
    ) -> Result<Response<pb::LeadershipStatus>, Status> {
        use pb::leadership_status::Standing as Wire;
        let status = match self.leadership.standing() {
            Standing::Follower { leader } => pb::LeadershipStatus {
                standing: Wire::Follower as i32,
                generation: None,
                holder: leader.unwrap_or_default(),
                stepped_down_because: String::new(),
            },
            Standing::Leader { term } => pb::LeadershipStatus {
                standing: Wire::Leader as i32,
                generation: Some(term.generation),
                holder: term.holder,
                stepped_down_because: String::new(),
            },
            Standing::SteppedDown { reason } => pb::LeadershipStatus {
                standing: Wire::SteppedDown as i32,
                generation: None,
                holder: String::new(),
                stepped_down_because: reason.reason().to_owned(),
            },
        };
        Ok(Response::new(status))
    }
}

/// Turn rows already in memory into the same stream shape a live query gives.
///
/// `stored` is the table's declared width, so a computed value goes in
/// `Row.computed` here exactly as it does on the streaming path. The two
/// shapes have to be identical: a client cannot see which one it got, and the
/// only thing worse than the arithmetic this removes would be it applying to
/// one of the two.
fn replay(
    rows: Vec<Row>,
    stored: usize,
    served_by: pb::ServedBy,
    warnings: Vec<String>,
    batch_size: usize,
    cursor: Vec<pb::Value>,
) -> RowStream {
    let mut messages = vec![Ok(pb::QueryResponse {
        rows: Vec::new(),
        served_by: Some(served_by),
        warnings,
        next_cursor: Vec::new(),
    })];
    for batch in rows.chunks(batch_size) {
        messages.push(Ok(pb::QueryResponse {
            rows: batch
                .iter()
                .map(|row| row_to_proto_split(row, stored))
                .collect(),
            served_by: None,
            warnings: Vec::new(),
            next_cursor: Vec::new(),
        }));
    }
    // On the last message, and on its own when the rows divided evenly into
    // batches — a caller reads the cursor off whichever message carries it,
    // and an empty trailing message is cheaper than making every batch carry
    // a field only one of them can fill.
    if !cursor.is_empty() {
        match messages.last_mut() {
            Some(Ok(last)) if !last.rows.is_empty() => last.next_cursor = cursor,
            _ => messages.push(Ok(pb::QueryResponse {
                rows: Vec::new(),
                served_by: None,
                warnings: Vec::new(),
                next_cursor: cursor,
            })),
        }
    }
    Box::pin(futures::stream::iter(messages))
}

/// Everything a streaming query needs, owned by its task.
struct Scan {
    pool: Arc<ReplicaPool>,
    table: TableId,
    context: SecurityContext,
    query: Query,
    /// What the server did with the request that the request did not ask for.
    /// Sent on the first message, with `served_by`.
    warnings: Vec<String>,
    batch_size: usize,
    freshness: Freshness,
    affinity: Option<Value>,
    /// Whether the caller asked for `next_cursor`. See `Query.paged`.
    paged: bool,
}

impl Scan {
    /// Route, open the cursor, report whether that worked, then walk it into
    /// the channel a batch at a time.
    ///
    /// Deliberately not `collect`: a query with no limit can be the whole
    /// table, and materialising it in order to send it would put the client's
    /// result set in the head node's memory. Back pressure comes from the
    /// channel — a slow client stops the cursor rather than filling a buffer.
    async fn run(
        &self,
        sender: &mpsc::Sender<Result<pb::QueryResponse, Status>>,
        started: oneshot::Sender<Result<(), Status>>,
    ) {
        let (view, store) = match self
            .pool
            .snapshot_from(self.freshness, self.affinity.as_ref())
            .await
        {
            Ok(routed) => routed,
            Err(error) => {
                let _ = started.send(Err(from_kernel(&error)));
                return;
            }
        };
        let served_by = served_by(store.as_ref());
        let Some(definition) = self.pool.catalog().table(self.table) else {
            let _ = started.send(Err(from_kernel(&KernelError::UnknownTable(self.table))));
            return;
        };

        let mut cursor = match view.execute(&self.context, definition, &self.query).await {
            Ok(cursor) => cursor,
            Err(error) => {
                let _ = started.send(Err(from_kernel(&error)));
                return;
            }
        };
        if started.send(Ok(())).is_err() {
            return;
        }

        // The first message carries `served_by`, the warnings and no rows, so
        // a client learns where its read went — and what the server did with
        // its hint — even when the result is empty.
        if sender
            .send(Ok(pb::QueryResponse {
                rows: Vec::new(),
                served_by: Some(served_by),
                warnings: self.warnings.clone(),
                next_cursor: Vec::new(),
            }))
            .await
            .is_err()
        {
            return;
        }

        let stored = definition.columns().len();
        let mut batch = Vec::with_capacity(self.batch_size);
        // The last row and how many went out, for the cursor.
        //
        // The row is *moved* here rather than having its key extracted per
        // row: the key is wanted once, at the end, and pulling it out on every
        // iteration would allocate two vectors per row to throw away all but
        // the last pair. This assignment costs nothing — `row` is already
        // owned and `row_to_proto_split` only borrows it.
        let mut last: Option<Row> = None;
        let mut sent = 0usize;
        loop {
            match cursor.next().await {
                Ok(Some(row)) => {
                    sent += 1;
                    batch.push(row_to_proto_split(&row, stored));
                    last = Some(row);
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.send(Err(from_kernel(&error))).await;
                    return;
                }
            }
            if batch.len() >= self.batch_size {
                let rows = core::mem::replace(&mut batch, Vec::with_capacity(self.batch_size));
                if sender
                    .send(Ok(pb::QueryResponse {
                        rows,
                        served_by: None,
                        warnings: Vec::new(),
                        next_cursor: Vec::new(),
                    }))
                    .await
                    .is_err()
                {
                    // The client hung up. Returning here is the point: an
                    // abandoned scan should stop reading object storage.
                    return;
                }
            }
        }
        // A short page proves there is nothing after it and gets no cursor. A
        // full one proves nothing either way and gets one anyway: reading one
        // row further to find out would be paid on every page to save one
        // empty request at the end of a sequence most callers never finish.
        // `Page` in the record layer makes the same trade.
        //
        // One check of `paged`, not two. It was two — a guard on collecting
        // the key as well as this one — and the second was unobservable given
        // the first, which a mutation forcing it to `true` proved by surviving
        // the whole suite.
        let cursor = match (self.paged, self.query.limit, &last) {
            (true, Some(limit), Some(row)) if sent >= limit => row
                .primary_key_values(definition)
                .iter()
                .map(value_to_proto)
                .collect(),
            _ => Vec::new(),
        };
        if !batch.is_empty() || !cursor.is_empty() {
            let _ = sender
                .send(Ok(pb::QueryResponse {
                    rows: batch,
                    served_by: None,
                    warnings: Vec::new(),
                    next_cursor: cursor,
                }))
                .await;
        }
    }
}

/// The joined rows of a join or chain, in batches.
type JoinedStream = Pin<Box<dyn futures::Stream<Item = Result<pb::JoinResponse, Status>> + Send>>;

/// The groups of an aggregate, in batches.
type GroupStream =
    Pin<Box<dyn futures::Stream<Item = Result<pb::AggregateResponse, Status>> + Send>>;

/// Turn joined rows already in memory into the same stream shape a live join
/// gives.
fn replay_joined(
    rows: Vec<MultiRow>,
    stored: &[usize],
    served_by: pb::ServedBy,
    warnings: Vec<String>,
    batch_size: usize,
    cursor: Vec<pb::Value>,
) -> JoinedStream {
    let mut messages = vec![Ok(pb::JoinResponse {
        rows: Vec::new(),
        served_by: Some(served_by),
        warnings,
        next_cursor: Vec::new(),
    })];
    for batch in rows.chunks(batch_size) {
        messages.push(Ok(pb::JoinResponse {
            rows: batch
                .iter()
                .map(|row| multi_row_to_proto(row, stored))
                .collect(),
            served_by: None,
            warnings: Vec::new(),
            next_cursor: Vec::new(),
        }));
    }
    // Same placement as `replay`: the cursor rides the last message, and gets
    // one of its own when the rows divided evenly into batches.
    if !cursor.is_empty() {
        match messages.last_mut() {
            Some(Ok(last)) if !last.rows.is_empty() => last.next_cursor = cursor,
            _ => messages.push(Ok(pb::JoinResponse {
                rows: Vec::new(),
                served_by: None,
                warnings: Vec::new(),
                next_cursor: cursor,
            })),
        }
    }
    Box::pin(futures::stream::iter(messages))
}

/// The same, for groups.
fn replay_groups(
    groups: Vec<Group>,
    served_by: pb::ServedBy,
    warnings: Vec<String>,
    batch_size: usize,
) -> GroupStream {
    let mut messages = vec![Ok(pb::AggregateResponse {
        groups: Vec::new(),
        served_by: Some(served_by),
        warnings,
    })];
    for batch in groups.chunks(batch_size) {
        messages.push(Ok(pb::AggregateResponse {
            groups: batch.iter().map(group_to_proto).collect(),
            served_by: None,
            warnings: Vec::new(),
        }));
    }
    Box::pin(futures::stream::iter(messages))
}

/// Either kernel explanation in its wire form.
fn multi_explanation_to_proto(
    explanation: &MultiExplanation,
    tables: &[&TableDef],
    read: &MultiRead,
    warnings: Vec<String>,
    served_by: pb::ServedBy,
) -> pb::JoinExplainResponse {
    match (explanation, read) {
        (MultiExplanation::Join(plan), _) => {
            join_explanation_to_proto(plan, warnings, Some(served_by))
        }
        (MultiExplanation::Chain(plan), MultiRead::Chain(chain)) => {
            chain_plan_to_proto(plan, tables, chain, warnings, Some(served_by))
        }
        // The two are built from the same request a few lines apart, so this
        // is unreachable rather than a case worth handling. It is written as
        // an empty explanation rather than a panic, on the principle that a
        // head node should not be able to bring itself down over a mismatch
        // it can describe.
        (MultiExplanation::Chain(plan), MultiRead::Join(_)) => pb::JoinExplainResponse {
            inputs: Vec::new(),
            estimated_rows: plan.estimated_rows,
            estimated_cost: plan.estimated_cost,
            display: String::new(),
            warnings,
            served_by: Some(served_by),
        },
    }
}

/// Everything a streaming join needs, owned by its task.
struct MultiScan {
    pool: Arc<ReplicaPool>,
    tables: Vec<TableId>,
    context: SecurityContext,
    read: MultiRead,
    /// As on [`Scan`]: sent on the first message, with `served_by`.
    warnings: Vec<String>,
    batch_size: usize,
    freshness: Freshness,
    affinity: Option<Value>,
    /// Whether the caller asked for `next_cursor`. See `JoinQuery.paged`.
    paged: bool,
    /// The page size, in **first-input rows**, which is what `JoinQuery.limit`
    /// counts once `paged` is set. Kept beside `read` rather than read back
    /// out of it because the kernel moves that limit onto the first input as
    /// part of paging, so by the time a cursor is wanted the join no longer
    /// carries it.
    page_size: Option<usize>,
}

impl MultiScan {
    /// Route once, open every input through that one view, and walk the joined
    /// rows into the channel a batch at a time.
    ///
    /// Routing once is the load-bearing part. Two calls into the pool would
    /// give two views at two sequences, and a join across them would be a
    /// result the database was never in — and `served_by` would have nothing
    /// truthful to report.
    async fn run(
        &self,
        sender: &mpsc::Sender<Result<pb::JoinResponse, Status>>,
        started: oneshot::Sender<Result<(), Status>>,
    ) {
        let (view, store) = match self
            .pool
            .snapshot_from(self.freshness, self.affinity.as_ref())
            .await
        {
            Ok(routed) => routed,
            Err(error) => {
                let _ = started.send(Err(from_kernel(&error)));
                return;
            }
        };
        let served_by = served_by(store.as_ref());

        let mut definitions = Vec::with_capacity(self.tables.len());
        for id in &self.tables {
            match self.pool.catalog().table(*id) {
                Some(table) => definitions.push(table),
                None => {
                    let _ = started.send(Err(from_kernel(&KernelError::UnknownTable(*id))));
                    return;
                }
            }
        }
        // One per input, so each input's computed values are split off its own
        // table's columns rather than off whichever width came to hand.
        let stored: Vec<usize> = definitions
            .iter()
            .map(|table| table.columns().len())
            .collect();

        let opened = match &self.read {
            MultiRead::Join(join) => match two_tables(&definitions) {
                Ok((left, right)) => view
                    .join(&self.context, left, right, join)
                    .await
                    .map(|cursor| MultiCursor::Join(Box::new(cursor))),
                Err(error) => Err(error),
            },
            MultiRead::Chain(chain) => view
                .chain(&self.context, &definitions, chain)
                .await
                .map(|cursor| MultiCursor::Chain(cursor, definitions.len())),
        };
        let mut cursor = match opened {
            Ok(cursor) => cursor,
            Err(error) => {
                let _ = started.send(Err(from_kernel(&error)));
                return;
            }
        };
        if started.send(Ok(())).is_err() {
            return;
        }

        // The first message carries `served_by`, the warnings and no rows, so
        // a client learns where its read went even when the result is empty.
        if sender
            .send(Ok(pb::JoinResponse {
                rows: Vec::new(),
                served_by: Some(served_by),
                warnings: self.warnings.clone(),
                next_cursor: Vec::new(),
            }))
            .await
            .is_err()
        {
            return;
        }

        let mut batch = Vec::with_capacity(self.batch_size);
        loop {
            match cursor.next().await {
                Ok(Some(row)) => batch.push(multi_row_to_proto(&row, &stored)),
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.send(Err(from_kernel(&error))).await;
                    return;
                }
            }
            if batch.len() >= self.batch_size {
                let rows = core::mem::replace(&mut batch, Vec::with_capacity(self.batch_size));
                if sender
                    .send(Ok(pb::JoinResponse {
                        rows,
                        served_by: None,
                        warnings: Vec::new(),
                        next_cursor: Vec::new(),
                    }))
                    .await
                    .is_err()
                {
                    // The client hung up. Returning here is the point: an
                    // abandoned join should stop reading object storage.
                    return;
                }
            }
        }
        // A page of a join is a page of its *first input's* table, so the
        // cursor is that table's key and "full page" is counted in its rows —
        // not in joined rows, which fan out. `Scan` makes the same trade about
        // the last page: a short page proves there is nothing after it, and a
        // full one gets a cursor rather than being read one row further to
        // find out.
        let next_cursor = match (self.paged, self.page_size, definitions.first()) {
            (true, Some(size), Some(first)) => match cursor.page_end(first) {
                (Some(key), read) if read >= size => key.iter().map(value_to_proto).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        if !batch.is_empty() || !next_cursor.is_empty() {
            let _ = sender
                .send(Ok(pb::JoinResponse {
                    rows: batch,
                    served_by: None,
                    warnings: Vec::new(),
                    next_cursor,
                }))
                .await;
        }
    }
}

/// The `IN` list a set of parent keys becomes: sorted, and each value once.
///
/// A free function rather than four lines inline, because the deduplication is
/// the saving this whole call exists for and it is **invisible in the answer**.
/// Removing it returns exactly the same rows — the grouping is by the row's own
/// value, so a repeated candidate creates no repeated group — and costs a
/// filter with ten thousand entries where two would do. A saving nothing can
/// observe is a saving nothing keeps, so it is named and tested here.
///
/// Sorted as well as deduplicated: `Expr::In` is turned into scan bounds, and
/// an unsorted list costs the planner a sort it would have to do anyway.
/// One step of a relationship path, resolved against the catalog.
///
/// See `RecordService::resolve_relation`, which is the only thing that builds
/// one. The lifetime is the catalog's: these borrow table definitions rather
/// than copying them, so a path of four steps holds four references and no
/// schema.
struct ResolvedStep<'a> {
    /// The table this step's rows come from.
    rows_from: &'a TableDef,
    /// The ordinal, in *this* step's rows, whose value groups them.
    match_on: Ordinal,
    /// The table the keys handed to this step belong to.
    ///
    /// For the first step that is the caller's own parent table, which nothing
    /// checks because the caller sent bare values rather than rows. For every
    /// later step it must be the previous step's `rows_from`, and that is
    /// checked: it is what makes a path compose.
    source: &'a TableDef,
    /// The ordinal, in the *previous* step's rows, whose value is this step's
    /// key. Meaningless on the first step, whose keys are the request's own.
    source_column: Ordinal,
}

/// A level per step, all empty, for a path with nothing to resolve.
///
/// The levels are emitted rather than left out so that `levels` and the
/// request's `path` stay index-for-index: a client walking the two together
/// should not have to special-case the empty answer.
fn empty_levels(steps: &[ResolvedStep<'_>]) -> Vec<pb::related_response::Level> {
    steps
        .iter()
        .map(|step| pb::related_response::Level {
            key_ordinal: step.source_column.0 as u32,
            groups: Vec::new(),
        })
        .collect()
}

fn distinct_keys(keys: &[pb::Value]) -> Result<Vec<Value>, Status> {
    let mut values = Vec::with_capacity(keys.len());
    for key in keys {
        values.push(value_from_proto(key)?);
    }
    values.sort();
    values.dedup();
    Ok(values)
}

#[cfg(test)]
mod tests {
    // The workspace bans `expect` in shipping code, and these tests are the
    // usual exception: a panic here is the failure report.
    #![allow(clippy::expect_used)]

    use super::{distinct_keys, pb};
    use slate_tuple::Value;

    fn u64s(ns: &[u64]) -> Vec<pb::Value> {
        ns.iter()
            .map(|n| pb::Value {
                kind: Some(pb::value::Kind::Uint64Value(*n)),
            })
            .collect()
    }

    /// Fifty parents over two libraries send two candidates.
    ///
    /// The number the caller sent is deliberately much larger than the number
    /// that survives, because "ten thousand books by one author send one value"
    /// is the claim and a two-element input could pass by accident.
    #[test]
    fn repeated_keys_collapse_to_one_candidate_each() {
        let many: Vec<u64> = (0..50).map(|n| 10 + (n % 2)).collect();
        let got = distinct_keys(&u64s(&many)).expect("valid values");
        assert_eq!(got, vec![Value::U64(10), Value::U64(11)]);
    }

    /// And they come back in order, which is what the scan bounds want.
    #[test]
    fn candidates_are_sorted() {
        let got = distinct_keys(&u64s(&[9, 3, 7, 3])).expect("valid values");
        assert_eq!(got, vec![Value::U64(3), Value::U64(7), Value::U64(9)]);
    }

    /// No parents, no candidates — which is what lets the caller skip the read
    /// entirely rather than scanning for an `IN ()` that matches nothing.
    #[test]
    fn no_keys_is_no_candidates() {
        assert!(distinct_keys(&[]).expect("valid values").is_empty());
    }
}
