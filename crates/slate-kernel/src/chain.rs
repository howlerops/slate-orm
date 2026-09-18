//! Joining more than two tables.
//!
//! A chain is left-deep and explicit: `A ⋈ B ⋈ C` joins `A` to `B`, then that
//! result to `C`, in the order written. There is no join-order search. That is
//! the part of query optimisation that actually needs one — the space is
//! factorial and the estimates feeding it compound — and pretending to do it
//! badly would be worse than not doing it, because a wrong order here costs
//! round trips rather than a constant factor. The caller knows their schema;
//! [`ChainCursor`] tells them what each step cost.
//!
//! # Security is unchanged, because nothing new reads
//!
//! Every step reads its new table through [`crate::read::SecuredReads`], the
//! same path a single-table query uses: authorised, row-filtered, planned. A
//! chain is *n* secured reads rather than two, and the argument that a join
//! cannot see a hidden row is the same argument, applied once per step.
//!
//! # What a step costs
//!
//! Each step joins an accumulated result that is already in memory to one
//! table that is not. That makes the choice narrower than in a two-table join:
//!
//! - **Hash**: scan the new table once and probe it with the accumulated rows.
//! - **Loop**: probe the new table once per accumulated row.
//!
//! The same comparison as before, and the same answer: a scanned row costs a
//! hundredth of a round trip and a probe at least one, so the loop wins only
//! when the accumulated result is small — which, walking down an association
//! from one record, it is.
//!
//! # Intermediates are materialised
//!
//! The accumulated result is a `Vec`, not a stream. A two-table join streams
//! its probe side; a chain does not, because each step is the next step's
//! build side. That is a real cost and it is bounded rather than hidden: the
//! accumulated set is checked against [`Join::build_limit`] at every step.
//! Streaming the final step would be a worthwhile optimisation and is not
//! done.

use crate::error::{KernelError, Result};
use crate::expr::{Columns, Expr};
use crate::join::{
    DEFAULT_BUILD_LIMIT, JoinAlgorithm, JoinKey, JoinSchema, JoinType, PROBE_CONCURRENCY,
    join_values, probe_filter, validate_compute,
};
use crate::plan::Plan;
use crate::query::Query;
use crate::read::SecuredReads;
use crate::scalar::Scalar;
use crate::security::SecurityContext;
use crate::stats::POINT_READ_COST;
use futures::stream::{FuturesOrdered, StreamExt as _};
use slate_schema::{ColumnDef, Ordinal, Row, TableDef};
use slate_tuple::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// One row of a chain: a row from each table, absent where an outer step
/// preserved something that matched nothing.
///
/// Positional rather than named, because a chain's shape is its order. The
/// tables are kept apart rather than concatenated for the same reason a
/// two-table join keeps its sides apart: each decodes into its own type, and
/// none has to know another's width.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainRow {
    rows: Vec<Option<Row>>,
    /// What [`Chain::compute`] produced for this row, in order. Empty when the
    /// chain computes nothing.
    ///
    /// A third field rather than an entry in `rows` for the reason
    /// [`JoinedRow::computed`](crate::JoinedRow::computed) gives: a value that
    /// may read every table belongs to none of them, and giving it a table's
    /// slot would make its ordinal depend on which.
    computed: Vec<Value>,
}

impl ChainRow {
    /// A row holding only the first table's row.
    #[must_use]
    pub fn start(row: Row) -> Self {
        Self {
            rows: vec![Some(row)],
            computed: Vec::new(),
        }
    }

    /// This row extended with the next table's row, or nothing.
    #[must_use]
    pub fn extended(&self, next: Option<Row>) -> Self {
        let mut rows = self.rows.clone();
        rows.push(next);
        Self {
            rows,
            computed: Vec::new(),
        }
    }

    /// The row from the table at `position`, if there was one.
    #[must_use]
    pub fn at(&self, position: usize) -> Option<&Row> {
        self.rows.get(position).and_then(Option::as_ref)
    }

    /// Every table's row, in chain order.
    #[must_use]
    pub fn rows(&self) -> &[Option<Row>] {
        &self.rows
    }

    /// How many tables this row spans.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the row spans no tables at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether every table contributed a row.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.rows.iter().all(Option::is_some)
    }

    /// Every table's columns laid end to end, in the [`JoinSchema`] space.
    ///
    /// The same shape [`JoinedRow::flatten`](crate::JoinedRow::flatten)
    /// produces for a two-table join, generalised to a chain: a table that
    /// contributed nothing to this row reads as its full width of nulls, so an
    /// ordinal means the same thing whether or not its table matched.
    ///
    /// A row that stops short of the chain's length — which happens while a
    /// chain is being built up, and cannot happen once it is finished — is
    /// padded to the schema's full width, so the result is always the width
    /// the schema describes rather than the width this row happens to have.
    #[must_use]
    pub fn flatten(&self, schema: &JoinSchema) -> Row {
        let mut values = Vec::with_capacity(schema.width());
        for position in 0..schema.len() {
            let width = schema.width_at(position);
            match self.at(position) {
                Some(row) => {
                    values.extend(row.values().iter().take(width).cloned());
                    // A side read under a projection is as wide as its table;
                    // this guards the case where it is not, rather than
                    // producing a row that silently shifts every later ordinal.
                    values.resize(schema.at(position, Ordinal(width)).0, Value::Null);
                }
                None => values.resize(values.len() + width, Value::Null),
            }
        }
        Row::new(values)
    }

    /// What [`Chain::compute`] produced for this row, in order.
    #[must_use]
    pub fn computed(&self) -> &[Value] {
        &self.computed
    }

    /// Flattened, with [`Self::computed`] appended in order.
    ///
    /// The n-table twin of
    /// [`JoinedRow::flatten_appending`](crate::JoinedRow::flatten_appending),
    /// and the same arrangement: the values were produced once by
    /// [`run`] and are appended here rather than evaluated a second time.
    #[must_use]
    pub fn flatten_appending(&self, schema: &JoinSchema) -> Row {
        let flat = self.flatten(schema);
        if self.computed.is_empty() {
            return flat;
        }
        let mut values = flat.into_values();
        values.extend(self.computed.iter().cloned());
        Row::new(values)
    }

    /// Pad to `len` tables with absent rows.
    ///
    /// Used when an outer step preserves a row of the new table: everything
    /// accumulated before it is absent, and the shape still has to line up
    /// with the schema.
    fn absent_before(next: Row, position: usize) -> Self {
        let mut rows = vec![None; position];
        rows.push(Some(next));
        Self {
            rows,
            computed: Vec::new(),
        }
    }
}

/// A [`ChainRow`] seen through a [`JoinSchema`], for evaluating a condition
/// over several tables at once.
struct ChainView<'r> {
    schema: &'r JoinSchema,
    row: &'r ChainRow,
    /// The row about to be appended, which is not in `row` yet. A step's
    /// condition is checked before the pair is formed.
    pending: Option<&'r Row>,
}

impl Columns for ChainView<'_> {
    fn value(&self, ordinal: Ordinal) -> Option<&Value> {
        let (position, at) = self.schema.locate(ordinal)?;
        if position == self.row.len() {
            return self.pending.and_then(|row| row.get(at));
        }
        self.row.at(position).and_then(|row| row.get(at))
    }
}

/// One table added to a chain.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinStep {
    /// What to read from this table.
    pub query: Query,
    /// Equalities between an accumulated column and one of this table's.
    ///
    /// `JoinKey::left` is in the joined space — use
    /// [`JoinSchema::at`](JoinSchema::at) with the earlier table's position —
    /// and `JoinKey::right` is this table's own ordinal. That is what lets a
    /// step join back to any earlier table rather than only the one before it.
    pub on: Vec<JoinKey>,
    /// Which rows this step keeps.
    ///
    /// `Left` preserves accumulated rows that matched nothing; `Right`
    /// preserves rows of this table that matched nothing, with every earlier
    /// table absent.
    pub join_type: JoinType,
    /// A condition over the accumulated row and this table's, in the joined
    /// space. Behaves like `ON`; see [`crate::join::Join::having`].
    pub having: Expr,
    /// Force an algorithm for this step instead of costing one.
    pub force: Option<JoinAlgorithm>,
}

impl JoinStep {
    /// Join this table on `keys`.
    #[must_use]
    pub fn on<I: IntoIterator<Item = JoinKey>>(keys: I) -> Self {
        Self {
            query: Query::all(),
            on: keys.into_iter().collect(),
            join_type: JoinType::Inner,
            having: Expr::True,
            force: None,
        }
    }

    /// Join this table on one pair: an accumulated column and one of its own.
    #[must_use]
    pub fn equating(accumulated: Ordinal, own: Ordinal) -> Self {
        Self::on([JoinKey::new(accumulated, own)])
    }

    /// What to read from this table.
    #[must_use]
    pub fn query(mut self, query: Query) -> Self {
        self.query = query;
        self
    }

    /// Keep accumulated rows this table did not match.
    #[must_use]
    pub const fn left_outer(mut self) -> Self {
        self.join_type = JoinType::Left;
        self
    }

    /// Keep this table's rows that matched nothing accumulated.
    #[must_use]
    pub const fn right_outer(mut self) -> Self {
        self.join_type = JoinType::Right;
        self
    }

    /// Keep everything from both.
    #[must_use]
    pub const fn full_outer(mut self) -> Self {
        self.join_type = JoinType::Full;
        self
    }

    /// Add a condition over the accumulated row and this table's.
    #[must_use]
    pub fn having(mut self, predicate: Expr) -> Self {
        self.having = core::mem::replace(&mut self.having, Expr::True).and(predicate);
        self
    }

    /// Run this algorithm rather than the cheapest one.
    #[must_use]
    pub const fn using(mut self, algorithm: JoinAlgorithm) -> Self {
        self.force = Some(algorithm);
        self
    }
}

/// A request to join a chain of tables.
#[derive(Debug, Clone, PartialEq)]
pub struct Chain {
    /// What to read from the first table.
    pub first: Query,
    /// The tables joined onto it, in order.
    pub steps: Vec<JoinStep>,
    /// Maximum rows to return.
    pub limit: Option<usize>,
    /// Rows to discard first.
    pub offset: usize,
    /// Rows an accumulated result may hold at any step.
    pub build_limit: usize,
    /// Resume after this first-table primary key. See [`Chain::after`].
    pub after: Option<Vec<Value>>,
    /// Whether this chain will be resumed from a cursor. See [`Chain::paging`].
    pub paging: bool,
    /// Extra values computed per chain row, appended after *every* table's
    /// columns.
    ///
    /// The same field [`Join::compute`](crate::Join::compute) puts on a
    /// two-table join, and it exists for the same reason: a step's own
    /// [`Query::compute`] is appended to *that step's* row, and
    /// [`ChainRow::flatten`] packs the tables by declared width, so such a
    /// value is truncated away and the ordinal it would have taken belongs to
    /// the next table. Grouping by it answered a different question with no
    /// error anywhere, which is why `read::no_side_computes` refuses it.
    ///
    /// Until this field existed that refusal had nowhere to redirect to and had
    /// to name a missing feature. Now it names this, which is the whole point:
    /// past every table is the one place an ordinal can be added without
    /// moving one that already exists, and a value computed there can read any
    /// table in the chain rather than only its own.
    ///
    /// The `i`th sits at [`JoinSchema::computed(i)`](JoinSchema::computed), and
    /// may read every table's columns and the `i` values before it.
    pub compute: Vec<Scalar>,
}

impl Chain {
    /// A chain starting at one table, reading all of it.
    #[must_use]
    pub fn from(first: Query) -> Self {
        Self {
            first,
            steps: Vec::new(),
            limit: None,
            offset: 0,
            build_limit: DEFAULT_BUILD_LIMIT,
            compute: Vec::new(),
            after: None,
            paging: false,
        }
    }

    /// A chain starting at one table, reading every row and column.
    #[must_use]
    pub fn start() -> Self {
        Self::from(Query::all())
    }

    /// Join another table onto the chain.
    #[must_use]
    pub fn join(mut self, step: JoinStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Return at most `limit` rows.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Discard the first `offset` rows.
    #[must_use]
    pub const fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Resume after the first table's row with this primary key.
    ///
    /// The same design [`Join::after`](crate::Join::after) uses, and simpler
    /// here: a chain already materialises each step from the first table's
    /// rows, so the first step *is* the driving scan. The cursor is the first
    /// table's primary key, [`Chain::limit`] counts first-table rows, and
    /// every chain row those rows produce comes back with them.
    ///
    /// Applying the limit to the first read rather than to the finished chain
    /// is also the only version that makes paging a chain *cheaper*: truncating
    /// at the end still walks every step over every row.
    ///
    /// Refuses a right- or full-outer step, whose preserved rows have no first
    /// table and so belong to no page; an `offset` alongside the cursor; and a
    /// page with no limit. See `docs/paging-a-join.md`.
    #[must_use]
    pub fn after(mut self, first_key: impl Into<Vec<Value>>) -> Self {
        self.after = Some(first_key.into());
        self.paging = true;
        self
    }

    /// Declare that this chain will be resumed, without resuming one yet.
    ///
    /// So the first page raises the refusals the second one would, rather than
    /// the caller learning on page two that page one was never resumable.
    #[must_use]
    pub const fn paging(mut self) -> Self {
        self.paging = true;
        self
    }

    /// Rows an accumulated result may hold at any step.
    #[must_use]
    pub const fn build_limit(mut self, rows: usize) -> Self {
        self.build_limit = rows;
        self
    }

    /// Compute these values per chain row. See [`Chain::compute`].
    #[must_use]
    pub fn computing<I: IntoIterator<Item = Scalar>>(mut self, scalars: I) -> Self {
        self.compute = scalars.into_iter().collect();
        self
    }

    /// Reject a request the executor could not run.
    pub(crate) fn validate(&self, tables: &[&TableDef], schema: &JoinSchema) -> Result<()> {
        if tables.len() != self.steps.len() + 1 {
            return Err(KernelError::JoinNotSupported {
                reason: format!(
                    "{} tables were given for a chain of {} steps; a chain needs \
                     one more table than it has steps",
                    tables.len(),
                    self.steps.len()
                ),
            });
        }
        if self.steps.is_empty() {
            return Err(KernelError::JoinNotSupported {
                reason: "a chain with no steps is a single-table query; use `execute`".to_owned(),
            });
        }

        for (index, step) in self.steps.iter().enumerate() {
            let position = index + 1;
            let table = *table_at(tables, position)?;
            if step.on.is_empty() {
                return Err(KernelError::JoinNotSupported {
                    reason: format!(
                        "step {position} (`{}`) has no equality; a cross join is not offered",
                        table.name()
                    ),
                });
            }
            for key in &step.on {
                // The accumulated side must name a table already in the chain.
                // Naming a later one would be a forward reference to a row
                // that does not exist yet, and reads as null rather than
                // failing, so it is caught here.
                match schema.locate(key.left) {
                    Some((at, _)) if at < position => {}
                    Some((at, _)) => {
                        return Err(KernelError::JoinNotSupported {
                            reason: format!(
                                "step {position} (`{}`) joins to table {at}, which is \
                                 not read until later in the chain",
                                table.name()
                            ),
                        });
                    }
                    None => {
                        return Err(KernelError::JoinNotSupported {
                            reason: format!(
                                "step {position} (`{}`) names {:?}, which is outside \
                                 the chain's {} columns",
                                table.name(),
                                key.left,
                                schema.width()
                            ),
                        });
                    }
                }
                if table.column(key.right).is_none() {
                    return Err(KernelError::JoinNotSupported {
                        reason: format!("table `{}` has no column {:?}", table.name(), key.right),
                    });
                }
            }
            for column in step.having.columns() {
                match schema.locate(column) {
                    Some((at, _)) if at <= position => {}
                    _ => {
                        return Err(KernelError::JoinNotSupported {
                            reason: format!(
                                "the condition on step {position} (`{}`) names {column:?}, \
                                 which is not read by that point in the chain",
                                table.name()
                            ),
                        });
                    }
                }
            }
            let column_type = |column: Ordinal| {
                let (at, ordinal) = schema.locate(column)?;
                tables.get(at)?.column(ordinal).map(ColumnDef::value_type)
            };
            if let Some((a, b)) = step.having.column_type_conflict(&column_type) {
                return Err(KernelError::ComparisonTypeMismatch {
                    at: format!("step {position} of a chain ending at `{}`", table.name()),
                    left: a,
                    right: b,
                });
            }
        }
        let names: Vec<&str> = tables.iter().map(|t| t.name()).collect();
        validate_compute(
            schema,
            tables,
            &self.compute,
            &format!("the chain `{}`", names.join("` ⋈ `")),
        )?;
        Ok(())
    }
}

/// What one step of a chain will cost, and how it will be run.
#[derive(Debug, Clone)]
pub struct JoinStepPlan {
    /// How this step's table is read.
    pub plan: Plan,
    /// How this step combines it with what came before.
    pub algorithm: JoinAlgorithm,
    /// Rows the planner expects this step to leave accumulated.
    pub estimated_rows: f64,
    /// Estimated cost of this step, in object-storage round trips.
    pub estimated_cost: f64,
}

/// A costed plan for a whole chain.
#[derive(Debug, Clone)]
pub struct ChainPlan {
    /// How the first table is read.
    pub first: Plan,
    /// One entry per step, in chain order.
    pub steps: Vec<JoinStepPlan>,
    /// Rows the planner expects at the end.
    pub estimated_rows: f64,
    /// Estimated cost of the whole chain.
    pub estimated_cost: f64,
}

impl ChainPlan {
    /// The step the planner expects to dominate the cost.
    ///
    /// The first question about a slow chain is which link is slow, and
    /// summing the whole thing hides it.
    #[must_use]
    pub fn most_expensive_step(&self) -> Option<(usize, &JoinStepPlan)> {
        self.steps
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.estimated_cost.total_cmp(&b.estimated_cost))
    }
}

/// The table at `position`, or a refusal naming the mismatch.
///
/// A chain carries its tables separately from its steps, so the two can
/// disagree. They are checked together in [`Chain::validate`], but the
/// executor should not depend on having been validated to avoid a panic.
pub(crate) fn table_at<'t, 'd>(
    tables: &'t [&'d TableDef],
    position: usize,
) -> Result<&'t &'d TableDef> {
    tables
        .get(position)
        .ok_or_else(|| KernelError::JoinNotSupported {
            reason: format!(
                "the chain reaches table {position} but only {} were given",
                tables.len()
            ),
        })
}

/// A floor on what a probe can cost. See [`crate::join::probe_floor`].
pub(crate) fn step_probe_floor(cost: f64) -> f64 {
    cost.max(POINT_READ_COST)
}

/// The rows accumulated so far, bucketed by a step's join values.
struct Accumulated {
    rows: Vec<ChainRow>,
}

impl Accumulated {
    /// Bucket by the joined-space columns this step keys on.
    /// A row with a null join value, or one whose join column comes from a
    /// table it has absent, simply gets no bucket: it matches nothing, and an
    /// outer step still finds it through the `paired` flags.
    fn bucket(&self, schema: &JoinSchema, keys: &[JoinKey]) -> HashMap<Vec<u8>, Vec<usize>> {
        let mut buckets: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
        for (index, row) in self.rows.iter().enumerate() {
            let view = ChainView {
                schema,
                row,
                pending: None,
            };
            if let Some(key) = chain_key(&view, keys) {
                buckets.entry(key).or_default().push(index);
            }
        }
        buckets
    }
}

/// The encoded join values of an accumulated row, or `None` if any is null or
/// missing.
fn chain_key(view: &ChainView<'_>, keys: &[JoinKey]) -> Option<Vec<u8>> {
    let values: Vec<Value> = keys
        .iter()
        .map(|key| view.value(key.left).cloned())
        .collect::<Option<Vec<_>>>()?;
    let row = Row::new(values);
    let ordinals: Vec<Ordinal> = (0..keys.len()).map(Ordinal).collect();
    join_values(&row, &ordinals)
}

/// A cursor over the rows a chain produces.
///
/// The rows are already in memory by the time this exists: a chain
/// materialises each step so the next can build on it. Handing them out
/// through a cursor keeps the shape the same as every other read here, and
/// leaves room for the last step to stream later without changing callers.
pub struct ChainCursor {
    rows: std::vec::IntoIter<ChainRow>,
    schema: Arc<JoinSchema>,
    /// What each step actually accumulated, alongside what it was expected to.
    counts: Vec<usize>,
    yielded: usize,
    /// The primary key of the last row the *first* step read — the page's
    /// boundary. See [`Chain::after`].
    page_end: Option<Vec<Value>>,
}

impl core::fmt::Debug for ChainCursor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChainCursor")
            .field("steps", &self.counts)
            .field("yielded", &self.yielded)
            .finish_non_exhaustive()
    }
}

impl ChainCursor {
    /// The next row.
    #[allow(clippy::unused_async, clippy::should_implement_trait)]
    pub async fn next(&mut self) -> Result<Option<ChainRow>> {
        Ok(self.rows.next().inspect(|_| self.yielded += 1))
    }

    /// Drain into a vector.
    #[allow(clippy::unused_async)]
    pub async fn collect(self) -> Result<Vec<ChainRow>> {
        Ok(self.rows.collect())
    }

    /// Count the rows.
    #[allow(clippy::unused_async)]
    pub async fn count(self) -> Result<usize> {
        Ok(self.rows.len())
    }

    /// How many rows each step left accumulated, first table first.
    ///
    /// The planner's estimates are guesses; these are what happened. A step
    /// that was estimated at ten rows and produced ten thousand is the usual
    /// reason a chain is slow, and it is invisible without this.
    #[must_use]
    pub fn step_counts(&self) -> &[usize] {
        &self.counts
    }

    /// The ordinal space the rows are in.
    #[must_use]
    pub fn schema(&self) -> &JoinSchema {
        &self.schema
    }

    /// The primary key of the last row the first step read, if paging.
    ///
    /// A chain materialises, so unlike a join's this is known before any row
    /// is handed out — there is no lookahead to converge.
    #[must_use]
    pub fn page_end(&self) -> Option<&[Value]> {
        self.page_end.as_deref()
    }

    /// How many rows the first step read.
    ///
    /// The page size is the first step's limit, so a caller tells a full page
    /// from a short one by comparing against it. `counts[0]` is the same
    /// number; this names it for the one caller that means *the page*.
    #[must_use]
    pub fn driving_rows(&self) -> usize {
        self.counts.first().copied().unwrap_or(0)
    }
}

/// Turn a paged chain into the chain that actually runs, or say why it cannot.
///
/// The same rewrite [`crate::read::paged_join`] performs, one level up and
/// with less to do: a chain's first step is already the driving scan, so the
/// window moves from the finished chain onto `first` and nothing else changes.
fn paged_chain(chain: &Chain, first_table: &TableDef) -> Result<Chain> {
    if !chain.paging {
        return Ok(chain.clone());
    }
    let refuse = |reason: &str| {
        Err(KernelError::InvalidCursor {
            table: first_table.name().to_owned(),
            reason: reason.to_owned(),
        })
    };
    // A step preserving its own side invents a chain row with no first-table
    // row, which is in no first-table page.
    if let Some(at) = chain
        .steps
        .iter()
        .position(|step| step.join_type.preserves(crate::join::Side::Right))
    {
        return refuse(&format!(
            "step {} is a right or full outer join, which keeps rows that match nothing earlier              in the chain — and a page of a chain is a page of its first table, so those rows              belong to no page",
            at + 1,
        ));
    }
    if chain.offset != 0 {
        return refuse(
            "counting and keying are the two ways to say where a page starts, and this request              carries both. Drop the offset",
        );
    }
    let Some(limit) = chain.limit else {
        return refuse("a page needs a size, and a chain with no limit is the whole chain");
    };

    let mut paged = chain.clone();
    paged.first.limit = Some(limit);
    paged.first.after = chain.after.clone();
    paged.first.paging = true;
    // Truncating the finished chain to the same number would cut the last
    // first-table row's matches in half, which is the fan-out bug this exists
    // to avoid — and would also walk every step over every row first, which is
    // the cost it exists to avoid.
    paged.limit = None;
    Ok(paged)
}

/// Run a chain. Called through [`crate::read::SecuredReads`], which is what
/// keeps every table's read secured.
pub(crate) async fn run<'a>(
    reads: SecuredReads<'a>,
    context: &SecurityContext,
    tables: &[&'a TableDef],
    chain: &Chain,
    plan: &ChainPlan,
    schema: Arc<JoinSchema>,
) -> Result<ChainCursor> {
    let first = *table_at(tables, 0)?;
    let chain = &paged_chain(chain, first)?;
    let started = reads
        .execute(context, first, &chain.first)
        .await?
        .collect()
        .await?;
    // The boundary is taken from the rows the first step *read*, before any
    // step can drop one: under an inner step a first-table row that matches
    // nothing produces no chain row, and a page whose rows all matched nothing
    // would otherwise come back empty and with no advanced cursor — a caller
    // looping forever.
    let page_end = chain
        .paging
        .then(|| {
            started
                .last()
                .map(|row| row.primary_key_values(first).to_vec())
        })
        .flatten();
    let mut accumulated = Accumulated {
        rows: started.into_iter().map(ChainRow::start).collect(),
    };
    let mut counts = vec![accumulated.rows.len()];
    check_size(&accumulated, chain.build_limit, first)?;

    for (index, step) in chain.steps.iter().enumerate() {
        let position = index + 1;
        let table = *table_at(tables, position)?;
        let algorithm = plan.steps.get(index).map_or(
            JoinAlgorithm::Hash {
                build: crate::join::Side::Right,
            },
            |s| s.algorithm,
        );

        accumulated = match algorithm {
            JoinAlgorithm::NestedLoop => {
                probe_step(reads, context, table, step, &accumulated, &schema).await?
            }
            JoinAlgorithm::Hash { .. } => {
                hash_step(reads, context, table, step, &accumulated, &schema, position).await?
            }
        };
        counts.push(accumulated.rows.len());
        check_size(&accumulated, chain.build_limit, table)?;
    }

    let mut rows = accumulated.rows;
    if chain.offset > 0 {
        rows.drain(..chain.offset.min(rows.len()));
    }
    if let Some(limit) = chain.limit {
        rows.truncate(limit);
    }
    // After the window, as the two-table cursor does it: a computed value for a
    // row the offset threw away is work nobody asked for, and `compute` may
    // hold a regex replace. Chains materialise, so this is one pass rather than
    // per-`next` bookkeeping — which is also why `next`, `collect` and `count`
    // cannot disagree about whether the values are there.
    if !chain.compute.is_empty() {
        for row in &mut rows {
            row.computed = crate::join::computed_values(&row.flatten(&schema), &chain.compute);
        }
    }
    Ok(ChainCursor {
        rows: rows.into_iter(),
        schema,
        counts,
        yielded: 0,
        page_end,
    })
}

fn check_size(accumulated: &Accumulated, limit: usize, table: &TableDef) -> Result<()> {
    if accumulated.rows.len() > limit {
        return Err(KernelError::JoinBuildTooLarge {
            table: table.name().to_owned(),
            limit,
        });
    }
    Ok(())
}

/// Whether a formed pair survives this step's condition.
fn admits(step: &JoinStep, schema: &JoinSchema, row: &ChainRow, next: &Row) -> bool {
    if matches!(step.having, Expr::True) {
        return true;
    }
    step.having.admits_over(&ChainView {
        schema,
        row,
        pending: Some(next),
    })
}

/// Scan the new table once and probe it with the accumulated rows.
async fn hash_step<'a>(
    reads: SecuredReads<'a>,
    context: &SecurityContext,
    table: &'a TableDef,
    step: &JoinStep,
    accumulated: &Accumulated,
    schema: &JoinSchema,
    position: usize,
) -> Result<Accumulated> {
    let buckets = accumulated.bucket(schema, &step.on);
    let own: Vec<Ordinal> = step.on.iter().map(|key| key.right).collect();

    let mut out = Vec::new();
    let mut paired: Vec<bool> = vec![false; accumulated.rows.len()];

    let mut cursor = reads.execute(context, table, &step.query).await?;
    while let Some(next) = cursor.next().await? {
        let mut matched_any = false;
        if let Some(key) = join_values(&next, &own)
            && let Some(indexes) = buckets.get(&key)
        {
            for index in indexes {
                let Some(row) = accumulated.rows.get(*index) else {
                    continue;
                };
                if !admits(step, schema, row, &next) {
                    continue;
                }
                matched_any = true;
                if let Some(flag) = paired.get_mut(*index) {
                    *flag = true;
                }
                out.push(row.extended(Some(next.clone())));
            }
        }
        // A row of the new table that matched nothing accumulated. Every
        // earlier table is absent for it, which is what a right join at this
        // point in the chain means.
        if !matched_any && step.join_type.preserves(crate::join::Side::Right) {
            out.push(ChainRow::absent_before(next, position));
        }
    }

    if step.join_type.preserves(crate::join::Side::Left) {
        for (index, row) in accumulated.rows.iter().enumerate() {
            let never = !paired.get(index).copied().unwrap_or(false);
            if never {
                out.push(row.extended(None));
            }
        }
    }

    Ok(Accumulated { rows: out })
}

/// Probe the new table once per accumulated row, overlapped.
async fn probe_step<'a>(
    reads: SecuredReads<'a>,
    context: &SecurityContext,
    table: &'a TableDef,
    step: &JoinStep,
    accumulated: &Accumulated,
    schema: &JoinSchema,
) -> Result<Accumulated> {
    let base = Arc::new(step.query.clone());
    let keys = Arc::new(step.on.clone());
    let context = Arc::new(context.clone());

    let mut out = Vec::new();
    let mut inflight = FuturesOrdered::new();
    let mut queued = accumulated.rows.iter();
    let mut pending: VecDeque<&ChainRow> = VecDeque::new();

    loop {
        while inflight.len() < PROBE_CONCURRENCY {
            let Some(row) = queued.next() else { break };
            pending.push_back(row);
            // The accumulated row's join values, flattened into a one-row
            // shape the shared probe builder understands.
            let view = ChainView {
                schema,
                row,
                pending: None,
            };
            let bound: Option<Row> = keys
                .iter()
                .map(|key| view.value(key.left).cloned())
                .collect::<Option<Vec<_>>>()
                .map(Row::new);
            let context = Arc::clone(&context);
            let base = Arc::clone(&base);
            let keys = Arc::clone(&keys);
            inflight.push_back(Box::pin(async move {
                let Some(bound) = bound else {
                    return Ok(Vec::new());
                };
                // Rebased onto the one-row shape: the accumulated column is at
                // position i, the table's own column keeps its ordinal.
                let rebased: Vec<JoinKey> = keys
                    .iter()
                    .enumerate()
                    .map(|(i, key)| JoinKey::new(Ordinal(i), key.right))
                    .collect();
                let Some(filter) = probe_filter(&rebased, &bound) else {
                    return Ok(Vec::new());
                };
                let mut query = (*base).clone();
                query.filter = core::mem::replace(&mut query.filter, Expr::True).and(filter);
                // A whole secured read, like every other one here.
                reads
                    .execute(&context, table, &query)
                    .await?
                    .collect()
                    .await
            }));
        }

        let Some(result) = inflight.next().await else {
            break;
        };
        let matches: Vec<Row> = result?;
        let Some(row) = pending.pop_front() else {
            break;
        };
        let mut any = false;
        for next in matches {
            if !admits(step, schema, row, &next) {
                continue;
            }
            any = true;
            out.push(row.extended(Some(next)));
        }
        if !any && step.join_type.preserves(crate::join::Side::Left) {
            out.push(row.extended(None));
        }
    }

    Ok(Accumulated { rows: out })
}
