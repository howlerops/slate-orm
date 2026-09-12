//! The record store: primary key operations with atomic index maintenance.
//!
//! Every write puts the row and all of its index entries into one transaction,
//! so an index can never lag the table it describes. There is no background
//! index builder to fall behind and no repair path to get wrong: either the
//! whole write lands or none of it does.
//!
//! Constraints live under the same rule. A `CHECK` is evaluated before the row
//! is written, and a cascading delete puts every row it removes into the *same*
//! transaction as the delete that caused it — a cascade that committed
//! separately would leave the window in which a child points at a parent that
//! is already gone.
//!
//! # Foreign keys and row-level security
//!
//! A foreign key check reads another table, which makes it a place a caller
//! could learn about rows they cannot see. Two rules keep that shut, and they
//! point in opposite directions on purpose:
//!
//! - **Checking a reference is an ordinary secured read.** The parent row is
//!   read through the same path [`RecordTransaction::get`] uses, so a row the
//!   caller's policy hides is not there for them. Hidden and absent produce the
//!   same [`SchemaError::ForeignKeyViolation`], so the constraint answers no
//!   question the caller could not already answer. The cost is real and is the
//!   safe direction: a policy that hides the parents also stops the children
//!   being written.
//! - **Finding the rows that reference a row being deleted is not.** Integrity
//!   is not relative to who is asking — a child a policy hid from the deleter
//!   would be left pointing at nothing, which is the corruption the constraint
//!   exists to prevent. That scan therefore ignores row policy, and what it
//!   discloses is bounded rather than absent: see [`RecordTransaction::delete`].

use crate::aggregate::{Aggregate, Group};
use crate::chain::{Chain, ChainCursor, ChainPlan};
use crate::error::{KernelError, Result};
use crate::exec::QueryCursor;
use crate::explain::{Explanation, JoinExplanation};
use crate::expr::{Expr, Truth};
use crate::join::{Join, JoinCursor, JoinSchema};
use crate::keys::{self, IndexEntry};
use crate::plan::Projection;
use crate::query::Query;
use crate::read::{self, SecuredReads};
use crate::retry::{RetryPolicy, with_retries};
use crate::scalar::Scalar;
use crate::security::{Action, SecurityCatalog, SecurityContext};
use crate::stats::{ColumnStats, HISTOGRAM_SAMPLE, Histogram, Statistics, TableStats};
use crate::store::{KvReadStore, KvSnapshot, KvStore, KvTransaction, ScanOrder};
use crate::token::ReadToken;
use futures::future::BoxFuture;
use futures::stream::{FuturesOrdered, StreamExt as _};
use slate_schema::{
    Catalog, ForeignKeyDef, IndexDef, Ordinal, PartialRow, ReferentialAction, Row, SchemaError,
    TableDef, encode_body,
};
use slate_tuple::Value;
use std::collections::HashSet;

/// How many distinct values [`RecordTransaction::analyze`] counts per column
/// before giving up and calling the column unique.
pub const DISTINCT_TRACKING_LIMIT: usize = 10_000;

/// How many rows of a bulk write are looked up at once.
///
/// The same trade as the read path's prefetch: enough to hide the round trips,
/// not so many that one batch monopolises the connection pool.
pub const BULK_READ_CONCURRENCY: usize = 32;

/// How many rows one delete may cascade to before it is refused.
///
/// A cascade is held in memory and committed atomically, so the alternative to
/// a limit is being bounded by the allocator. Termination is not what this
/// guards — a row is scheduled at most once, so a cycle in the reference graph
/// stops on its own — it is size.
pub const CASCADE_LIMIT: usize = 10_000;

/// `Expr` is what a `CHECK` is written in.
///
/// The schema crate cannot name [`Expr`] — the kernel depends on it, not the
/// other way round — so it declares what a check needs of a predicate and this
/// supplies it. One expression language, one evaluator, one set of null rules.
///
/// The three-valued result is passed through rather than collapsed here.
/// `CHECK` accepts an unknown and `WHERE` withholds it, and
/// [`CheckDef::satisfied_by`](slate_schema::CheckDef::satisfied_by) is the one
/// place that difference is written down.
impl slate_schema::Predicate for Expr {
    fn truth(&self, row: &Row) -> Option<bool> {
        match self.evaluate(row) {
            Truth::True => Some(true),
            Truth::False => Some(false),
            Truth::Unknown => None,
        }
    }

    /// So the planner can read a partial index's predicate rather than only run
    /// it. See [`slate_schema::Predicate::as_any`], and `plan::implies` for
    /// what it is read for.
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

impl slate_schema::Computed for Scalar {
    fn value(&self, row: &Row) -> Value {
        self.evaluate(row)
    }

    /// So the planner can match the index's expression against what a query
    /// computes. See [`slate_schema::Computed::as_any`].
    fn as_any(&self) -> Option<&dyn core::any::Any> {
        Some(self)
    }
}

/// An expression index's key as a scalar, when it is one.
///
/// `None` for an index keyed on some other implementation of
/// [`slate_schema::Computed`]. Such an index is maintained correctly — the
/// write path only needs to run the expression — but the planner cannot match
/// it against a query, so it is never chosen. That is the same asymmetry
/// [`index_predicate`] has and the same safe direction: an unmatched index
/// costs a scan.
#[must_use]
pub fn index_expression(index: &slate_schema::IndexDef) -> Option<&Scalar> {
    index.expression()?.as_any()?.downcast_ref::<Scalar>()
}

/// A partial index's predicate as an expression, when it is one.
///
/// An index whose predicate is some other implementation of
/// [`slate_schema::Predicate`] — a closure, say — is one the planner cannot
/// reason about, and this returns `None` for it. The caller must then treat the
/// index as unusable rather than as unrestricted: the entries are missing
/// either way.
#[must_use]
pub fn index_predicate(index: &slate_schema::IndexDef) -> Option<&Expr> {
    index.predicate()?.as_any()?.downcast_ref::<Expr>()
}

/// A small deterministic generator, for sampling during `analyze`.
///
/// Deterministic because statistics that move between runs make plans that
/// move between runs. Not for anything that needs to be unpredictable: this
/// picks which rows describe a column, and nothing else.
struct Xorshift(u64);

impl Xorshift {
    const fn new() -> Self {
        // Any non-zero seed will do; a fixed one makes `analyze` reproducible.
        Self(0x2545_F491_4F6C_DD1D)
    }

    const fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A value in `0..bound`, or zero when there is no room.
    const fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 { 0 } else { self.next() % bound }
    }
}

/// A typed record store over a key-value backend.
#[derive(Debug)]
pub struct RecordStore<S> {
    store: S,
    catalog: Catalog,
    security: SecurityCatalog,
    retry: RetryPolicy,
    statistics: Statistics,
}

impl<S> RecordStore<S> {
    /// Create a record store over `store`, serving `catalog` under `security`.
    ///
    /// An empty [`SecurityCatalog`] denies every non-superuser action, so a
    /// store wired up without rules is closed rather than open.
    pub const fn new(store: S, catalog: Catalog, security: SecurityCatalog) -> Self {
        Self {
            store,
            catalog,
            security,
            retry: RetryPolicy::DEFAULT,
            statistics: Statistics::new(),
        }
    }

    /// Supply table statistics for the planner.
    ///
    /// Without these every table is assumed to hold a thousand rows with a
    /// hundred distinct values per column, which is wrong for any real table
    /// but is at least a claim about *data* rather than about the shape of a
    /// predicate. See [`Statistics`] and [`RecordTransaction::analyze`].
    #[must_use]
    pub fn with_statistics(mut self, statistics: Statistics) -> Self {
        self.statistics = statistics;
        self
    }

    /// The statistics the planner is using.
    pub const fn statistics(&self) -> &Statistics {
        &self.statistics
    }

    /// Replace the statistics, for example after re-analysing.
    pub fn set_statistics(&mut self, statistics: Statistics) {
        self.statistics = statistics;
    }

    /// Change how [`RecordStore::transact`] retries conflicts.
    #[must_use]
    pub const fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The retry policy [`RecordStore::transact`] uses.
    pub const fn retry_policy(&self) -> RetryPolicy {
        self.retry
    }

    /// The catalog this store serves.
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The access rules this store enforces.
    pub const fn security(&self) -> &SecurityCatalog {
        &self.security
    }

    /// The underlying key-value store.
    pub const fn backend(&self) -> &S {
        &self.store
    }
}

impl<S: KvReadStore> RecordStore<S> {
    /// Open a read-only view.
    ///
    /// Available for any backend that can be read, including a replica that
    /// cannot be written. The security checks are the same ones the writer
    /// applies, because both go through the same read layer.
    pub async fn snapshot(&self) -> Result<RecordSnapshot<'_>> {
        Ok(RecordSnapshot {
            snapshot: self.store.snapshot().await?,
            catalog: &self.catalog,
            security: &self.security,
            statistics: &self.statistics,
        })
    }

    /// The highest sequence number this backend's reads are guaranteed to
    /// include, when it tracks one. See [`KvReadStore::visible_sequence`].
    pub fn visible_sequence(&self) -> Option<u64> {
        self.store.visible_sequence()
    }
}

impl<S: KvStore> RecordStore<S> {
    /// Run `operation` in a transaction, retrying it if it loses a conflict.
    ///
    /// This is the intended way to write. Conflicts are ordinary here — a unique
    /// index is enforced by two writers colliding on one key — so the retry loop
    /// belongs in one place rather than at every call site.
    ///
    /// `operation` may run more than once and gets a fresh transaction each
    /// time, so it must do its own reading rather than close over values read
    /// earlier: a retry that reused the previous snapshot would commit a
    /// decision made from data that has since changed. It should also avoid
    /// side effects outside the transaction, for the same reason.
    ///
    /// ```no_run
    /// # use slate_kernel::{KvStore, RecordStore, Result, SecurityContext};
    /// # use slate_schema::{Row, TableDef};
    /// # async fn example<S: KvStore>(
    /// #     store: &RecordStore<S>, ctx: &SecurityContext, table: &TableDef, row: &Row,
    /// # ) -> Result<()> {
    /// store
    ///     .transact(async |txn| txn.insert(ctx, table, row).await)
    ///     .await
    /// # }
    /// ```
    pub async fn transact<F, T>(&self, operation: F) -> Result<T>
    where
        // `AsyncFn` rather than a boxed-future bound: the body borrows the
        // transaction it is handed *and* the caller's locals, and a
        // `for<'t> Fn(&'t _) -> BoxFuture<'t, _>` bound forces those locals to
        // be `'static`, which makes the helper unusable for the case it exists
        // to serve.
        F: AsyncFn(&RecordTransaction<'_>) -> Result<T>,
    {
        self.transact_tracked(operation)
            .await
            .map(|(value, _)| value)
    }

    /// [`RecordStore::transact`], also returning the commit's read token.
    ///
    /// Use this when the caller will read its own write back from a replica;
    /// the token is what a replica checks before serving that read.
    pub async fn transact_tracked<F, T>(&self, operation: F) -> Result<(T, Option<ReadToken>)>
    where
        F: AsyncFn(&RecordTransaction<'_>) -> Result<T>,
    {
        with_retries(self.retry, |_attempt| async {
            let txn = self.begin().await?;
            match operation(&txn).await {
                Ok(value) => {
                    let token = txn.commit().await?;
                    Ok((value, token))
                }
                Err(error) => {
                    txn.rollback();
                    Err(error)
                }
            }
        })
        .await
    }

    /// [`RecordStore::transact`] for a caller the `AsyncFn` bound cannot serve.
    ///
    /// Inside a `#[async_trait]` method the trait's own futures are boxed with
    /// a `Send` bound, and the compiler cannot prove `Send` for a
    /// higher-ranked future built over a borrowed transaction. It reports
    /// "implementation of `Send` is not general enough", pointing at the
    /// method signature and naming a type the caller never wrote — `&'0 u64`,
    /// say — and there is no way to annotate around it. `transact.rs` shows the
    /// exact failure. This exists for that position and no other.
    ///
    /// The lifetime on the transaction is named, not higher-ranked
    /// (`&'t RecordTransaction<'a>`, not `&'t RecordTransaction<'t>`), and the
    /// difference is the whole usability of this function. Quantifying it too
    /// compiles, and then forces every capture the body borrows to be
    /// `'static` — which fails at the one call site this exists for, since the
    /// head node's autocommit hands the body a `&SecurityContext` and a batch
    /// of rows it does not own. As written the body may borrow the caller's
    /// locals; they need only outlive the call.
    ///
    /// What does remain is that the closure is `Fn`, because it runs once per
    /// attempt: the future cannot *move* out of it. Borrow the captures
    /// (`let write = &write;`) or clone them per attempt.
    ///
    /// ```no_run
    /// # use slate_kernel::{KvStore, RecordStore, Result, SecurityContext};
    /// # use slate_schema::{Row, TableDef};
    /// # async fn example<S: KvStore>(
    /// #     store: &RecordStore<S>, ctx: SecurityContext, table: TableDef, row: Row,
    /// # ) -> Result<()> {
    /// let (ctx, table, row) = (&ctx, &table, &row);
    /// store
    ///     .transact_boxed(move |txn| {
    ///         Box::pin(async move { txn.insert(ctx, table, row).await })
    ///     })
    ///     .await
    /// # }
    /// ```
    pub async fn transact_boxed<'a, F, T>(&'a self, operation: F) -> Result<T>
    where
        F: for<'t> Fn(&'t RecordTransaction<'a>) -> BoxFuture<'t, Result<T>>,
    {
        self.transact_boxed_tracked(operation)
            .await
            .map(|(value, _)| value)
    }

    /// [`RecordStore::transact_boxed`], also returning the commit's read token.
    pub async fn transact_boxed_tracked<'a, F, T>(
        &'a self,
        operation: F,
    ) -> Result<(T, Option<ReadToken>)>
    where
        F: for<'t> Fn(&'t RecordTransaction<'a>) -> BoxFuture<'t, Result<T>>,
    {
        with_retries(self.retry, |_attempt| async {
            let txn = self.begin().await?;
            match operation(&txn).await {
                Ok(value) => {
                    let token = txn.commit().await?;
                    Ok((value, token))
                }
                Err(error) => {
                    txn.rollback();
                    Err(error)
                }
            }
        })
        .await
    }

    /// Begin a record-level transaction.
    ///
    /// Prefer [`RecordStore::transact`], which handles the conflict retry that
    /// a correct writer needs anyway.
    pub async fn begin(&self) -> Result<RecordTransaction<'_>> {
        Ok(RecordTransaction {
            txn: self.store.begin().await?,
            catalog: &self.catalog,
            security: &self.security,
            statistics: &self.statistics,
        })
    }
}

/// What a bulk write does about a primary key that is, or is not, taken.
///
/// One code path serves all three, because everything else about them is the
/// same — the same validation, the same batched reads, the same all-or-nothing
/// rule — and three copies of that would be three places for a check to go
/// missing from one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BulkMode {
    /// `insert_many`: a taken key is a duplicate.
    Insert,
    /// `upsert_many`: a taken key is replaced, a free one is filled.
    Upsert,
    /// `update_many`: a taken key is replaced, a free one is an error.
    Update,
}

impl BulkMode {
    /// Whether the batch may create a row that was not there.
    const fn may_insert(self) -> bool {
        matches!(self, Self::Insert | Self::Upsert)
    }

    /// Whether the batch may overwrite a row that was.
    const fn may_replace(self) -> bool {
        matches!(self, Self::Upsert | Self::Update)
    }
}

/// A transaction over a [`RecordStore`].
///
/// Writes are buffered until [`RecordTransaction::commit`]. Reads see the
/// transaction's own uncommitted writes.
pub struct RecordTransaction<'a> {
    txn: Box<dyn KvTransaction + Send + 'a>,
    catalog: &'a Catalog,
    security: &'a SecurityCatalog,
    statistics: &'a Statistics,
}

impl core::fmt::Debug for RecordTransaction<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecordTransaction").finish_non_exhaustive()
    }
}

impl<'a> RecordTransaction<'a> {
    /// The catalog this transaction resolves tables against.
    #[must_use]
    pub const fn catalog(&self) -> &'a Catalog {
        self.catalog
    }

    /// The access rules this transaction enforces.
    #[must_use]
    pub const fn security(&self) -> &'a SecurityCatalog {
        self.security
    }

    /// The underlying key-value transaction.
    ///
    /// Exposed so the executor can scan without going back through the store.
    #[must_use]
    pub fn raw(&self) -> &(dyn KvTransaction + Send + 'a) {
        self.txn.as_ref()
    }

    /// The read half of this transaction, sharing its snapshot.
    fn reads(&self) -> SecuredReads<'_> {
        SecuredReads {
            snapshot: self.snapshot(),
            security: self.security,
            statistics: self.statistics,
        }
    }

    /// This transaction viewed as a plain snapshot.
    fn snapshot(&self) -> &(dyn KvSnapshot + 'a) {
        self.txn.as_ref()
    }

    /// Read one row by primary key.
    ///
    /// A row the caller's policy hides reads as absent, so this cannot be used
    /// to test whether a forbidden row exists.
    pub async fn get(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        self.reads().get(context, table, primary_key).await
    }

    /// Run `query`, with the caller's security filter folded in.
    ///
    /// The policy is conjoined onto the filter *before* planning, so it can
    /// narrow the scan as well as filter it, and it is re-evaluated on every
    /// candidate row regardless.
    pub async fn execute<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        query: &Query,
    ) -> Result<QueryCursor<'q>> {
        self.reads().execute(context, table, query).await
    }

    /// Every row matching `filter`, in `order`.
    pub async fn query<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<QueryCursor<'q>> {
        self.execute(context, table, &Query::all().filter(filter).order(order))
            .await
    }

    /// [`RecordTransaction::query`], reading only the columns named.
    ///
    /// When an index holds every column the query touches, the row lookup is
    /// skipped entirely. Columns outside the projection come back null.
    pub async fn query_projected<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
        projection: &Projection,
    ) -> Result<QueryCursor<'q>> {
        let mut query = Query::all().filter(filter).order(order);
        query.projection = projection.clone();
        self.execute(context, table, &query).await
    }

    /// The plan `query` would run under, without running it.
    pub fn explain(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<Explanation> {
        let plan = self.reads().plan(context, table, query)?;
        Ok(Explanation::of(table, &plan, query))
    }

    /// Join two tables on equal columns.
    ///
    /// Each side is read through its own secured plan, so each is authorised
    /// and each carries its own row filter. A join does not widen what the
    /// caller can see; it is two reads the caller could already have made.
    ///
    /// The planner picks a hash join or a nested loop by cost. See
    /// [`crate::join`].
    pub async fn join<'q>(
        &'q self,
        context: &SecurityContext,
        left: &'q TableDef,
        right: &'q TableDef,
        join: &Join,
    ) -> Result<JoinCursor<'q>> {
        self.reads().join(context, left, right, join).await
    }

    /// The plan `join` would run under, without running it.
    pub fn explain_join(
        &self,
        context: &SecurityContext,
        left: &TableDef,
        right: &TableDef,
        join: &Join,
    ) -> Result<JoinExplanation> {
        let plan = self.reads().plan_join(context, left, right, join)?;
        Ok(JoinExplanation::of(left, right, &plan, join))
    }

    /// Join a chain of tables, in the order given.
    ///
    /// `tables` is the chain in order and must be one longer than the chain's
    /// steps: the first table, then the table each step adds. Every one is
    /// read through its own secured plan. See [`crate::chain`].
    pub async fn chain(
        &self,
        context: &SecurityContext,
        tables: &[&TableDef],
        chain: &Chain,
    ) -> Result<ChainCursor> {
        self.reads().chain(context, tables, chain).await
    }

    /// The plan `chain` would run under, without running it.
    pub fn explain_chain(
        &self,
        context: &SecurityContext,
        tables: &[&TableDef],
        chain: &Chain,
    ) -> Result<ChainPlan> {
        let schema = JoinSchema::over(tables.iter().copied());
        self.reads().plan_chain(context, tables, chain, &schema)
    }

    /// Compute `aggregates` over the rows `query` selects.
    ///
    /// The projection is narrowed to exactly the columns the aggregates read,
    /// so an index holding them answers without reading any row —
    /// `COUNT(*)` reads no columns at all.
    pub async fn aggregate(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>> {
        self.reads()
            .aggregate(context, table, query, aggregates)
            .await
    }

    /// Rows matching `query`, counted.
    pub async fn count(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<u64> {
        let values = self
            .aggregate(context, table, query, &[Aggregate::Count])
            .await?;
        Ok(match values.first() {
            Some(Value::U64(n)) => *n,
            _ => 0,
        })
    }

    /// Compute `aggregates` per distinct combination of `group`, ordered by
    /// the grouping columns.
    pub async fn group_by(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>> {
        self.reads()
            .group_by(context, table, query, group, aggregates, &Expr::True)
            .await
    }

    /// [`RecordTransaction::group_by`], keeping only the groups `having`
    /// admits.
    ///
    /// The predicate is evaluated over the group rather than a row: its
    /// grouping values come first and its aggregates after, so
    /// [`Group::aggregate`] names the one to test. That is how `HAVING
    /// COUNT(*) > 100` is written here.
    pub async fn group_by_having(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
        having: &Expr,
    ) -> Result<Vec<Group>> {
        self.reads()
            .group_by(context, table, query, group, aggregates, having)
            .await
    }

    /// Collect statistics for `table` by reading it.
    ///
    /// The planner needs to know how many rows a predicate selects, and there
    /// is no way to know without looking. This is the equivalent of `ANALYZE`:
    /// run it after a bulk load, and periodically after that.
    ///
    /// It is an ordinary read, so it sees what `context` is allowed to see.
    /// Statistics gathered under a restrictive policy describe that slice
    /// rather than the table, which would make the planner optimise for the
    /// wrong shape — analyse as a superuser unless you mean otherwise.
    ///
    /// Distinct values are counted exactly up to
    /// [`DISTINCT_TRACKING_LIMIT`]; a column with more than that is treated as
    /// unique, which is the right answer for the identifiers and timestamps
    /// that usually exceed it.
    pub async fn analyze(&self, context: &SecurityContext, table: &TableDef) -> Result<TableStats> {
        let column_count = table.columns().len();
        let mut distinct: Vec<HashSet<Vec<u8>>> = vec![HashSet::new(); column_count];
        let mut overflowed = vec![false; column_count];
        let mut nulls = vec![0u64; column_count];
        let mut samples: Vec<Vec<Value>> = vec![Vec::new(); column_count];
        let mut row_count = 0u64;
        // Reservoir sampling, so the sample describes the whole table rather
        // than its first ten thousand rows. That distinction matters here
        // because a scan arrives in key order, and any column correlated with
        // the key would otherwise be described by one end of its own range.
        //
        // The generator is deterministic and unseeded on purpose: analysing
        // the same data twice gives the same statistics, and a planner whose
        // choices move between runs is one nobody can reason about.
        let mut rng = Xorshift::new();

        let mut cursor = self.execute(context, table, &Query::all()).await?;
        while let Some(row) = cursor.next().await? {
            row_count += 1;
            for (ordinal, value) in row.values().iter().enumerate() {
                if value.is_null() {
                    if let Some(count) = nulls.get_mut(ordinal) {
                        *count += 1;
                    }
                    continue;
                }
                let Some(seen) = distinct.get_mut(ordinal) else {
                    continue;
                };
                if overflowed.get(ordinal).copied().unwrap_or(false) {
                    continue;
                }
                if seen.len() >= DISTINCT_TRACKING_LIMIT {
                    // Stop counting and stop paying for the set.
                    seen.clear();
                    seen.shrink_to_fit();
                    if let Some(flag) = overflowed.get_mut(ordinal) {
                        *flag = true;
                    }
                    continue;
                }
                seen.insert(slate_tuple::encode(core::slice::from_ref(value)));
            }

            // Sampled separately from the distinct count, which stops early on
            // a high-cardinality column; a histogram wants values from
            // exactly those.
            for (ordinal, value) in row.values().iter().enumerate() {
                if value.is_null() {
                    continue;
                }
                let Some(reservoir) = samples.get_mut(ordinal) else {
                    continue;
                };
                if reservoir.len() < HISTOGRAM_SAMPLE {
                    reservoir.push(value.clone());
                } else {
                    // Algorithm R: the nth row replaces a held value with
                    // probability sample/n, which keeps every row equally
                    // likely to be in the sample.
                    let at = rng.below(row_count);
                    if let Ok(at) = usize::try_from(at)
                        && let Some(slot) = reservoir.get_mut(at)
                    {
                        *slot = value.clone();
                    }
                }
            }
        }

        let mut stats = TableStats::with_row_count(row_count);
        for ordinal in 0..column_count {
            let null_count = nulls.get(ordinal).copied().unwrap_or(0);
            let counted = distinct.get(ordinal).map_or(0, HashSet::len) as u64;
            let distinct_values = if overflowed.get(ordinal).copied().unwrap_or(false) {
                row_count.max(1)
            } else {
                counted.max(1)
            };
            stats = stats.with_column(
                Ordinal(ordinal),
                ColumnStats {
                    distinct: distinct_values,
                    null_fraction: if row_count == 0 {
                        0.0
                    } else {
                        null_count as f64 / row_count as f64
                    },
                },
            );
            if let Some(sample) = samples.get_mut(ordinal)
                && let Some(histogram) = Histogram::from_values(core::mem::take(sample))
            {
                stats = stats.with_histogram(Ordinal(ordinal), histogram);
            }
        }
        Ok(stats)
    }

    /// Read a row with no authorisation or policy applied, for the write paths
    /// that need to see the row they are about to replace.
    async fn read_row_unchecked(
        &self,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        read::read_row_unchecked(self.snapshot(), table, primary_key).await
    }

    /// Insert a row, failing if its primary key is already taken.
    ///
    /// A column with a `DEFAULT` is filled in by [`PartialRow`] before the row
    /// gets here — see [`RecordTransaction::insert_partial`].
    pub async fn insert(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: &Row,
    ) -> Result<()> {
        self.security.authorize(context, table, Action::Insert)?;
        row.validate(table)?;
        self.check_row(context, table, Action::Insert, row)?;
        // Before the reads: a row that no `CHECK` accepts should not cost a
        // round trip to find that out.
        check_constraints(table, row)?;

        let primary_key = row.primary_key_values(table);
        if self
            .read_row_unchecked(table, &primary_key)
            .await?
            .is_some()
        {
            return Err(KernelError::DuplicatePrimaryKey {
                table: table.name().to_owned(),
            });
        }
        self.check_foreign_keys(context, table, row, None, &HashSet::new())
            .await?;
        self.write_row(table, row, None).await
    }

    /// Insert a row, supplying each unset column's `DEFAULT`.
    ///
    /// This is the only place a default can be applied, because it is the only
    /// place that knows a column was *not supplied* — a [`Row`] is full width
    /// and its nulls are values a caller meant.
    pub async fn insert_partial(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: PartialRow,
    ) -> Result<()> {
        let row = row.into_row(table)?;
        self.insert(context, table, &row).await
    }

    /// Replace an existing row, failing if there is nothing to replace.
    ///
    /// The row's own primary key values identify which row is being replaced,
    /// so an update never moves a row to a different key.
    pub async fn update(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: &Row,
    ) -> Result<()> {
        self.security.authorize(context, table, Action::Update)?;
        row.validate(table)?;

        let primary_key = row.primary_key_values(table);
        let existing = self
            .visible_row(context, table, Action::Update, &primary_key)
            .await?;
        let Some(existing) = existing else {
            return Err(KernelError::RowNotFound {
                table: table.name().to_owned(),
            });
        };
        // The row the update leaves behind must also be one the caller could
        // have written, or a policy could be escaped by editing your way out.
        self.check_row(context, table, Action::Update, row)?;
        check_constraints(table, row)?;
        self.check_foreign_keys(context, table, row, Some(&existing), &HashSet::new())
            .await?;
        self.write_row(table, row, Some(existing)).await
    }

    /// Insert the row, or replace it if its primary key is already present.
    pub async fn upsert(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: &Row,
    ) -> Result<()> {
        self.security.authorize(context, table, Action::Insert)?;
        self.security.authorize(context, table, Action::Update)?;
        row.validate(table)?;

        let primary_key = row.primary_key_values(table);
        // Read without the policy: a hidden row still occupies the key, so
        // treating it as absent would turn an upsert into a silent overwrite of
        // a row the caller may not touch.
        let existing = self.read_row_unchecked(table, &primary_key).await?;
        let action = if existing.is_some() {
            Action::Update
        } else {
            Action::Insert
        };
        if let Some(current) = &existing
            && !self
                .security
                .permits_row(context, table, Action::Update, current)?
        {
            return Err(KernelError::RowNotFound {
                table: table.name().to_owned(),
            });
        }
        self.check_row(context, table, action, row)?;
        check_constraints(table, row)?;
        self.check_foreign_keys(context, table, row, existing.as_ref(), &HashSet::new())
            .await?;
        self.write_row(table, row, existing).await
    }

    /// Insert many rows, failing if any primary key is already taken.
    ///
    /// The same checks as [`RecordTransaction::insert`], but the reads they
    /// need are issued together rather than one at a time. A single insert
    /// costs a round trip to tell a duplicate key from a new one; a thousand
    /// inserts should not cost a thousand round trips of waiting.
    ///
    /// Rows are checked against each other as well as against storage — two
    /// rows in one batch sharing a primary key is a duplicate too, and would
    /// otherwise be silently resolved by whichever was written last.
    pub async fn insert_many(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        rows: &[Row],
    ) -> Result<()> {
        self.write_many(context, table, rows, BulkMode::Insert)
            .await
    }

    /// Insert or replace many rows.
    ///
    /// As [`RecordTransaction::insert_many`], except an existing row is
    /// replaced rather than refused.
    pub async fn upsert_many(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        rows: &[Row],
    ) -> Result<()> {
        self.write_many(context, table, rows, BulkMode::Upsert)
            .await
    }

    /// Replace many rows, failing if any of them is not there.
    ///
    /// As [`RecordTransaction::update_many`]'s single-row counterpart: the
    /// row's own primary key says which row is being replaced, so an update
    /// never moves a row to a different key. What the batch buys is the same
    /// thing `insert_many` buys — the reads that decide whether each row exists
    /// are issued together, rather than one round trip per row.
    ///
    /// A batch is one statement, so a single missing row fails the whole thing
    /// rather than updating the rest. Anything else would leave the caller
    /// holding an error and a partially applied change with no way to tell
    /// which rows landed.
    pub async fn update_many(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        rows: &[Row],
    ) -> Result<()> {
        self.write_many(context, table, rows, BulkMode::Update)
            .await
    }

    async fn write_many(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        rows: &[Row],
        mode: BulkMode,
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        // An update writes no new key, so it needs no `Insert`. Asking for one
        // would make `update_many` refuse callers the single-row `update`
        // serves, which is a behaviour difference between two spellings of the
        // same operation.
        if mode.may_insert() {
            self.security.authorize(context, table, Action::Insert)?;
        }
        if mode.may_replace() {
            self.security.authorize(context, table, Action::Update)?;
        }

        // Validate everything before reading anything: a batch that cannot be
        // written should not spend round trips discovering that.
        let mut primary_keys = Vec::with_capacity(rows.len());
        let mut seen: HashSet<Vec<u8>> = HashSet::with_capacity(rows.len());
        for row in rows {
            row.validate(table)?;
            let primary_key = row.primary_key_values(table);
            if !seen.insert(keys::row_key(table, &primary_key)) {
                return Err(KernelError::DuplicatePrimaryKey {
                    table: table.name().to_owned(),
                });
            }
            primary_keys.push(primary_key);
        }

        // Two rows in one batch can also collide on a unique index, which no
        // amount of reading storage would reveal.
        for index in table.indexes().iter().filter(|i| i.is_unique()) {
            let mut slots: HashSet<Vec<u8>> = HashSet::with_capacity(rows.len());
            for (row, primary_key) in rows.iter().zip(&primary_keys) {
                // Two rows outside a partial index cannot collide inside it.
                if !index.admits(row) {
                    continue;
                }
                let entry = keys::index_entry(table, index, &index.key_values(row), primary_key);
                if entry.enforces_uniqueness && !slots.insert(entry.key) {
                    return Err(KernelError::UniqueViolation {
                        table: table.name().to_owned(),
                        index: index.name().to_owned(),
                    });
                }
            }
        }

        // Every `CHECK` before any read, for the same reason the single-row
        // path does it: a batch that cannot be written should not spend round
        // trips discovering that.
        for row in rows {
            check_constraints(table, row)?;
        }

        let existing = self.read_rows_concurrently(table, &primary_keys).await?;
        self.check_unique_slots(table, rows, &primary_keys, &existing)
            .await?;

        let mut parents = self.visible_parents(context, table, rows).await?;
        // A batch may reference itself — a comment thread loaded in one go, an
        // org chart — and those parents are not in storage yet. The batch's own
        // keys count, which is the whole batch being one statement. They are
        // not policy-checked as parents because they do not need to be: each is
        // a row this same caller is writing, and `check_row` below already
        // requires every one of them to be a row they may have written.
        if table
            .foreign_keys()
            .iter()
            .any(|key| key.parent() == table.id())
        {
            parents.extend(primary_keys.iter().map(|key| keys::row_key(table, key)));
        }

        // Everything about every row is decided before any of it is written.
        // A check that failed halfway would leave a prefix of the batch
        // buffered, and a caller that committed anyway — having seen the error
        // and treated it as "some of this worked" — would land it.
        let mut previous_rows = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let previous = existing.get(index).and_then(Clone::clone);
            match (&previous, mode) {
                (Some(_), BulkMode::Insert) => {
                    return Err(KernelError::DuplicatePrimaryKey {
                        table: table.name().to_owned(),
                    });
                }
                // Not there to replace. The same error a hidden row gets, and
                // deliberately: which of the two it was is exactly what a
                // policy exists not to tell the caller.
                (None, BulkMode::Update) => {
                    return Err(KernelError::RowNotFound {
                        table: table.name().to_owned(),
                    });
                }
                (Some(current), BulkMode::Upsert | BulkMode::Update) => {
                    if !self
                        .security
                        .permits_row(context, table, Action::Update, current)?
                    {
                        return Err(KernelError::RowNotFound {
                            table: table.name().to_owned(),
                        });
                    }
                }
                (None, BulkMode::Insert | BulkMode::Upsert) => {}
            }
            let action = if previous.is_some() {
                Action::Update
            } else {
                Action::Insert
            };
            self.check_row(context, table, action, row)?;
            self.check_foreign_keys(context, table, row, previous.as_ref(), &parents)
                .await?;
            previous_rows.push(previous);
        }

        for (row, previous) in rows.iter().zip(previous_rows) {
            self.write_row_with(table, row, previous, false).await?;
        }
        Ok(())
    }

    /// Check every unique slot a batch would occupy, in one round of reads.
    ///
    /// Doing this per row inside the write loop costs a round trip per row per
    /// unique index, which is the cost the bulk path exists to avoid. As with
    /// the single-row path, the read is for the error message rather than the
    /// guarantee: two writers racing for one slot write the same key and the
    /// store settles it.
    async fn check_unique_slots(
        &self,
        table: &TableDef,
        rows: &[Row],
        primary_keys: &[Vec<Value>],
        existing: &[Option<Row>],
    ) -> Result<()> {
        for index in table.indexes().iter().filter(|i| i.is_unique()) {
            let mut pending: Vec<(Vec<u8>, &[Value])> = Vec::new();
            for ((row, primary_key), previous) in rows.iter().zip(primary_keys).zip(existing) {
                // No entry, no slot to contend for.
                if !index.admits(row) {
                    continue;
                }
                let entry = keys::index_entry(table, index, &index.key_values(row), primary_key);
                if !entry.enforces_uniqueness {
                    continue;
                }
                // An unchanged slot is already this row's, so there is nothing
                // to check and nothing to read. A previous row the index did
                // not hold owns no slot, so this row's is new even when the
                // indexed values did not change.
                let unchanged = previous
                    .as_ref()
                    .filter(|old| index.admits(old))
                    .is_some_and(|old| {
                        keys::index_entry(table, index, &index.key_values(old), primary_key).key
                            == entry.key
                    });
                if !unchanged {
                    pending.push((entry.key, primary_key.as_slice()));
                }
            }

            for chunk in pending.chunks(BULK_READ_CONCURRENCY) {
                let mut inflight = FuturesOrdered::new();
                for (key, _) in chunk {
                    inflight.push_back(self.txn.get(key));
                }
                let mut position = 0;
                while let Some(found) = inflight.next().await {
                    let owner = found?;
                    if let Some(stored) = owner {
                        let holder = slate_tuple::decode(&stored, &table.primary_key_types())?;
                        let expected = chunk.get(position).map(|(_, pk)| *pk).unwrap_or_default();
                        if holder != expected {
                            return Err(KernelError::UniqueViolation {
                                table: table.name().to_owned(),
                                index: index.name().to_owned(),
                            });
                        }
                    }
                    position += 1;
                }
            }
        }
        Ok(())
    }

    /// Read many rows at once, keeping the results in the order asked for.
    ///
    /// Each read is a round trip; issuing them together is the difference
    /// between a batch costing one wait and costing one per row.
    async fn read_rows_concurrently(
        &self,
        table: &TableDef,
        primary_keys: &[Vec<Value>],
    ) -> Result<Vec<Option<Row>>> {
        let mut out = Vec::with_capacity(primary_keys.len());
        for chunk in primary_keys.chunks(BULK_READ_CONCURRENCY) {
            let mut inflight = FuturesOrdered::new();
            for primary_key in chunk {
                inflight.push_back(self.read_row_unchecked(table, primary_key));
            }
            while let Some(row) = inflight.next().await {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Delete a row and every index entry that pointed at it.
    ///
    /// Returns whether a row was there to delete.
    ///
    /// # Foreign keys
    ///
    /// Rows referencing this one are dealt with in the *same transaction*:
    /// [`ReferentialAction::Restrict`] refuses the delete, and
    /// [`ReferentialAction::Cascade`] deletes them too, transitively, up to
    /// [`CASCADE_LIMIT`] rows. There is no window in which a child points at a
    /// parent that has gone.
    ///
    /// The search for referencing rows deliberately **ignores row-level
    /// security**, and it has to: a child hidden from the deleter is still a
    /// child, and skipping it would either leave a dangling reference or make
    /// `RESTRICT` pass while the thing it guards is true. Integrity is not
    /// relative to who is asking.
    ///
    /// What that discloses, stated plainly rather than waved away: a caller who
    /// may delete a parent can learn from a `RESTRICT` refusal that *something*
    /// references it, including rows their policy hides, and a `CASCADE` can
    /// remove rows they cannot read. Two things bound it. A cascade requires
    /// the caller to hold delete on the referencing table, so it cannot reach a
    /// table they have no business writing to at all. And when the child is
    /// tenant-scoped its foreign key carries the tenant — the parent's key
    /// begins with it — so the search is confined to the caller's own tenant by
    /// the key encoding rather than by a filter. What is left is deliberate:
    /// the schema author declared this consequence when they wrote `CASCADE`.
    pub async fn delete(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<bool> {
        self.security.authorize(context, table, Action::Delete)?;
        let existing = self
            .visible_row(context, table, Action::Delete, primary_key)
            .await?;
        let Some(existing) = existing else {
            return Ok(false);
        };

        // Nothing is written until the whole consequence is known. Deleting as
        // the graph is walked would leave a half-applied delete behind for a
        // caller that ignored the error and committed anyway, and would make a
        // `RESTRICT` further out depend on the order the walk happened to take.
        let doomed = self.deletion_closure(context, table, existing).await?;
        for (owner, row) in &doomed {
            self.remove_row(owner, row)?;
        }
        Ok(true)
    }

    /// Every row a delete of `row` removes, itself first.
    ///
    /// Two passes rather than one. The first follows `CASCADE` edges to a fixed
    /// point; the second checks every `RESTRICT` edge against the result. Doing
    /// them together would make the answer depend on traversal order — a row
    /// that blocks the delete when it is met before the cascade reaches it, and
    /// does not when it is met after.
    async fn deletion_closure<'t>(
        &'t self,
        context: &SecurityContext,
        table: &'t TableDef,
        row: Row,
    ) -> Result<Vec<(&'t TableDef, Row)>> {
        if self.catalog.referencing(table.id()).is_empty() {
            return Ok(vec![(table, row)]);
        }

        // Authorised from the *schema*, before a single row is read. Doing it
        // per table as the walk reaches one would make the grant a caller needs
        // depend on which children happen to exist — and an `AccessDenied` that
        // only appears when there is something to cascade to is an existence
        // oracle wearing a different error code.
        for reachable in self.cascade_reachable(table) {
            self.security
                .authorize(context, reachable, Action::Delete)?;
        }

        // The one read in this file that is deliberately unpoliced; see the
        // note on `delete`. Named here rather than threaded in, so that a grep
        // for `superuser` lands on the comment explaining why.
        let unpoliced = SecurityContext::superuser();

        let mut scheduled: HashSet<Vec<u8>> =
            HashSet::from([keys::row_key(table, &row.primary_key_values(table))]);
        let mut closure: Vec<(&'t TableDef, Row)> = vec![(table, row)];

        let mut at = 0;
        while at < closure.len() {
            let Some((parent, parent_key)) = closure
                .get(at)
                .map(|(parent, row)| (*parent, row.primary_key_values(parent)))
            else {
                break;
            };
            at += 1;

            for (child, foreign_key) in self.catalog.referencing(parent.id()) {
                if foreign_key.on_delete() != ReferentialAction::Cascade {
                    continue;
                }
                for found in self
                    .referencing_rows(&unpoliced, child, foreign_key, &parent_key)
                    .await?
                {
                    let key = keys::row_key(child, &found.primary_key_values(child));
                    // Scheduling a row at most once is what makes a cycle in
                    // the reference graph terminate. A self-referencing table
                    // is an ordinary schema, not a pathological one.
                    if !scheduled.insert(key) {
                        continue;
                    }
                    if closure.len() >= CASCADE_LIMIT {
                        return Err(SchemaError::CascadeTooLarge {
                            table: table.name().to_owned(),
                            limit: CASCADE_LIMIT,
                        }
                        .into());
                    }
                    closure.push((child, found));
                }
            }
        }

        for (parent, row) in &closure {
            let parent_key = row.primary_key_values(parent);
            for (child, foreign_key) in self.catalog.referencing(parent.id()) {
                if foreign_key.on_delete() != ReferentialAction::Restrict {
                    continue;
                }
                for found in self
                    .referencing_rows(&unpoliced, child, foreign_key, &parent_key)
                    .await?
                {
                    // A referencing row that is itself being deleted does not
                    // block: the reference goes away with it.
                    let key = keys::row_key(child, &found.primary_key_values(child));
                    if !scheduled.contains(&key) {
                        return Err(SchemaError::ForeignKeyRestricted {
                            table: parent.name().to_owned(),
                            child: child.name().to_owned(),
                            foreign_key: foreign_key.name().to_owned(),
                        }
                        .into());
                    }
                }
            }
        }

        Ok(closure)
    }

    /// Every table a cascade from `table` could reach, `table` excluded.
    ///
    /// From the catalog alone: which tables have rows in them is not part of
    /// the answer, so neither is which grants the caller turns out to need.
    fn cascade_reachable(&self, table: &TableDef) -> Vec<&'a TableDef> {
        let mut seen = vec![table.id()];
        let mut out: Vec<&'a TableDef> = Vec::new();
        let mut at = 0;
        while let Some(parent) = seen.get(at).copied() {
            at += 1;
            for (child, foreign_key) in self.catalog.referencing(parent) {
                if foreign_key.on_delete() != ReferentialAction::Cascade
                    || seen.contains(&child.id())
                {
                    continue;
                }
                seen.push(child.id());
                out.push(child);
            }
        }
        out
    }

    /// The rows of `child` whose foreign key holds `parent_key`.
    ///
    /// An ordinary planned read, so an index on the referencing columns makes
    /// this a range rather than a scan — which is the difference between a
    /// cascade costing one scan per parent row and costing rather less.
    async fn referencing_rows<'t>(
        &'t self,
        context: &SecurityContext,
        child: &'t TableDef,
        foreign_key: &ForeignKeyDef,
        parent_key: &[Value],
    ) -> Result<Vec<Row>> {
        let filter = Expr::all(
            foreign_key
                .columns()
                .iter()
                .zip(parent_key)
                .map(|(ordinal, value)| Expr::eq(*ordinal, value.clone())),
        );
        self.reads()
            .execute(context, child, &Query::all().filter(filter))
            .await?
            .collect()
            .await
    }

    /// Remove a row and every index entry that pointed at it.
    ///
    /// A partial index that does not hold the row is skipped rather than sent a
    /// delete for a key that was never written. The delete would be harmless in
    /// itself and is not harmless in a transaction: it writes the key, and so
    /// conflicts with any concurrent writer of the row that really does own
    /// that slot.
    fn remove_row(&self, table: &TableDef, row: &Row) -> Result<()> {
        for index in table.indexes().iter().filter(|index| index.admits(row)) {
            let entry = self.entry_for(table, index, row);
            self.txn.delete(entry.key)?;
        }
        self.txn
            .delete(keys::row_key(table, &row.primary_key_values(table)))?;
        Ok(())
    }

    /// Refuse a row that references a parent that is not there.
    ///
    /// `previous` is the row being replaced, when there is one: a reference it
    /// already held was checked when it was written and cannot have gone stale,
    /// because deleting the parent would have had to cascade to this row or be
    /// refused. Skipping it is a round trip saved on the common update that
    /// touches everything but the reference.
    ///
    /// `known` holds row keys of parents a bulk write has already found; see
    /// [`RecordTransaction::visible_parents`].
    async fn check_foreign_keys(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        row: &Row,
        previous: Option<&Row>,
        known: &HashSet<Vec<u8>>,
    ) -> Result<()> {
        for foreign_key in table.foreign_keys() {
            // A null anywhere in the referencing columns means the row
            // references nothing: SQL's `MATCH SIMPLE`.
            let Some(parent_key) = foreign_key.parent_key(row) else {
                continue;
            };
            if previous
                .and_then(|old| foreign_key.parent_key(old))
                .as_deref()
                == Some(parent_key.as_slice())
            {
                continue;
            }
            let parent = self.parent_of(foreign_key)?;
            if known.contains(&keys::row_key(parent, &parent_key)) {
                continue;
            }
            // A secured read, so a parent the caller's policy hides is absent
            // for them and reports identically to one that never existed. The
            // constraint therefore answers no question they could not already
            // answer for themselves.
            //
            // It is a read in the full sense, including the grant: writing a
            // row that references a table means being allowed to read that
            // table. An `AccessDenied` naming the parent discloses nothing
            // about its rows, and the alternative — a privileged read behind a
            // caller who cannot read the table at all — is the oracle.
            if self
                .reads()
                .get(context, parent, &parent_key)
                .await?
                .is_none()
            {
                return Err(SchemaError::ForeignKeyViolation {
                    table: table.name().to_owned(),
                    foreign_key: foreign_key.name().to_owned(),
                    parent: parent.name().to_owned(),
                }
                .into());
            }
        }
        Ok(())
    }

    /// The row keys of every parent a batch references and the caller can read,
    /// gathered in one round of overlapped reads.
    ///
    /// Without this a bulk insert costs a round trip per row per foreign key,
    /// which is the cost the bulk path exists to avoid. Keys are deduplicated
    /// first, because a batch of children usually points at a handful of
    /// parents.
    ///
    /// A key missing from the result is not yet a violation: it may belong to a
    /// row written earlier in this same batch, which the per-row check picks up
    /// because a transaction reads its own writes.
    async fn visible_parents(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        rows: &[Row],
    ) -> Result<HashSet<Vec<u8>>> {
        let mut wanted: Vec<(&TableDef, Vec<Value>)> = Vec::new();
        let mut asked: HashSet<Vec<u8>> = HashSet::new();
        for foreign_key in table.foreign_keys() {
            let parent = self.parent_of(foreign_key)?;
            for row in rows {
                let Some(key) = foreign_key.parent_key(row) else {
                    continue;
                };
                if asked.insert(keys::row_key(parent, &key)) {
                    wanted.push((parent, key));
                }
            }
        }

        let mut found = HashSet::with_capacity(wanted.len());
        for chunk in wanted.chunks(BULK_READ_CONCURRENCY) {
            let mut inflight = FuturesOrdered::new();
            for (parent, key) in chunk {
                inflight.push_back(self.reads().get(context, parent, key));
            }
            let mut position = 0;
            while let Some(row) = inflight.next().await {
                if row?.is_some()
                    && let Some((parent, key)) = chunk.get(position)
                {
                    found.insert(keys::row_key(parent, key));
                }
                position += 1;
            }
        }
        Ok(found)
    }

    /// The table a foreign key points at.
    ///
    /// `Catalog::from_tables` resolves every parent, so a store built the usual
    /// way cannot fail here. Failing closed matters anyway: a foreign key
    /// nobody can resolve must refuse the write rather than silently permit it.
    fn parent_of(&self, foreign_key: &ForeignKeyDef) -> Result<&'a TableDef> {
        self.catalog
            .table(foreign_key.parent())
            .ok_or(KernelError::UnknownTable(foreign_key.parent()))
    }

    /// Read a row only if the caller's policy for `action` admits it.
    ///
    /// A row that exists but is hidden comes back as `None`, which is what makes
    /// an update or delete against it report "no such row" rather than
    /// "forbidden" — the latter would confirm the row exists.
    async fn visible_row(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        let Some(row) = self.read_row_unchecked(table, primary_key).await? else {
            return Ok(None);
        };
        if self.security.permits_row(context, table, action, &row)? {
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    /// Postgres's `WITH CHECK`: refuse to store a row the policy would hide.
    fn check_row(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        action: Action,
        row: &Row,
    ) -> Result<()> {
        if self.security.permits_row(context, table, action, row)? {
            Ok(())
        } else {
            Err(KernelError::RowCheckFailed {
                table: table.name().to_owned(),
            })
        }
    }

    /// Write a row and reconcile its index entries against `previous`.
    async fn write_row(&self, table: &TableDef, row: &Row, previous: Option<Row>) -> Result<()> {
        self.write_row_with(table, row, previous, true).await
    }

    /// Write a row, optionally trusting that its unique slots have already been
    /// checked — which a bulk write does, in one batch, before writing anything.
    async fn write_row_with(
        &self,
        table: &TableDef,
        row: &Row,
        previous: Option<Row>,
        verify_unique: bool,
    ) -> Result<()> {
        let primary_key = row.primary_key_values(table);

        for index in table.indexes() {
            // A partial index holds an entry only for the rows its predicate
            // admits, so both halves of an update are conditional and they are
            // conditional separately. A row that stops matching has its entry
            // deleted and no new one written; one that starts matching has an
            // entry written and none to delete. Getting the second half wrong
            // leaves an entry pointing at a row the index is not supposed to
            // hold, which reading the index returns and every other access path
            // does not.
            let new_entry = index.admits(row).then(|| self.entry_for(table, index, row));
            let old_entry = previous
                .as_ref()
                .filter(|old| index.admits(old))
                .map(|old| self.entry_for(table, index, old));

            // An index entry only needs touching when the row's indexed values
            // changed. Rewriting an unchanged key would add a spurious
            // write-write conflict against concurrent writers of other rows
            // that happen to share the slot.
            if let (Some(old), Some(new)) = (old_entry.as_ref(), new_entry.as_ref())
                && old.key == new.key
            {
                continue;
            }

            if verify_unique
                && let Some(new) = new_entry.as_ref()
                && new.enforces_uniqueness
            {
                self.check_unique(table, index, new, &primary_key).await?;
            }
            if let Some(old) = old_entry {
                self.txn.delete(old.key)?;
            }
            if let Some(new) = new_entry {
                self.txn.put(new.key, new.value)?;
            }
        }

        self.txn
            .put(keys::row_key(table, &primary_key), encode_body(table, row))?;
        Ok(())
    }

    /// Reject a write that would put a second row in a unique index slot.
    ///
    /// This read is for the error message, not for the guarantee. A unique
    /// entry's key omits the primary key, so two racing writers produce the same
    /// key and the store's write-write conflict detection settles it regardless
    /// of isolation level. Without this check the loser would see a retryable
    /// conflict instead of being told what it collided with.
    async fn check_unique(
        &self,
        table: &TableDef,
        index: &IndexDef,
        entry: &IndexEntry,
        primary_key: &[Value],
    ) -> Result<()> {
        let Some(existing) = self.txn.get(&entry.key).await? else {
            return Ok(());
        };
        let owner = slate_tuple::decode(&existing, &table.primary_key_types())?;
        if owner != primary_key {
            return Err(KernelError::UniqueViolation {
                table: table.name().to_owned(),
                index: index.name().to_owned(),
            });
        }
        Ok(())
    }

    fn entry_for(&self, table: &TableDef, index: &IndexDef, row: &Row) -> IndexEntry {
        keys::index_entry(
            table,
            index,
            &index.key_values(row),
            &row.primary_key_values(table),
        )
    }

    /// Commit every buffered write atomically.
    ///
    /// Returns the sequence the writes landed at, or `None` if there were none.
    /// Pass it to a later read as [`Freshness::AtLeast`](crate::Freshness) to
    /// read your own writes back from a replica.
    pub async fn commit(self) -> Result<Option<ReadToken>> {
        Ok(self.txn.commit().await?.map(ReadToken::new))
    }

    /// Discard every buffered write.
    pub fn rollback(self) {
        self.txn.rollback();
    }
}

/// A read-only view of a [`RecordStore`].
///
/// This is what a read replica serves. It has no write methods at all rather
/// than write methods that fail, so routing a read to a replica cannot
/// accidentally become an attempt to write to one.
pub struct RecordSnapshot<'a> {
    snapshot: Box<dyn KvSnapshot + Send + 'a>,
    catalog: &'a Catalog,
    security: &'a SecurityCatalog,
    statistics: &'a Statistics,
}

impl core::fmt::Debug for RecordSnapshot<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecordSnapshot").finish_non_exhaustive()
    }
}

impl<'a> RecordSnapshot<'a> {
    /// Wrap a raw snapshot as a secured read view.
    ///
    /// For a router that picks the underlying store per read; see
    /// [`crate::pool::ReplicaPool`].
    #[must_use]
    pub fn over(
        snapshot: Box<dyn KvSnapshot + Send + 'a>,
        catalog: &'a Catalog,
        security: &'a SecurityCatalog,
        statistics: &'a Statistics,
    ) -> Self {
        Self {
            snapshot,
            catalog,
            security,
            statistics,
        }
    }

    /// The catalog this view resolves tables against.
    #[must_use]
    pub const fn catalog(&self) -> &'a Catalog {
        self.catalog
    }

    fn reads(&self) -> SecuredReads<'_> {
        SecuredReads {
            snapshot: self.snapshot.as_ref(),
            security: self.security,
            statistics: self.statistics,
        }
    }

    /// Read one row by primary key, subject to the caller's policy.
    pub async fn get(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        primary_key: &[Value],
    ) -> Result<Option<Row>> {
        self.reads().get(context, table, primary_key).await
    }

    /// Run `query`, with the caller's security filter folded in.
    pub async fn execute<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        query: &Query,
    ) -> Result<QueryCursor<'q>> {
        self.reads().execute(context, table, query).await
    }

    /// Every row matching `filter`, in `order`.
    pub async fn query<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
    ) -> Result<QueryCursor<'q>> {
        self.execute(context, table, &Query::all().filter(filter).order(order))
            .await
    }

    /// [`RecordSnapshot::query`], reading only the columns named. See
    /// [`RecordTransaction::query_projected`].
    pub async fn query_projected<'q>(
        &'q self,
        context: &SecurityContext,
        table: &'q TableDef,
        filter: Expr,
        order: ScanOrder,
        projection: &Projection,
    ) -> Result<QueryCursor<'q>> {
        let mut query = Query::all().filter(filter).order(order);
        query.projection = projection.clone();
        self.execute(context, table, &query).await
    }

    /// The plan `query` would run under, without running it.
    pub fn explain(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<Explanation> {
        let plan = self.reads().plan(context, table, query)?;
        Ok(Explanation::of(table, &plan, query))
    }

    /// Join two tables on equal columns.
    ///
    /// Each side is read through its own secured plan, so each is authorised
    /// and each carries its own row filter. A join does not widen what the
    /// caller can see; it is two reads the caller could already have made.
    ///
    /// The planner picks a hash join or a nested loop by cost. See
    /// [`crate::join`].
    pub async fn join<'q>(
        &'q self,
        context: &SecurityContext,
        left: &'q TableDef,
        right: &'q TableDef,
        join: &Join,
    ) -> Result<JoinCursor<'q>> {
        self.reads().join(context, left, right, join).await
    }

    /// The plan `join` would run under, without running it.
    pub fn explain_join(
        &self,
        context: &SecurityContext,
        left: &TableDef,
        right: &TableDef,
        join: &Join,
    ) -> Result<JoinExplanation> {
        let plan = self.reads().plan_join(context, left, right, join)?;
        Ok(JoinExplanation::of(left, right, &plan, join))
    }

    /// Join a chain of tables, in the order given.
    ///
    /// `tables` is the chain in order and must be one longer than the chain's
    /// steps: the first table, then the table each step adds. Every one is
    /// read through its own secured plan. See [`crate::chain`].
    pub async fn chain(
        &self,
        context: &SecurityContext,
        tables: &[&TableDef],
        chain: &Chain,
    ) -> Result<ChainCursor> {
        self.reads().chain(context, tables, chain).await
    }

    /// The plan `chain` would run under, without running it.
    pub fn explain_chain(
        &self,
        context: &SecurityContext,
        tables: &[&TableDef],
        chain: &Chain,
    ) -> Result<ChainPlan> {
        let schema = JoinSchema::over(tables.iter().copied());
        self.reads().plan_chain(context, tables, chain, &schema)
    }

    /// Compute `aggregates` over the rows `query` selects.
    ///
    /// The projection is narrowed to exactly the columns the aggregates read,
    /// so an index holding them answers without reading any row —
    /// `COUNT(*)` reads no columns at all.
    pub async fn aggregate(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        aggregates: &[Aggregate],
    ) -> Result<Vec<Value>> {
        self.reads()
            .aggregate(context, table, query, aggregates)
            .await
    }

    /// Rows matching `query`, counted.
    pub async fn count(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
    ) -> Result<u64> {
        let values = self
            .aggregate(context, table, query, &[Aggregate::Count])
            .await?;
        Ok(match values.first() {
            Some(Value::U64(n)) => *n,
            _ => 0,
        })
    }

    /// Compute `aggregates` per distinct combination of `group`, ordered by
    /// the grouping columns.
    pub async fn group_by(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
    ) -> Result<Vec<Group>> {
        self.reads()
            .group_by(context, table, query, group, aggregates, &Expr::True)
            .await
    }

    /// [`RecordTransaction::group_by`], keeping only the groups `having`
    /// admits.
    ///
    /// The predicate is evaluated over the group rather than a row: its
    /// grouping values come first and its aggregates after, so
    /// [`Group::aggregate`] names the one to test. That is how `HAVING
    /// COUNT(*) > 100` is written here.
    pub async fn group_by_having(
        &self,
        context: &SecurityContext,
        table: &TableDef,
        query: &Query,
        group: &[Ordinal],
        aggregates: &[Aggregate],
        having: &Expr,
    ) -> Result<Vec<Group>> {
        self.reads()
            .group_by(context, table, query, group, aggregates, having)
            .await
    }
}

/// Refuse a row that fails a `CHECK`.
///
/// A check passes when its predicate is *unknown*, which is the opposite of a
/// `WHERE` and is easy to get backwards; the rule itself lives in
/// [`CheckDef::satisfied_by`](slate_schema::CheckDef::satisfied_by) so there is
/// one copy of it.
fn check_constraints(table: &TableDef, row: &Row) -> Result<()> {
    for check in table.checks() {
        if !check.satisfied_by(row) {
            return Err(SchemaError::CheckViolation {
                table: table.name().to_owned(),
                check: check.name().to_owned(),
            }
            .into());
        }
    }
    Ok(())
}
