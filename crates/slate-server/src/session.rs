//! Transactions that outlive a request.
//!
//! # Why a task per transaction
//!
//! [`RecordTransaction`] borrows the store it came from, so it cannot simply be
//! put in a map: the map would have to hold a reference into something it does
//! not own. The usual escapes are a self-referential struct or leaking the
//! store to get a `'static` borrow, and both were rejected — the first needs a
//! crate to be sound, and the second leaks a store per handover, which is
//! exactly the event a head node has to survive.
//!
//! What works with no machinery at all is to keep the borrow inside a task. The
//! task owns an `Arc<RecordStore>`, opens the transaction from it, and lives
//! for as long as the transaction does; requests reach it as messages and
//! answers come back on a one-shot. The borrow never crosses a boundary, so
//! there is nothing to make sound.
//!
//! It buys two things beyond that. Commands on one transaction are serialised
//! by construction, which they have to be — `RecordTransaction` is not
//! something two requests should interleave on. And the transaction has an
//! owner that can end it: the task, not the client.
//!
//! # Ending it
//!
//! A transaction on the writer pins a snapshot. A client that opens one and
//! disappears would pin it forever, so the task gives up after
//! [`Limits::idle_timeout`] of silence and rolls back. That is a policy choice
//! with a cost — a genuinely slow client loses its work — and the alternative
//! is a head node whose memory is controlled by whoever disconnects least
//! politely.
//!
//! The count is capped too. Each open transaction is a task, a channel and a
//! snapshot, and "how many transactions may one client open" is otherwise
//! answered by the machine falling over.
//!
//! # Who may use one
//!
//! The handle is a v4 UUID, so it is a capability: holding it is enough. That
//! is not enough on its own — capabilities leak through logs, proxies and
//! traces — so a session also remembers the principal that opened it and
//! refuses anyone else. The refusal is the same `NOT_FOUND` an unknown handle
//! gets, because "that transaction exists but is not yours" is a fact worth
//! not disclosing.

use crate::convert::{GroupedRead, GroupedSource, MultiRead, chain_row_values, two_tables};
use crate::leadership::Leadership;
use crate::status::from_kernel;
use slate_kernel::security::Principal;
use slate_kernel::{
    Chain, ChainCursor, ChainPlan, Explanation, Expr, Group, JoinCursor, JoinExplanation,
    KernelError, KvStore, Query, ReadToken, RecordStore, RecordTransaction, Scalar,
    SecurityContext,
};
use slate_schema::{Ordinal, Row, TableDef, TableId};
use slate_tuple::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tonic::{Code, Status};
use uuid::Uuid;

/// What a head node will spend on open transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How many transactions may be open at once, across all clients.
    pub max_transactions: usize,
    /// How long a transaction may sit idle before it is rolled back.
    pub idle_timeout: Duration,
    /// How many rows go in one message of a query stream.
    ///
    /// Framing costs per message and latency costs per batch, so this trades
    /// one against the other. Measured (`slate-headbench`, see
    /// `docs/performance.md`), and then measured again after the first round
    /// turned out to have been taken through a Nagled socket: the useful range
    /// is 32 at the bottom to about 1,000 at the top, and 256 sits inside it.
    ///
    /// The earlier numbers here said 64–1024 and cited a first-row cost of
    /// 370 µs at a batch of 1 against 3.07 ms at 256, plus a reproducible and
    /// unexplained step around a batch of 125. All three were the socket. With
    /// `TCP_NODELAY` the first-row gap is 547 µs against 1.13 ms, which is the
    /// honest cost of 144 more rows and is bought back in framing, and the
    /// step is gone. The argument for lowering this to 112 is withdrawn: it
    /// was buying back a delay that should not have been there.
    pub rows_per_message: usize,
    /// How many operations one `Batch` may carry, or `None` for no cap.
    ///
    /// A batch is a round trip's worth of work done in one request, so the
    /// cap is on the *request* rather than on the rows it touches — the
    /// per-operation limits the rest of this struct sets still apply to each
    /// one. `None` is allowed and is not the default: an uncapped batch is a
    /// client deciding how long the server's next unit of work is.
    pub max_batch_operations: Option<usize>,
    /// How many rows a predicate write may hand back, or `None` for no cap.
    ///
    /// A cap on the *answer*, not on the write: a `DELETE … WHERE` over a
    /// million rows is a legitimate delete and is uncapped, and only asking
    /// for those million rows back is refused. `WriteResponse` is one message,
    /// so a large enough set of rows makes it undeliverable — and the refusal
    /// has to land before the write, because otherwise the rows are gone and
    /// the caller has an error instead of the only record of them.
    ///
    /// That is not hypothetical: at 8,000 rows of about a kilobyte each the
    /// response was 8,423,749 bytes against a client decode limit of
    /// 4,194,304, and the delete had committed. See
    /// `crates/slate-server/tests/returning_cap.rs`.
    ///
    /// The number is a row count because rows are what the scan counts, and it
    /// is a proxy for what actually matters, which is bytes: one row holding a
    /// large enough blob passes a count cap and fails to encode anyway. A
    /// byte-exact bound needs the rows encoded before the write is allowed to
    /// commit, which is a bigger change than this and is written up in
    /// `docs/orm-comparison.md`.
    pub max_returned_rows: Option<usize>,
    /// How many steps a `Related` path may have, or `None` for no cap.
    ///
    /// One step is one read, performed on the server, so the depth of a path
    /// is how many reads a single request buys. That number arrives *in the
    /// request*, which is what makes this different from every nested load
    /// this repository had before it: `slate-orm`'s `load_nested` takes its
    /// depth as a type parameter, so a caller cannot ask for a thousand levels
    /// without writing a thousand types, and its documentation says as much —
    /// "a limit nobody can exceed is a limit nobody maintains". Putting the
    /// path on the wire is exactly the form that comment named as needing the
    /// refusal, because the depth is now a number in a message.
    ///
    /// Bounds the reads, not the rows. A step that fans out to a million rows
    /// is bounded by nothing here; what is bounded is how many times the
    /// server goes back to storage before it answers.
    pub max_relation_depth: Option<usize>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_transactions: 1024,
            idle_timeout: Duration::from_secs(30),
            rows_per_message: 256,
            // A thousand writes in one request is far past where the round
            // trip is the cost, and is a number a client reaches by accident
            // rather than on purpose. Not measured: it is a guard against a
            // request nobody should send, not a tuning knob.
            max_batch_operations: Some(1_000),
            // 10,000 rows of the 256-byte row this node was measured on is
            // about 2.6 MB, comfortably inside the 4 MiB a default gRPC
            // client will decode, and far more rows than a caller reads.
            // Derived from that limit rather than measured, which is why it
            // is round.
            max_returned_rows: Some(10_000),
            // Four, because the shapes that motivate this are two and three
            // deep — an article's tags through a join table is two, and the
            // authors of an article's comments is two with the middle kept —
            // and a fourth leaves room without inviting a path nobody can
            // reason about. Not measured: like `max_batch_operations` it is a
            // guard against a request nobody should send rather than a tuning
            // knob, and a deployment that genuinely walks five relationships
            // in one call can raise it.
            max_relation_depth: Some(4),
        }
    }
}

/// One row of a multi-table read: one entry per input, in request order, and
/// whatever the read itself computed.
///
/// `inputs` is `None` where an outer join preserved something that matched
/// nothing. The two kernel cursors spell that differently — a `JoinedRow` has a
/// left and a right, a `ChainRow` has however many tables it has reached — so
/// both are flattened to this shape once, here, rather than at each of the two
/// call sites that would otherwise have to agree.
///
/// `computed` is `JoinQuery.compute`: values over the *joined* row, which
/// belong to no single input and so cannot live inside one. An input's own
/// computed values are still inside that input's `Row`, where they have always
/// been, and are split off it by declared table width on the way out.
///
/// This was a bare `Vec<Option<Row>>` until the join grew computed values of
/// its own. Widening the type rather than passing a second vector alongside it
/// is what stops the two getting out of step at the two call sites — the exact
/// thing the paragraph above says this type exists to prevent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MultiRow {
    /// One entry per input, `None` where nothing matched.
    pub inputs: Vec<Option<Row>>,
    /// What the join's or chain's own `compute` produced, in order.
    pub computed: Vec<Value>,
}

/// How a multi-table read will be run.
///
/// Two shapes because the kernel has two: a two-table join can choose which
/// side to hold in memory, and a chain cannot. See [`MultiRead`].
#[derive(Debug)]
pub enum MultiExplanation {
    /// A two-table join.
    Join(Box<JoinExplanation>),
    /// Three or more tables.
    Chain(Box<ChainPlan>),
}

/// The plan a grouped read would run under, whichever source it groups.
///
/// Three variants rather than reusing [`MultiExplanation`] plus a table case,
/// because a grouped read over one table is a first-class shape here: grouping
/// narrows its projection too, so `Explain` on the underlying query describes
/// something the aggregate will not run.
#[derive(Debug)]
pub enum GroupedExplanation {
    /// Grouping one table.
    Table(Box<Explanation>),
    /// Grouping a two-table join.
    Join(Box<JoinExplanation>),
    /// Grouping a chain, with the narrowed chain the plan describes — a step
    /// cannot be rendered without its own query, and the caller's would report
    /// a limit the grouped read discards.
    Chain(Box<Chain>, Box<ChainPlan>),
}

/// A join or chain read inside a transaction: its rows, and where it ended.
///
/// The two travel together because they cannot be derived from one another. A
/// page of a join is a page of its *first input's* table, and under an inner
/// join a first-input row that matched nothing is read and produces no row —
/// so the returned rows do not say where the page ended, and a cursor taken
/// from them would not advance past such a page.
#[derive(Debug)]
pub struct MultiPage {
    /// The rows, already flattened and padded.
    pub rows: Vec<MultiRow>,
    /// Where to resume, and how many first-input rows were read. The second is
    /// compared against the page size to tell a full page from a short one.
    pub page: (Option<Vec<Value>>, usize),
}

/// A cursor over either kernel shape, handing out [`MultiRow`]s.
///
/// The padding matters and is easy to miss: a right outer step of a chain
/// preserves a row of a later table with every earlier one absent, and the
/// kernel represents that as a *shorter* `ChainRow`. A client reading its
/// inputs positionally has to find each one where it declared it, so the row
/// is padded to the full width here.
#[derive(Debug)]
pub enum MultiCursor<'a> {
    /// A two-table join.
    /// Boxed because a `JoinCursor` holds a hash table's worth of state and a
    /// `ChainCursor` holds an iterator, so the enum would otherwise be the
    /// size of the larger everywhere it is moved.
    Join(Box<JoinCursor<'a>>),
    /// A chain, and how many inputs its rows must be padded to.
    Chain(ChainCursor, usize),
}

impl MultiCursor<'_> {
    /// The next row, flattened.
    pub async fn next(&mut self) -> Result<Option<MultiRow>, KernelError> {
        match self {
            Self::Join(cursor) => Ok(cursor.next().await?.map(|row| MultiRow {
                inputs: vec![row.left, row.right],
                computed: row.computed,
            })),
            Self::Chain(cursor, inputs) => Ok(cursor.next().await?.map(|row| MultiRow {
                inputs: chain_row_values(&row, *inputs),
                computed: row.computed().to_vec(),
            })),
        }
    }

    /// Drain into a vector.
    pub async fn collect(mut self) -> Result<Vec<MultiRow>, KernelError> {
        self.collect_rows().await
    }

    /// Drain into a vector, leaving the cursor to be asked where the page
    /// ended. `collect` consumes, and `page_end` has to be read afterwards.
    pub async fn collect_rows(&mut self) -> Result<Vec<MultiRow>, KernelError> {
        let mut out = Vec::new();
        while let Some(row) = self.next().await? {
            out.push(row);
        }
        Ok(out)
    }

    /// Where a paged read should resume, and whether the page was a full one.
    ///
    /// A page of a join or a chain is a page of its **first input's** table,
    /// so this is that table's primary key — which is why the first definition
    /// is what it is asked against and the others are not needed.
    ///
    /// Returns the boundary and how many first-input rows were read. A caller
    /// compares the second against the page size: a short page proves there is
    /// nothing after it and gets no cursor, exactly as a single-table scan
    /// decides it.
    ///
    /// Meaningful once the cursor is drained. A join keeps probes in flight,
    /// so before then the boundary runs ahead of the rows handed out; both
    /// converge when the first input's scan ends, which its own limit makes
    /// certain.
    pub fn page_end(&self, first: &TableDef) -> (Option<Vec<Value>>, usize) {
        match self {
            Self::Join(cursor) => (cursor.page_end(first), cursor.driving_rows()),
            Self::Chain(cursor, _) => (
                cursor.page_end().map(<[Value]>::to_vec),
                cursor.driving_rows(),
            ),
        }
    }
}

/// A request to a transaction's task.
///
/// Every variant carries an already-converted kernel type: conversion happens
/// in the request handler so a malformed request fails there, with the
/// transaction untouched.
enum Command {
    Insert {
        context: Box<SecurityContext>,
        table: TableId,
        rows: Vec<Row>,
        upsert: bool,
        reply: oneshot::Sender<Result<u64, KernelError>>,
    },
    Update {
        context: Box<SecurityContext>,
        table: TableId,
        rows: Vec<Row>,
        /// Empty for an ordinary update; otherwise one row per row in `rows`,
        /// already checked for arity by the handler that decoded it.
        expected: Vec<Row>,
        reply: oneshot::Sender<Result<u64, KernelError>>,
    },
    Delete {
        context: Box<SecurityContext>,
        table: TableId,
        keys: Vec<Vec<Value>>,
        /// Empty for an ordinary delete; otherwise one row per key, already
        /// checked for arity by the handler that decoded it.
        expected: Vec<Row>,
        reply: oneshot::Sender<Result<u64, KernelError>>,
    },
    /// Both predicate writes reply with the rows rather than a count. The
    /// kernel has them either way — every row touched had to be read to be
    /// written — and `RETURNING` is the only thing a caller who named a
    /// condition rather than rows can use to learn what it hit.
    DeleteWhere {
        context: Box<SecurityContext>,
        table: TableId,
        predicate: Expr,
        /// The ceiling on the match, or `None`. Resolved by the caller rather
        /// than read from `Limits` here, because it depends on whether the
        /// request asked for its rows back and this task cannot see that.
        at_most: Option<usize>,
        reply: oneshot::Sender<Result<Vec<Row>, KernelError>>,
    },
    UpdateWhere {
        context: Box<SecurityContext>,
        table: TableId,
        predicate: Expr,
        assignments: Vec<(Ordinal, Scalar)>,
        at_most: Option<usize>,
        reply: oneshot::Sender<Result<Vec<Row>, KernelError>>,
    },
    Get {
        context: Box<SecurityContext>,
        table: TableId,
        key: Vec<Value>,
        reply: oneshot::Sender<Result<Option<Row>, KernelError>>,
    },
    Query {
        context: Box<SecurityContext>,
        table: TableId,
        query: Box<Query>,
        reply: oneshot::Sender<Result<Vec<Row>, KernelError>>,
    },
    Explain {
        context: Box<SecurityContext>,
        table: TableId,
        query: Box<Query>,
        reply: oneshot::Sender<Result<Explanation, KernelError>>,
    },
    MultiRead {
        context: Box<SecurityContext>,
        tables: Vec<TableId>,
        read: Box<MultiRead>,
        reply: oneshot::Sender<Result<MultiPage, KernelError>>,
    },
    ExplainMulti {
        context: Box<SecurityContext>,
        tables: Vec<TableId>,
        read: Box<MultiRead>,
        reply: oneshot::Sender<Result<MultiExplanation, KernelError>>,
    },
    Aggregate {
        context: Box<SecurityContext>,
        read: Box<GroupedRead>,
        reply: oneshot::Sender<Result<Vec<Group>, KernelError>>,
    },
    ExplainAggregate {
        context: Box<SecurityContext>,
        read: Box<GroupedRead>,
        reply: oneshot::Sender<Result<GroupedExplanation, KernelError>>,
    },
    Commit {
        reply: oneshot::Sender<Result<Option<ReadToken>, KernelError>>,
    },
    Rollback {
        reply: oneshot::Sender<()>,
    },
}

struct Session {
    owner: Principal,
    commands: mpsc::Sender<Command>,
}

/// Every transaction this node has open.
#[derive(Debug)]
pub struct Sessions {
    open: Arc<Mutex<HashMap<Uuid, Session>>>,
    limits: Limits,
}

impl core::fmt::Debug for Session {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

/// The status an unknown handle and someone else's handle both get.
fn no_such_transaction() -> Status {
    Status::new(Code::NotFound, "no such transaction")
}

/// Lost the task: it timed out, was fenced, or the process is shutting down.
fn transaction_ended() -> Status {
    Status::new(
        Code::Aborted,
        "the transaction ended before this request reached it; open a new one",
    )
}

impl Sessions {
    /// An empty registry.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            open: Arc::new(Mutex::new(HashMap::new())),
            limits,
        }
    }

    /// How many transactions are open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.open.lock().map_or(0, |open| open.len())
    }

    /// Whether none are open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Open a transaction on `store` and return its handle.
    ///
    /// Fails before spawning anything if the node is at its limit, and fails
    /// after opening if the store refuses — which is how a fenced writer is
    /// found out here.
    pub async fn begin<S: KvStore>(
        &self,
        store: Arc<RecordStore<S>>,
        leadership: Arc<Leadership>,
        owner: Principal,
    ) -> Result<Uuid, Status> {
        // Checked before the task is spawned, so a client hammering `Begin`
        // cannot outrun the accounting.
        {
            let open = self.open.lock().map_err(poisoned)?;
            if open.len() >= self.limits.max_transactions {
                return Err(Status::new(
                    Code::ResourceExhausted,
                    format!(
                        "this node already has {} transactions open, which is its limit",
                        open.len()
                    ),
                ));
            }
        }

        let id = Uuid::new_v4();
        let (commands, receiver) = mpsc::channel(1);
        let (ready, opened) = oneshot::channel();
        let registry = Arc::clone(&self.open);
        let idle = self.limits.idle_timeout;

        tokio::spawn(async move {
            run(store, leadership, receiver, ready, idle).await;
            // The task removes its own entry: nothing else knows when a
            // transaction has timed out, and a registry that only shrank on
            // `Commit` would leak an entry for every client that walked away.
            if let Ok(mut open) = registry.lock() {
                open.remove(&id);
            }
        });

        match opened.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(from_kernel(&error)),
            Err(_) => return Err(transaction_ended()),
        }

        self.open
            .lock()
            .map_err(poisoned)?
            .insert(id, Session { owner, commands });
        Ok(id)
    }

    /// Send a command to `id`'s task and wait for its answer.
    async fn dispatch<T>(
        &self,
        id: &str,
        caller: &SecurityContext,
        build: impl FnOnce(oneshot::Sender<T>) -> Command,
    ) -> Result<T, Status> {
        let id = Uuid::parse_str(id).map_err(|_| no_such_transaction())?;
        let commands = {
            let open = self.open.lock().map_err(poisoned)?;
            let session = open.get(&id).ok_or_else(no_such_transaction)?;
            // Same status as an unknown handle: see the module docs.
            if session.owner != *caller.principal() {
                return Err(no_such_transaction());
            }
            session.commands.clone()
        };

        let (reply, answer) = oneshot::channel();
        commands
            .send(build(reply))
            .await
            .map_err(|_| transaction_ended())?;
        answer.await.map_err(|_| transaction_ended())
    }

    /// Insert or upsert rows in an open transaction.
    pub async fn insert(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        rows: Vec<Row>,
        upsert: bool,
    ) -> Result<u64, Status> {
        self.dispatch(id, context, |reply| Command::Insert {
            context: Box::new(context.clone()),
            table,
            rows,
            upsert,
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Replace rows in an open transaction.
    pub async fn update(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        rows: Vec<Row>,
        expected: Vec<Row>,
    ) -> Result<u64, Status> {
        self.dispatch(id, context, |reply| Command::Update {
            context: Box::new(context.clone()),
            table,
            rows,
            expected,
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Delete rows in an open transaction.
    pub async fn delete(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        keys: Vec<Vec<Value>>,
        expected: Vec<Row>,
    ) -> Result<u64, Status> {
        self.dispatch(id, context, |reply| Command::Delete {
            context: Box::new(context.clone()),
            table,
            keys,
            expected,
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Delete every row a predicate selects, in an open transaction.
    pub async fn delete_where(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        predicate: Expr,
        at_most: Option<usize>,
    ) -> Result<Vec<Row>, Status> {
        self.dispatch(id, context, |reply| Command::DeleteWhere {
            context: Box::new(context.clone()),
            table,
            predicate,
            at_most,
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Assign to columns of every row a predicate selects, in an open
    /// transaction.
    pub async fn update_where(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        predicate: Expr,
        assignments: Vec<(Ordinal, Scalar)>,
        at_most: Option<usize>,
    ) -> Result<Vec<Row>, Status> {
        self.dispatch(id, context, |reply| Command::UpdateWhere {
            context: Box::new(context.clone()),
            table,
            predicate,
            assignments,
            at_most,
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Read one row in an open transaction.
    pub async fn get(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        key: Vec<Value>,
    ) -> Result<Option<Row>, Status> {
        self.dispatch(id, context, |reply| Command::Get {
            context: Box::new(context.clone()),
            table,
            key,
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Run a query in an open transaction.
    ///
    /// The rows are collected rather than streamed. A cursor borrows the
    /// transaction, so streaming one out of the task would put the borrow back
    /// where it could not go; and a read inside a write transaction is nearly
    /// always the small read that decides what to write next, not the export.
    /// A large result should be a non-transactional query, which does stream.
    pub async fn query(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        query: Query,
    ) -> Result<Vec<Row>, Status> {
        self.dispatch(id, context, |reply| Command::Query {
            context: Box::new(context.clone()),
            table,
            query: Box::new(query),
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Explain a query in an open transaction.
    pub async fn explain(
        &self,
        id: &str,
        context: &SecurityContext,
        table: TableId,
        query: Query,
    ) -> Result<Explanation, Status> {
        self.dispatch(id, context, |reply| Command::Explain {
            context: Box::new(context.clone()),
            table,
            query: Box::new(query),
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Run a join or a chain in an open transaction.
    ///
    /// Collected rather than streamed, for the same reason a transactional
    /// query is: a cursor borrows the transaction, and streaming one out of the
    /// task would put that borrow back where it cannot go.
    pub async fn multi_read(
        &self,
        id: &str,
        context: &SecurityContext,
        tables: Vec<TableId>,
        read: MultiRead,
    ) -> Result<MultiPage, Status> {
        self.dispatch(id, context, |reply| Command::MultiRead {
            context: Box::new(context.clone()),
            tables,
            read: Box::new(read),
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Explain a join or a chain in an open transaction.
    pub async fn explain_multi(
        &self,
        id: &str,
        context: &SecurityContext,
        tables: Vec<TableId>,
        read: MultiRead,
    ) -> Result<MultiExplanation, Status> {
        self.dispatch(id, context, |reply| Command::ExplainMulti {
            context: Box::new(context.clone()),
            tables,
            read: Box::new(read),
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Explain a grouped read in an open transaction.
    pub async fn explain_aggregate(
        &self,
        id: &str,
        context: &SecurityContext,
        read: GroupedRead,
    ) -> Result<GroupedExplanation, Status> {
        self.dispatch(id, context, |reply| Command::ExplainAggregate {
            context: Box::new(context.clone()),
            read: Box::new(read),
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Compute aggregates, per group, in an open transaction.
    pub async fn aggregate(
        &self,
        id: &str,
        context: &SecurityContext,
        read: GroupedRead,
    ) -> Result<Vec<Group>, Status> {
        self.dispatch(id, context, |reply| Command::Aggregate {
            context: Box::new(context.clone()),
            read: Box::new(read),
            reply,
        })
        .await?
        .map_err(|error| from_kernel(&error))
    }

    /// Commit, returning the sequence the writes landed at.
    pub async fn commit(
        &self,
        id: &str,
        context: &SecurityContext,
    ) -> Result<Option<ReadToken>, Status> {
        self.dispatch(id, context, |reply| Command::Commit { reply })
            .await?
            .map_err(|error| from_kernel(&error))
    }

    /// Discard everything the transaction buffered.
    pub async fn rollback(&self, id: &str, context: &SecurityContext) -> Result<(), Status> {
        self.dispatch(id, context, |reply| Command::Rollback { reply })
            .await
    }
}

fn poisoned(_: impl core::fmt::Debug) -> Status {
    Status::new(
        Code::Internal,
        "the transaction registry is poisoned; this node is not serving transactions",
    )
}

/// One transaction, for as long as it lives.
async fn run<S: KvStore>(
    store: Arc<RecordStore<S>>,
    leadership: Arc<Leadership>,
    mut commands: mpsc::Receiver<Command>,
    ready: oneshot::Sender<Result<(), KernelError>>,
    idle: Duration,
) {
    let transaction = match store.begin().await {
        Ok(transaction) => {
            if ready.send(Ok(())).is_err() {
                // The client gave up between `Begin` and its answer. Nothing
                // has been written, so there is nothing to roll back.
                return;
            }
            transaction
        }
        Err(error) => {
            // A fenced writer fails at `begin`, before a transaction exists —
            // which is why this is checked here and not only on the first
            // write. See `handover.rs`.
            note_fencing(&leadership, &error).await;
            let _ = ready.send(Err(error));
            return;
        }
    };

    loop {
        let command = match tokio::time::timeout(idle, commands.recv()).await {
            // Idle too long. Rolling back is the safe direction: a transaction
            // nobody is driving has, by definition, not been told to commit.
            Err(_elapsed) => break,
            // Every handle to the session is gone.
            Ok(None) => break,
            Ok(Some(command)) => command,
        };

        match command {
            Command::Commit { reply } => {
                let outcome = transaction.commit().await;
                if let Err(error) = &outcome {
                    note_fencing(&leadership, error).await;
                }
                let _ = reply.send(outcome);
                return;
            }
            Command::Rollback { reply } => {
                transaction.rollback();
                let _ = reply.send(());
                return;
            }
            other => {
                if let Some(error) = apply(&transaction, &store, other).await {
                    note_fencing(&leadership, &error).await;
                    if matches!(error, KernelError::WriterFenced) {
                        // The transaction is unusable and so is the store.
                        // Ending the task frees the snapshot and makes every
                        // later request on this handle say so.
                        return;
                    }
                }
            }
        }
    }

    transaction.rollback();
}

/// Run one command, returning the error it produced if any.
///
/// The error is both sent to the caller and handed back, so the loop can act on
/// a fence without every arm having to remember to.
async fn apply<S: KvStore>(
    transaction: &RecordTransaction<'_>,
    store: &RecordStore<S>,
    command: Command,
) -> Option<KernelError> {
    /// Resolve a table, or answer the caller and stop.
    macro_rules! table {
        ($id:expr, $reply:expr) => {
            match resolve(store, $id) {
                Ok(table) => table,
                Err(error) => {
                    let _ = $reply.send(Err(error));
                    return None;
                }
            }
        };
    }

    match command {
        Command::Insert {
            context,
            table,
            rows,
            upsert,
            reply,
        } => {
            let definition = table!(table, reply);
            let affected = rows.len() as u64;
            // `insert_many` overlaps the duplicate-key reads across the batch;
            // a loop here would cost a round trip per row.
            let outcome = if upsert {
                transaction.upsert_many(&context, definition, &rows).await
            } else {
                transaction.insert_many(&context, definition, &rows).await
            };
            answer(reply, outcome.map(|()| affected))
        }
        Command::Update {
            context,
            table,
            rows,
            expected,
            reply,
        } => {
            let definition = table!(table, reply);
            let affected = rows.len() as u64;
            let outcome = if expected.is_empty() {
                // `update_many` overlaps the reads that decide whether each row
                // is there, the same as `insert_many`. It is also
                // all-or-nothing, where the loop this replaces applied a prefix
                // and then reported the error — a count the caller could not
                // act on, since the transaction rolls back anyway.
                transaction.update_many(&context, definition, &rows).await
            } else {
                // A row at a time; see the same branch in `Write::apply` for
                // why there is no batched form. All-or-nothing holds here for a
                // different reason: the first refusal returns, and the
                // transaction this ran inside rolls back whatever came before.
                let mut outcome = Ok(());
                for (row, was) in rows.iter().zip(expected.iter()) {
                    outcome = transaction
                        .update_if_unchanged(&context, definition, row, was)
                        .await;
                    if outcome.is_err() {
                        break;
                    }
                }
                outcome
            };
            answer(reply, outcome.map(|()| affected))
        }
        Command::Delete {
            context,
            table,
            keys,
            expected,
            reply,
        } => {
            let definition = table!(table, reply);
            if !expected.is_empty() {
                // A conditional delete is counted differently from a plain one,
                // for the reason `Write::apply`'s twin of this arm gives:
                // `delete_if_unchanged` refuses an absent row rather than
                // returning `false`, so every key that got through was there.
                let mut outcome = Ok(());
                for (key, was) in keys.iter().zip(expected.iter()) {
                    outcome = transaction
                        .delete_if_unchanged(&context, definition, key, was)
                        .await;
                    if outcome.is_err() {
                        break;
                    }
                }
                return answer(reply, outcome.map(|()| keys.len() as u64));
            }
            let mut affected = 0;
            let mut outcome = Ok(());
            // Still a round trip per key: batching a delete means unioning
            // the foreign-key closures two keys can share, which is a different
            // change from `update_many`. Recorded rather than assumed away.
            for key in &keys {
                match transaction.delete(&context, definition, key).await {
                    // A row the policy hides deletes as absent, so a caller
                    // cannot use the count to find out whether it was there.
                    Ok(true) => affected += 1,
                    Ok(false) => {}
                    Err(error) => {
                        outcome = Err(error);
                        break;
                    }
                }
            }
            answer(reply, outcome.map(|()| affected))
        }
        Command::DeleteWhere {
            context,
            table,
            predicate,
            at_most,
            reply,
        } => {
            let definition = table!(table, reply);
            let outcome = transaction
                .delete_where(&context, definition, predicate, at_most)
                .await;
            answer(reply, outcome)
        }
        Command::UpdateWhere {
            context,
            table,
            predicate,
            assignments,
            at_most,
            reply,
        } => {
            let definition = table!(table, reply);
            let outcome = transaction
                .update_where(&context, definition, predicate, &assignments, at_most)
                .await;
            answer(reply, outcome)
        }
        Command::Get {
            context,
            table,
            key,
            reply,
        } => {
            let definition = table!(table, reply);
            let outcome = transaction.get(&context, definition, &key).await;
            answer(reply, outcome)
        }
        Command::Query {
            context,
            table,
            query,
            reply,
        } => {
            let definition = table!(table, reply);
            let outcome = match transaction.execute(&context, definition, &query).await {
                Ok(cursor) => cursor.collect().await,
                Err(error) => Err(error),
            };
            answer(reply, outcome)
        }
        Command::Explain {
            context,
            table,
            query,
            reply,
        } => {
            let definition = table!(table, reply);
            let outcome = transaction.explain(&context, definition, &query);
            answer(reply, outcome)
        }
        Command::MultiRead {
            context,
            tables,
            read,
            reply,
        } => {
            let definitions = match resolve_all(store, &tables) {
                Ok(definitions) => definitions,
                Err(error) => {
                    let _ = reply.send(Err(error));
                    return None;
                }
            };
            // The boundary comes back with the rows rather than being derived
            // from them: a page of a join is a page of its first input's
            // table, and a first-input row that matched nothing is read and
            // returns nothing — so the rows do not know where the page ended.
            let outcome = match open_multi(transaction, &context, &definitions, &read).await {
                Ok(mut cursor) => match cursor.collect_rows().await {
                    Ok(rows) => {
                        let first = definitions.first().copied();
                        let page = first.map_or((None, 0), |first| cursor.page_end(first));
                        Ok(MultiPage { rows, page })
                    }
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            answer(reply, outcome)
        }
        Command::ExplainMulti {
            context,
            tables,
            read,
            reply,
        } => {
            let definitions = match resolve_all(store, &tables) {
                Ok(definitions) => definitions,
                Err(error) => {
                    let _ = reply.send(Err(error));
                    return None;
                }
            };
            let outcome = match &*read {
                MultiRead::Join(join) => two_tables(&definitions).and_then(|(left, right)| {
                    transaction
                        .explain_join(&context, left, right, join)
                        .map(|plan| MultiExplanation::Join(Box::new(plan)))
                }),
                MultiRead::Chain(chain) => transaction
                    .explain_chain(&context, &definitions, chain)
                    .map(|plan| MultiExplanation::Chain(Box::new(plan))),
            };
            answer(reply, outcome)
        }
        Command::ExplainAggregate {
            context,
            read,
            reply,
        } => {
            let grouping = read.grouping();
            let outcome = match &read.source {
                GroupedSource::Table { table, query } => {
                    let definition = table!(*table, reply);
                    transaction
                        .explain_grouped(&context, definition, query, &grouping)
                        .map(|plan| GroupedExplanation::Table(Box::new(plan)))
                }
                GroupedSource::Join { left, right, join } => {
                    let left = table!(*left, reply);
                    let right = table!(*right, reply);
                    transaction
                        .explain_grouped_join(&context, left, right, join, &grouping)
                        .map(|plan| GroupedExplanation::Join(Box::new(plan)))
                }
                GroupedSource::Chain { tables, chain } => {
                    let definitions = match resolve_all(store, tables) {
                        Ok(definitions) => definitions,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            return None;
                        }
                    };
                    transaction
                        .explain_grouped_chain(&context, &definitions, chain, &grouping)
                        .map(|(narrowed, plan)| {
                            GroupedExplanation::Chain(Box::new(narrowed), Box::new(plan))
                        })
                }
            };
            answer(reply, outcome)
        }
        Command::Aggregate {
            context,
            read,
            reply,
        } => {
            let grouping = read.grouping();
            let outcome = match &read.source {
                GroupedSource::Table { table, query } => {
                    let definition = table!(*table, reply);
                    transaction
                        .grouped(&context, definition, query, &grouping)
                        .await
                }
                GroupedSource::Join { left, right, join } => {
                    let left = table!(*left, reply);
                    let right = table!(*right, reply);
                    transaction
                        .group_by_join(&context, left, right, join, &grouping)
                        .await
                }
                GroupedSource::Chain { tables, chain } => {
                    let mut definitions = Vec::with_capacity(tables.len());
                    for id in tables {
                        definitions.push(table!(*id, reply));
                    }
                    transaction
                        .group_by_chain(&context, &definitions, chain, &grouping)
                        .await
                }
            };
            answer(reply, outcome)
        }
        // Handled by the loop, which has to consume the transaction.
        Command::Commit { .. } | Command::Rollback { .. } => None,
    }
}

fn answer<T>(
    reply: oneshot::Sender<Result<T, KernelError>>,
    outcome: Result<T, KernelError>,
) -> Option<KernelError> {
    let error = match &outcome {
        Ok(_) => None,
        // Cloning is not available on `KernelError`, and the caller needs the
        // real one, so the loop gets a re-derived copy of only what it acts on.
        Err(KernelError::WriterFenced) => Some(KernelError::WriterFenced),
        Err(_) => None,
    };
    let _ = reply.send(outcome);
    error
}

/// The handler resolved this name against the same catalog a moment ago, so
/// this is a consistency check rather than a lookup — but it is one worth
/// making, because the alternative is an index into a catalog that has to be
/// assumed to match.
fn resolve<S: KvStore>(store: &RecordStore<S>, id: TableId) -> Result<&TableDef, KernelError> {
    store
        .catalog()
        .table(id)
        .ok_or(KernelError::UnknownTable(id))
}

/// Every table of a multi-table read, in the order the request declared them.
fn resolve_all<'s, S: KvStore>(
    store: &'s RecordStore<S>,
    tables: &[TableId],
) -> Result<Vec<&'s TableDef>, KernelError> {
    tables.iter().map(|id| resolve(store, *id)).collect()
}

/// Open the right kernel cursor for a multi-table read.
///
/// Two tables go through `join`, which can choose which side to hold in
/// memory; more go through `chain`, which cannot. Written once here and once
/// in `service.rs` for the replica path, because the two take different
/// receivers — a transaction and a snapshot — and there is no trait over the
/// pair to share.
async fn open_multi<'t>(
    transaction: &'t RecordTransaction<'_>,
    context: &SecurityContext,
    tables: &[&'t TableDef],
    read: &MultiRead,
) -> Result<MultiCursor<'t>, KernelError> {
    match read {
        MultiRead::Join(join) => {
            let (left, right) = two_tables(tables)?;
            Ok(MultiCursor::Join(Box::new(
                transaction.join(context, left, right, join).await?,
            )))
        }
        MultiRead::Chain(chain) => Ok(MultiCursor::Chain(
            transaction.chain(context, tables, chain).await?,
            tables.len(),
        )),
    }
}

/// Tell leadership if the storage layer says another writer has taken over.
async fn note_fencing(leadership: &Leadership, error: &KernelError) {
    if matches!(error, KernelError::WriterFenced) {
        leadership.fenced().await;
    }
}
