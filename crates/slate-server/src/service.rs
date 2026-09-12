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
    explanation_to_proto, freshness_from_proto, query_from_proto, row_from_proto, row_to_proto,
    values_from_proto,
};
use crate::leadership::{Leadership, Standing};
use crate::proto as pb;
use crate::proto::records_server::{Records, RecordsServer};
use crate::session::{Limits, Sessions};
use crate::status::{from_kernel, redirect};
use slate_kernel::{
    Freshness, KernelError, KvReadStore, KvStore, Query, ReadToken, RecordSnapshot, RecordStore,
    RecordTransaction, ReplicaPool, RoutingPolicy, SecurityCatalog, SecurityContext, Statistics,
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
/// One writer store, a pool of replicas, a lease, and the rules for who may ask
/// what. Everything it holds is shared behind `Arc`, because request handlers
/// hand pieces of it to spawned tasks — see [`crate::session`] for why the
/// transaction path has to.
pub struct Head<S> {
    pool: Arc<ReplicaPool>,
    writer: Arc<RecordStore<Arc<S>>>,
    leadership: Arc<Leadership>,
    authenticator: Arc<dyn Authenticator>,
    sessions: Arc<Sessions>,
    limits: Limits,
}

impl<S> core::fmt::Debug for Head<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Head")
            .field("standing", &self.leadership.standing())
            .field("open_transactions", &self.sessions.len())
            .field("pool", &self.pool)
            .finish_non_exhaustive()
    }
}

impl<S: KvStore + KvReadStore> Head<S> {
    /// A head node over `writer`, reading through `replicas`.
    ///
    /// The writer joins the pool as well, as the fallback for a read no replica
    /// can serve: [`Freshness::Latest`] has nowhere else to go, and a pool
    /// without a writer refuses it rather than substituting a stale view.
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
        } = config;

        let pool = ReplicaPool::new(replicas, catalog.clone(), security.clone())
            .with_statistics(statistics.clone())
            .with_policy(routing)
            .with_writer(Arc::clone(&writer) as Arc<dyn KvReadStore>);
        let store =
            RecordStore::new(Arc::clone(&writer), catalog, security).with_statistics(statistics);

        Self {
            pool: Arc::new(pool),
            writer: Arc::new(store),
            leadership,
            authenticator,
            sessions: Arc::new(Sessions::new(limits)),
            limits,
        }
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
            Standing::Leader { .. } => Ok(&self.writer),
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
        table
            .tenant_column()
            .and_then(|_| context.principal().tenant.clone())
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
        let keys = request
            .primary_keys
            .iter()
            .map(values_from_proto)
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
        let Some(wire_key) = &request.primary_key else {
            return Err(Status::new(Code::InvalidArgument, "no primary key given"));
        };
        let key = values_from_proto(wire_key)?;

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
        let (query, _warnings) = query_from_proto(&wire, table)?;
        let batch_size = self.limits.rows_per_message.max(1);

        if !request.transaction.is_empty() {
            // A transactional read is answered in one go; see `Sessions::query`
            // for why it cannot stream.
            let rows = self
                .sessions
                .query(&request.transaction, &context, table.id(), query)
                .await?;
            return Ok(Response::new(replay(rows, in_transaction(), batch_size)));
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
fn replay(rows: Vec<Row>, served_by: pb::ServedBy, batch_size: usize) -> RowStream {
    let mut messages = vec![Ok(pb::QueryResponse {
        rows: Vec::new(),
        served_by: Some(served_by),
    })];
    for batch in rows.chunks(batch_size) {
        messages.push(Ok(pb::QueryResponse {
            rows: batch.iter().map(row_to_proto).collect(),
            served_by: None,
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

        // The first message carries `served_by` and no rows, so a client learns
        // where its read went even when the result is empty.
        if sender
            .send(Ok(pb::QueryResponse {
                rows: Vec::new(),
                served_by: Some(served_by),
            }))
            .await
            .is_err()
        {
            return;
        }

        let mut batch = Vec::with_capacity(self.batch_size);
        loop {
            match cursor.next().await {
                Ok(Some(row)) => batch.push(row_to_proto(&row)),
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
                }))
                .await;
        }
    }
}
