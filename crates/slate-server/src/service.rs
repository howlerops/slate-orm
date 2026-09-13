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
    MultiRead, aggregate_from_proto_query, chain_plan_to_proto, explanation_to_proto,
    freshness_from_proto, group_to_proto, join_explanation_to_proto, join_from_proto,
    multi_row_to_proto, primary_key_from_proto, query_from_proto, row_from_proto, row_to_proto,
    row_to_proto_split, two_tables,
};
use crate::fingerprint;
use crate::leadership::{Leadership, Standing};
use crate::proto as pb;
use crate::proto::records_server::{Records, RecordsServer};
use crate::session::{Limits, MultiCursor, MultiExplanation, MultiRow, Sessions};
use crate::status::{from_kernel, redirect};
use slate_kernel::{
    ExecutionLimits, Freshness, Group, KernelError, KvReadStore, KvStore, Query, ReadToken,
    RecordSnapshot, RecordStore, RecordTransaction, ReplicaPool, RoutingPolicy, SecurityCatalog,
    SecurityContext, Statistics,
};
use slate_schema::{Catalog, Row, TableDef, TableId};
use slate_tuple::Value;
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
    async fn autocommit(
        &self,
        context: &SecurityContext,
        write: Write<'_>,
    ) -> Result<(u64, Option<ReadToken>), Status> {
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
            Ok(outcome) => Ok(outcome),
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
    },
    Delete {
        table: &'a TableDef,
        keys: &'a [Vec<Value>],
    },
}

impl Write<'_> {
    /// Apply it, returning how many rows it acted on.
    async fn apply(
        &self,
        transaction: &RecordTransaction<'_>,
        context: &SecurityContext,
    ) -> Result<u64, KernelError> {
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
                .map(|()| rows.len() as u64),
            Self::Insert {
                table,
                rows,
                upsert: true,
            } => transaction
                .upsert_many(context, table, rows)
                .await
                .map(|()| rows.len() as u64),
            // `update_many` reads whether each row exists in one wave, the
            // same as `insert_many`, and refuses the whole batch if any of
            // them is missing rather than applying a prefix.
            Self::Update { table, rows } => transaction
                .update_many(context, table, rows)
                .await
                .map(|()| rows.len() as u64),
            Self::Delete { table, keys } => {
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
                Ok(affected)
            }
        }
    }
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
        let table = self.table(&request.table)?;
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
                affected,
            }));
        }

        let affected = self
            .sessions
            .insert(&request.transaction, &context, table.id(), rows, upsert)
            .await?;
        Ok(Response::new(pb::WriteResponse {
            sequence: None,
            affected,
        }))
    }

    async fn update(
        &self,
        request: Request<pb::UpdateRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.table(&request.table)?;
        fingerprint::check(table, request.schema.as_ref())?;
        let rows = request
            .rows
            .iter()
            .map(row_from_proto)
            .collect::<Result<Vec<Row>, Status>>()?;

        if request.transaction.is_empty() {
            let (affected, token) = self
                .autocommit(&context, Write::Update { table, rows: &rows })
                .await?;
            return Ok(Response::new(pb::WriteResponse {
                sequence: token.map(ReadToken::sequence),
                affected,
            }));
        }

        let affected = self
            .sessions
            .update(&request.transaction, &context, table.id(), rows)
            .await?;
        Ok(Response::new(pb::WriteResponse {
            sequence: None,
            affected,
        }))
    }

    async fn delete(
        &self,
        request: Request<pb::DeleteRequest>,
    ) -> Result<Response<pb::WriteResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.table(&request.table)?;
        fingerprint::check(table, request.schema.as_ref())?;
        // Checked against the table's key rather than encoded and looked up: a
        // key of the wrong arity or the wrong integer width used to delete
        // nothing and report `affected: 0`, which is also what a key that was
        // never there reports, and what a key the caller's policy hides
        // reports.
        let keys = request
            .primary_keys
            .iter()
            .map(|key| primary_key_from_proto(key, table))
            .collect::<Result<Vec<Vec<Value>>, Status>>()?;

        if request.transaction.is_empty() {
            let (affected, token) = self
                .autocommit(&context, Write::Delete { table, keys: &keys })
                .await?;
            return Ok(Response::new(pb::WriteResponse {
                sequence: token.map(ReadToken::sequence),
                affected,
            }));
        }

        let affected = self
            .sessions
            .delete(&request.transaction, &context, table.id(), keys)
            .await?;
        Ok(Response::new(pb::WriteResponse {
            sequence: None,
            affected,
        }))
    }

    async fn get(
        &self,
        request: Request<pb::GetRequest>,
    ) -> Result<Response<pb::GetResponse>, Status> {
        let context = self.context(&request)?;
        let request = request.into_inner();
        let table = self.table(&request.table)?;
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
        let table = self.table(&wire.table)?;
        // Kept, not dropped. An ignored index hint used to be reported by
        // `Explain` alone, so a caller whose hint did nothing had to issue a
        // *different* request and trust the planner had decided the same way
        // on it. The first message of the stream is always sent and already
        // carries `served_by`, so there was a header to put this in all along.
        let (query, warnings) = query_from_proto(&wire, table)?;
        let stored = table.columns().len();
        let batch_size = self.limits.rows_per_message.max(1);

        if !request.transaction.is_empty() {
            // A transactional read is answered in one go; see `Sessions::query`
            // for why it cannot stream.
            let rows = self
                .sessions
                .query(&request.transaction, &context, table.id(), query)
                .await?;
            return Ok(Response::new(replay(
                rows,
                stored,
                in_transaction(),
                warnings,
                batch_size,
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
        let table = self.table(&wire.table)?;
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
        let (tables, read, warnings) = join_from_proto(&wire, self.pool.catalog())?;
        let definitions = self.definitions(&tables)?;
        let stored: Vec<usize> = definitions
            .iter()
            .map(|table| table.columns().len())
            .collect();
        let batch_size = self.limits.rows_per_message.max(1);

        if !request.transaction.is_empty() {
            let rows = self
                .sessions
                .multi_read(&request.transaction, &context, tables, read)
                .await?;
            return Ok(Response::new(replay_joined(
                rows,
                &stored,
                in_transaction(),
                warnings,
                batch_size,
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
        let table = self.definition(read.table)?;
        let affinity = Self::affinity(table, &context);
        let (view, served_by) = self.read_view(freshness, affinity.as_ref()).await?;
        let groups = view
            .group_by_having(
                &context,
                table,
                &read.query,
                &read.group,
                &read.aggregates,
                &read.having,
            )
            .await
            .map_err(|e| from_kernel(&e))?;

        Ok(Response::new(replay_groups(
            groups, served_by, warnings, batch_size,
        )))
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
) -> RowStream {
    let mut messages = vec![Ok(pb::QueryResponse {
        rows: Vec::new(),
        served_by: Some(served_by),
        warnings,
    })];
    for batch in rows.chunks(batch_size) {
        messages.push(Ok(pb::QueryResponse {
            rows: batch
                .iter()
                .map(|row| row_to_proto_split(row, stored))
                .collect(),
            served_by: None,
            warnings: Vec::new(),
        }));
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
            }))
            .await
            .is_err()
        {
            return;
        }

        let stored = definition.columns().len();
        let mut batch = Vec::with_capacity(self.batch_size);
        loop {
            match cursor.next().await {
                Ok(Some(row)) => batch.push(row_to_proto_split(&row, stored)),
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
        if !batch.is_empty() {
            let _ = sender
                .send(Ok(pb::QueryResponse {
                    rows: batch,
                    served_by: None,
                    warnings: Vec::new(),
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
) -> JoinedStream {
    let mut messages = vec![Ok(pb::JoinResponse {
        rows: Vec::new(),
        served_by: Some(served_by),
        warnings,
    })];
    for batch in rows.chunks(batch_size) {
        messages.push(Ok(pb::JoinResponse {
            rows: batch
                .iter()
                .map(|row| multi_row_to_proto(row, stored))
                .collect(),
            served_by: None,
            warnings: Vec::new(),
        }));
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
        if !batch.is_empty() {
            let _ = sender
                .send(Ok(pb::JoinResponse {
                    rows: batch,
                    served_by: None,
                    warnings: Vec::new(),
                }))
                .await;
        }
    }
}
