//! The record layer, in a browser.
//!
//! # What this is
//!
//! `slate-kernel` and `slate-schema`, compiled to `wasm32-unknown-unknown` and
//! driven over the kernel's own [`MemoryStore`]. Queries are planned by the
//! real planner, executed by the real executor, and the plan returned beside
//! the rows is the real [`Explanation`] — the same type the gRPC head node
//! serialises for `EXPLAIN`.
//!
//! It exists because the site could otherwise only *show* code. A reader can
//! now filter on an indexed column and an unindexed one and watch the access
//! path change, on their own query, which is the one claim this project makes
//! that is most worth being able to check.
//!
//! # What it is not
//!
//! Not a JavaScript reimplementation. That would agree with whatever its
//! author believed, which is precisely the failure the conformance runner
//! exists to catch between the three SDKs, and it would drift from the kernel
//! silently.
//!
//! There is no head node here, so nothing about leadership, replicas,
//! freshness or the wire protocol is exercised. This is the kernel, alone.
//!
//! # Why the calls are synchronous
//!
//! The kernel is async, and every future here is driven to completion with
//! [`futures::executor::block_on`]. That is safe *because of what backs it*:
//! `MemoryStore` is a `BTreeMap` behind a lock and never yields on I/O, so
//! every future is ready on first poll and `block_on` never parks. A browser
//! main thread cannot block, so this would be a hang rather than a slowdown if
//! that stopped being true — swapping in a backend that really awaits means
//! returning promises through `wasm-bindgen-futures` instead.
//!
//! The alternative, making every call return a `Promise` today, buys nothing:
//! there is no concurrency to express and it would put a `.await` in the
//! caller's way for a value that is already computed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod fixture;
pub mod sql;
pub mod taxi;

use futures::executor::block_on;
use serde::{Deserialize, Serialize};
use slate_kernel::{
    Aggregate, CalendarPart, CalendarUnit, CmpOp, Expr, Grouping, Join, JoinAlgorithm, JoinKey,
    Query, RecordStore, Scalar, ScanOrder, SortKey, TimeUnit,
    memory::MemoryStore,
    security::{Action, Grant, Principal, SecurityCatalog, SecurityContext},
    stats::Statistics,
};
use slate_schema::{Ordinal, Row, TableDef};
use slate_tuple::Value;
use wasm_bindgen::prelude::*;

/// A table, as the playground's UI needs to describe it.
#[derive(Serialize)]
struct TableInfo {
    name: String,
    columns: Vec<ColumnInfo>,
    /// Ordinals carrying a secondary index, so the UI can mark which filters
    /// have a chance of avoiding a table scan.
    indexed: Vec<u32>,
    /// Ordinals of the primary key. The workbench marks them because they are
    /// the only columns `UPDATE` and `DELETE` will match on — a reader who
    /// cannot see which column that is writes a statement that gets refused.
    #[serde(rename = "primaryKey")]
    primary_key: Vec<u32>,
}

#[derive(Serialize)]
struct ColumnInfo {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    ordinal: u32,
}

/// What the UI sends. Every field optional, because the panel builds it up.
///
/// `Serialize` as well as `Deserialize` because the workbench shows the
/// compiled spec beside the results: SQL typed in the editor is lowered onto
/// this and the reader is shown what it became, which is the only honest way
/// to offer SQL for a database that does not have any.
///
/// The `skip_serializing_if` attributes are for that panel rather than for
/// the wire — a spec printed with six empty fields buries the two that the
/// reader's query actually set.
#[derive(Deserialize, Serialize, Default, Debug, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct QuerySpec {
    pub table: String,
    /// One condition, kept for the shape the panel started with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<FilterSpec>,
    /// Several, ANDed. Where this gets interesting is that the planner does
    /// not treat them alike: one conjunct may become a scan bound and the
    /// rest stay a residual predicate evaluated per row, and the plan says
    /// which is which.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterSpec>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub offset: u64,
    /// Column ordinals to return. Empty means every column — which is also
    /// what makes an index-only scan impossible, so the UI exposes it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<u32>,
    /// Group by these columns. Empty means return rows rather than groups.
    ///
    /// Grouping is not a filter applied after the fact: the kernel narrows the
    /// projection to the group keys and the aggregates' columns, which is what
    /// lets `count(*) per zone` be answered from an index without reading a
    /// row. The plan says whether it managed to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<u32>,
    /// What to compute per group. `count(*)` when grouping with none named.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aggregates: Vec<AggregateSpec>,
    /// Values computed per row and appended after the table's own columns, so
    /// the `i`th sits at ordinal `columns().len() + i`. Everything downstream
    /// — a group key, a sort key, a HAVING — addresses one the ordinary way.
    ///
    /// This is how `hour(pickup_time)` becomes something the spec can hold:
    /// the query spec has no expression language and is not getting one, and a
    /// computed column is the kernel's own answer to that.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub compute: Vec<ComputeSpec>,
    /// Which groups survive, ANDed. Ordinals are in *group* space —
    /// `[keys..., aggregates...]` — the same space `sort` uses when there is a
    /// grouping, and not the table's. `HAVING count(*) > 100` names ordinal
    /// `group_by.len()`, not a column.
    ///
    /// A separate field from `filters` rather than a flag on it, because the
    /// two are evaluated against different things at different times: a filter
    /// can become a scan bound and skip rows before they are read, and a
    /// having cannot — the group it tests does not exist until every row is in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub having: Vec<FilterSpec>,
}

/// For `skip_serializing_if`, which needs a path rather than a closure.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FilterSpec {
    pub column: u32,
    pub op: String,
    pub value: String,
}

/// One computed column: a function of one of the table's own columns.
///
/// A named function rather than a nested expression tree. The kernel's
/// `Scalar` is a tree and could carry `hour(x) + 1`, but the SQL front end has
/// no expression grammar to produce one — `WHERE` takes `col <op> literal` and
/// nothing else — so a spec that could express more than the parser can parse
/// would be a shape nobody produces and nobody tests.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComputeSpec {
    /// `hour`, `year`, `day_of_week`, and the rest of `compute_scalar`.
    pub function: String,
    /// Which side of the join the column is on: 0 for the left table, 1 for
    /// the right. Zero on a single-table query, where there is only one.
    ///
    /// This used to be implicit and left-only. `Join::compute` is evaluated
    /// over the *joined* row and has always been able to read either side; the
    /// restriction was in this spec, which resolved every computed column's
    /// name against the left table. So `hour(pickup_time)` worked on `trips`
    /// and `hour(zones.updated_at)` was "not a column of trips".
    #[serde(default)]
    pub input: u32,
    /// The column it reads, an ordinal within the table `input` names.
    pub column: u32,
    /// Seconds to add before reading the calendar out, so a timestamp stored
    /// in UTC can be asked about at a fixed offset. Zero is UTC.
    ///
    /// This needs no kernel and no wire support, because it is already
    /// expressible: shifting a timestamp is adding to it, and `Scalar::Add`
    /// has been there since scalars arrived. `compute_scalar` wraps the column
    /// rather than passing an offset down, so every path that already handles
    /// `Add` — the planner, the wire, the covering scan — handles this one
    /// with no change at all.
    ///
    /// Mutually exclusive with [`ComputeSpec::zone`]: a fixed offset and a
    /// named zone are two answers to one question, and the parser produces at
    /// most one of them. `compute_scalar` refuses both rather than picking.
    #[serde(default)]
    pub offset: i64,
    /// An IANA zone name — `America/New_York` — whose offset *at each row's
    /// instant* is added before the calendar is read out. Empty is "no zone".
    ///
    /// This used to be refused, with a reason that was true of a timezone
    /// *database* and not of a timezone *table*: the kernel now ships the
    /// transitions for a curated list of zones, a few kilobytes of sorted
    /// integers, so daylight saving is looked up rather than guessed at. A
    /// name outside the list is still refused, and the refusal names the ones
    /// that are there.
    ///
    /// Unlike `offset`, this does need a kernel variant: the shift is not a
    /// constant, so `Scalar::Add` cannot express it. `Scalar::ZoneShift`
    /// composes the same way, so the same argument applies one level down —
    /// the planner and the wire handle a nested scalar already.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zone: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: u32,
    #[serde(default)]
    pub descending: bool,
}

/// What comes back: the rows, and the plan that produced them.
///
/// A grouped read puts its groups in `rows` rather than in a second field.
/// The two are the same shape to the reader — a header and cells — and the
/// binding returning one of two field names depending on the query is how a
/// UI ends up with two rendering paths that drift.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    rows: Vec<Vec<String>>,
    /// Milliseconds inside the kernel: planning and executing, and nothing
    /// else. Not the time to get the answer into JavaScript — see `now_ms`.
    kernel_ms: f64,
    plan: PlanInfo,
    /// How many rows the query actually returned, which is the number to
    /// compare against the plan's estimate.
    returned: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanInfo {
    table: String,
    access: String,
    index_only: bool,
    sorts: bool,
    descending: bool,
    estimated_rows: f64,
    /// Shown, and worth showing. The playground's most useful lesson is that
    /// an index is not automatically cheaper here, and the number is where
    /// that becomes visible rather than assertable.
    estimated_cost: f64,
    residual: String,
    decodes: Vec<u32>,
    display: String,
}

/// What the UI sends for a join or a grouped join.
///
/// This used to hard-code `authors.id = books.author_id`, which was defensible
/// while that was the only join in the database. With `trips` joining `zones`
/// there are two, and a spec that can only express one of them would have made
/// the zone names — the entire reason to carry a lookup table — unreachable.
///
/// The keys are still named explicitly rather than discovered: the parser
/// checks the `ON` clause against the schema, so a pair of columns that would
/// join to nothing is refused with a reason instead of returning an empty
/// result.
#[derive(Deserialize, Serialize, Default, Debug, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct JoinSpec {
    /// The left table's name. Its columns come first in a joined row.
    pub left: String,
    /// The right table's name.
    pub right: String,
    /// The join key's ordinal in the left table.
    pub left_key: u32,
    /// And in the right table.
    pub right_key: u32,
    /// Conditions on the left table, ANDed.
    pub left_where: Vec<FilterSpec>,
    /// Conditions on the right table, ANDed.
    pub right_where: Vec<FilterSpec>,
    /// Computed per joined row, appended after *both* tables' columns. See
    /// `Join::compute`.
    ///
    /// Each names its side with `ComputeSpec::input`, so an expression may read
    /// either table. It used to be able to read only the left, which was a
    /// limitation of this spec rather than of the kernel.
    pub compute: Vec<ComputeSpec>,
    /// Group by this ordinal of the **joined row**: a left column keeps its own
    /// ordinal, a right column sits at `left.columns() + n`, and a computed
    /// column at `left.columns() + right.columns() + i`. Absent means return
    /// joined rows rather than groups.
    pub group_by: Option<u32>,
    /// Computed per group. Each names its side with `AggregateSpec::input`.
    pub aggregates: Vec<AggregateSpec>,
    /// How to order the **groups** of a grouped join. Empty leaves them in the
    /// order the encoded key sorts them.
    ///
    /// A group is `[key, aggregates...]`, so `column: 0` is the key and
    /// `column: 1` is the first aggregate — a space of its own with nothing to
    /// do with either table's ordinals. That is `Grouping::sort`, and it is
    /// what `ORDER BY count(*) DESC` on a grouped join lowers onto.
    ///
    /// Only meaningful with `group_by`. An *ungrouped* join has no sort,
    /// because `Join` has no sort: the kernel orders groups and not joined
    /// rows, and the parser refuses `ORDER BY` there with that reason rather
    /// than accepting it into a field nothing reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortSpec>,
    pub limit: Option<u64>,
    /// Rows, or groups, to discard first.
    ///
    /// On a grouped join this is `Grouping::offset` and on an ungrouped one
    /// `Join::offset`, both of which the kernel has always had. This spec did
    /// not, so `LIMIT 3 OFFSET 5` was a parse error on a join and worked on a
    /// single table — a difference in the *front end* that read as a
    /// difference in the engine.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: u64,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AggregateSpec {
    pub kind: String,
    /// Which side of the join the column is on: 0 for the left table, 1 for
    /// the right. Zero on a single-table query.
    ///
    /// Like `ComputeSpec::input`, this used to be implicit -- and implicitly
    /// the *opposite* side: an aggregate's ordinal was shifted past every left
    /// column unconditionally, so it could only name the right table. Between
    /// the two defaults, "group by the hour and average the fare" over `trips
    /// JOIN zones` was not expressible: the group key had to come from the left
    /// and the aggregate from the right, and both of those columns are on
    /// `trips`.
    #[serde(default)]
    pub input: u32,
    /// Ordinal within the table `input` names, ignored by `count`.
    #[serde(default)]
    pub column: u32,
}

/// Joined rows, or groups, with the plan for either.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JoinAnswer {
    /// One entry per row: the author's columns, then the book's.
    rows: Vec<Vec<String>>,
    /// Milliseconds inside the kernel, as on [`Answer`].
    kernel_ms: f64,
    /// Present when grouped: the key columns, then one value per aggregate.
    groups: Vec<Vec<String>>,
    returned: usize,
    /// One per input, in the order the join reads them.
    inputs: Vec<InputPlan>,
    display: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InputPlan {
    table: String,
    access: String,
    index_only: bool,
    decodes: Vec<u32>,
    algorithm: String,
}

/// What one statement in the editor produced.
///
/// One shape for reads, joins and writes, because the workbench renders them
/// in one results pane and a reader who ran three statements wants one log,
/// not three dialects. The fields that do not apply are empty rather than
/// absent, so the JavaScript never branches on a missing key.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SqlResult {
    /// The statement as the reader wrote it, so the log can show it.
    sql: String,
    /// `select`, `join`, `group`, or `write`.
    kind: String,
    /// Headers for the grid.
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    returned: usize,
    /// The spec this SQL compiled to — the same JSON an SDK would send.
    /// This is the point of the exercise: the editor is a front end, and the
    /// panel showing this is what keeps that claim checkable rather than
    /// asserted.
    spec: serde_json::Value,
    plan: Option<PlanInfo>,
    inputs: Vec<InputPlan>,
    message: String,
    /// Milliseconds the kernel spent planning and executing this statement.
    ///
    /// Reported separately from whatever the page measures around the call,
    /// because the two differ by the cost of turning rows into JSON — which
    /// is nothing for twenty groups and most of the wall clock for a hundred
    /// thousand rows. A panel that showed only the outer number would be
    /// telling a reader that this database is slow at `SELECT *` when what is
    /// slow is `serde_json`.
    kernel_ms: f64,
    /// True things about the statement that are not refusals.
    ///
    /// One kind so far: an unqualified name that both of a join's tables have,
    /// which resolves left-first. That rule was documented in the parser and
    /// nowhere a reader could see it. See `sql::Parsed::warnings`.
    ///
    /// Beside the answer rather than folded into `message`, which the write
    /// path uses for what it did — a caller that wants to render warnings
    /// differently from a status line should not have to parse one string.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    error: Option<SqlFailure>,
}

#[derive(Serialize)]
struct SqlFailure {
    message: String,
    /// Byte offset into the whole editor buffer, so the UI can point at it.
    at: usize,
}

impl SqlResult {
    fn blank(sql: &str, kind: &str) -> Self {
        Self {
            sql: sql.trim().to_owned(),
            kind: kind.to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
            returned: 0,
            spec: serde_json::Value::Null,
            plan: None,
            inputs: Vec::new(),
            message: String::new(),
            kernel_ms: 0.0,
            warnings: Vec::new(),
            error: None,
        }
    }

    fn failed(sql: &str, at: usize, message: &str) -> Self {
        let mut out = Self::blank(sql, "error");
        out.error = Some(SqlFailure {
            message: message.to_owned(),
            at,
        });
        out
    }
}

/// A monotonic millisecond clock, on both targets.
///
/// Exists so the workbench can report *the kernel's* time rather than the time
/// to get an answer into JavaScript. Those are not close for a query that
/// returns a lot of rows: `SELECT * FROM trips` spends 166 ms in this binding
/// and 90 ms more in `JSON.parse`, and most of the 166 is `serde_json`
/// building an 8.4 MB string — none of which is planning or executing.
///
/// `std::time::Instant` panics on `wasm32-unknown-unknown` (there is no clock
/// source), so the browser's own `performance.now()` is imported instead. The
/// native arm exists for the tests, which is the only reason this is not
/// simply the JS call.
#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    #[wasm_bindgen(inline_js = "export function now() { return performance.now(); }")]
    extern "C" {
        fn now() -> f64;
    }
    now()
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// One prefix of the keyspace, as the viewer shows it.
///
/// "Folder" is the right word for what a reader sees and the wrong word for
/// what is there: an object store has no directories, and neither does the
/// kernel. There is one ordered key space, and a prefix is a contiguous range
/// in it. The viewer renders prefixes as folders because that is how people
/// read paths — and the byte column is what gives the lie away, since a
/// directory does not have a size.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyGroup {
    /// `rows` or `index`.
    space: String,
    /// The path a reader sees, e.g. `rows/trips` or `index/by_pickup_zone`.
    path: String,
    /// Table or index name.
    label: String,
    /// The fixed-width id inside the key, as it appears there.
    id: u32,
    keys: usize,
    key_bytes: usize,
    value_bytes: usize,
    /// A few real keys from this range, in order.
    samples: Vec<KeySample>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeySample {
    /// The key's bytes, hex, grouped as the layout describes them.
    key: String,
    /// What those bytes mean, decoded through the kernel's own decoder.
    decoded: String,
    value_bytes: usize,
}

/// A seeded kernel: two tables, an index, and the rows the site talks about.
///
/// `Debug` is hand-written rather than derived: `RecordStore` holds the whole
/// seeded dataset, so a derived one would print several thousand rows the
/// first time anybody put this in a `dbg!`.
#[wasm_bindgen]
pub struct Playground {
    store: RecordStore<MemoryStore>,
    /// A handle on the same bytes the store writes, for the keyspace viewer.
    ///
    /// `MemoryStore` is an `Arc` around the map, so this is the store's own
    /// data and not a copy of it — which is the only way the viewer can be
    /// worth anything. A snapshot taken at seed time would go stale the first
    /// time a reader inserted a row, and showing a *stale* picture of a
    /// keyspace is worse than showing none.
    bytes: MemoryStore,
    context: SecurityContext,
}

impl std::fmt::Debug for Playground {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Playground").finish_non_exhaustive()
    }
}

impl Default for Playground {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Playground {
    /// Build the store and seed it. Cheap: a few dozen rows into a map.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        // `EVERYTHING`, not `ALL`. They differ by `Action::Explain`, which is
        // deliberately not a data action: it reads statistics describing rows
        // a row policy may be hiding, so a blanket data grant does not carry
        // it and a deployment opts in per role and per table.
        //
        // The playground is the case the knob exists for — showing the plan is
        // the entire point — and it grants it explicitly rather than reaching
        // for a wider default. The first run of the tests below failed with
        // "no role grants explain on table `books`", which is the control
        // working.
        let security = SecurityCatalog::new()
            .grant(Grant::new("app", fixture::AUTHORS, Action::EVERYTHING))
            .grant(Grant::new("app", fixture::BOOKS, Action::EVERYTHING))
            .grant(Grant::new("app", taxi::TRIPS, Action::EVERYTHING))
            .grant(Grant::new("app", taxi::ZONES, Action::EVERYTHING));
        // All four tables, including the two the taxi data will fill. A
        // catalog cannot gain a table after the store is built, and a
        // workbench whose schema tree changes shape when a download lands is
        // one where every query written before it arrived stops parsing.
        let bytes = MemoryStore::new();
        let store = RecordStore::new(bytes.clone(), taxi::catalog(), security);

        // Seeded as the superuser and queried as `app`, so the playground
        // exercises the ordinary authorised path rather than the one that
        // skips the checks.
        let root = SecurityContext::superuser();
        let store = block_on(async {
            let txn = store.begin().await.expect("begin");
            txn.insert_many(&root, &fixture::authors(), &fixture::author_rows())
                .await
                .expect("seed authors");
            txn.insert_many(&root, &taxi::zones(), &taxi::zone_rows())
                .await
                .expect("seed zones");
            txn.insert_many(&root, &fixture::books(), &fixture::book_rows())
                .await
                .expect("seed books");
            txn.commit().await.expect("commit the seed");

            // `ANALYZE`, then hand the results to the planner.
            //
            // Without this the planner works from defaults and the access path
            // it picks says more about those defaults than about the data. The
            // playground's whole claim is that the plan responds to the query,
            // so the statistics behind it have to be real ones measured off
            // these rows.
            // The transaction is scoped so its borrow of `store` ends before
            // `with_statistics` consumes it.
            let stats = analyze(&store, &root).await;
            store.with_statistics(stats)
        });

        Self {
            store,
            bytes,
            context: SecurityContext::new(
                Principal::new(Value::Str("reader".to_owned())).with_role("app"),
            ),
        }
    }

    /// The schema, as JSON, so the UI can build its controls from the kernel's
    /// own catalog rather than from a copy that can drift from it.
    #[must_use]
    pub fn schema(&self) -> String {
        let tables: Vec<TableInfo> = [
            taxi::trips(),
            taxi::zones(),
            fixture::authors(),
            fixture::books(),
        ]
        .into_iter()
        .map(|table| TableInfo {
            name: table.name().to_owned(),
            columns: table
                .columns()
                .iter()
                .enumerate()
                .map(|(i, column)| ColumnInfo {
                    name: column.name().to_owned(),
                    kind: column.value_type().name().to_owned(),
                    ordinal: u32::try_from(i).unwrap_or(0),
                })
                .collect(),
            indexed: table
                .indexes()
                .iter()
                .flat_map(|index| index.columns().iter().map(|c| c.ordinal.0 as u32))
                .collect(),
            primary_key: table
                .primary_key()
                .iter()
                .map(|o| u32::try_from(o.0).unwrap_or(0))
                .collect(),
        })
        .collect();
        serde_json::to_string(&tables).expect("the schema serialises")
    }

    /// Insert a row, given one string per column.
    ///
    /// Strings because that is what an HTML form has, and because the parsing
    /// is the interesting part: each is turned into the column's declared type
    /// or refused. A `Str` written into a `U64` column would not fail at the
    /// storage layer — the kernel's value order is type-first, so it would
    /// sort among the strings and simply never match a numeric predicate.
    #[must_use]
    pub fn insert(&self, table: &str, values: &str) -> String {
        match self.write(table, values, Write::Insert) {
            Ok((message, ms)) => serde_json::json!({ "ok": message, "ms": ms }).to_string(),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }

    /// Replace a row that already exists, by primary key.
    #[must_use]
    pub fn update(&self, table: &str, values: &str) -> String {
        match self.write(table, values, Write::Update) {
            Ok((message, ms)) => serde_json::json!({ "ok": message, "ms": ms }).to_string(),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }

    /// Delete by primary key. `values` is the key alone, not a whole row.
    #[must_use]
    pub fn delete(&self, table: &str, key: &str) -> String {
        match self.remove(table, key) {
            Ok((message, ms)) => serde_json::json!({ "ok": message, "ms": ms }).to_string(),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }

    /// Join `authors` to `books`, and optionally group the result.
    ///
    /// Grouped and ungrouped go through one entry point because the plan for
    /// the two is *not* the same and the difference is the point: grouping
    /// narrows each input's projection to the group keys and the aggregates'
    /// columns, so a `count(*)` per author can be answered without reading a
    /// book row at all. Two entry points would let the panel show the wrong
    /// one beside the right rows.
    #[must_use]
    pub fn join(&self, spec: &str) -> String {
        match self.joined(spec) {
            Ok(answer) => serde_json::to_string(&answer).expect("the answer serialises"),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }

    /// Run a buffer of SQL, one statement at a time.
    ///
    /// Returns an array with one entry per statement, so the editor can show
    /// a log of what happened alongside the last statement's rows. Errors are
    /// entries too, never exceptions: everything in here is something the
    /// reader typed, and a refusal with the offset that caused it is more use
    /// than a stack trace in a console.
    #[must_use]
    pub fn sql(&self, text: &str) -> String {
        serde_json::to_string(&self.run_sql(text)).expect("the results serialise")
    }

    /// Load the packed trip file, seed `trips`, and re-measure.
    ///
    /// Separate from the constructor because it arrives separately: the page
    /// is usable on the books fixture while 1.2 MB is in flight, and a
    /// visitor who never gets the file gets a working workbench with an empty
    /// `trips` rather than a blank page.
    ///
    /// Re-analysing afterwards is not optional. Statistics measured over an
    /// empty table say a hundred rows with a hundred distinct values, and the
    /// planner would go on believing that over 100,000 real ones — every plan
    /// the reader saw would be chosen from a fiction.
    #[must_use]
    pub fn load_trips(&mut self, bytes: &[u8]) -> String {
        match self.seed_trips(bytes) {
            Ok(rows) => serde_json::json!({ "ok": rows }).to_string(),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }

    /// One page of real keys under a prefix, for the storage browser.
    ///
    /// Paged rather than returned whole: `rows/trips` is 100,000 keys, and a
    /// folder viewer that renders all of them is not a viewer. `offset` and
    /// `limit` walk the range in key order — which is the order the store
    /// holds them in, so paging through is paging through the actual layout
    /// rather than through a list somebody sorted afterwards.
    #[must_use]
    pub fn keys(&self, path: &str, offset: usize, limit: usize) -> String {
        let tables = [
            fixture::authors(),
            fixture::books(),
            taxi::trips(),
            taxi::zones(),
        ];
        let mut out: Vec<KeySample> = Vec::new();
        let mut seen = 0usize;
        for (key, value) in self.bytes.entries() {
            let Some((space, id)) = header(&key) else {
                continue;
            };
            let (this, _, table) = describe(space, id, &tables);
            if this != path {
                continue;
            }
            if seen < offset {
                seen += 1;
                continue;
            }
            if out.len() >= limit {
                break;
            }
            out.push(KeySample {
                key: hex(&key),
                decoded: decode_key(space, &key, table.as_ref()),
                value_bytes: value.len(),
            });
            seen += 1;
        }
        serde_json::to_string(&out).expect("the keys serialise")
    }

    /// The keyspace, grouped by prefix, with real keys.
    ///
    /// Reads the store's own committed map, so it reflects writes the reader
    /// has made. This is the one view in the workbench that is about *layout*
    /// rather than about answers: a row and its index entry are two keys in
    /// one ordered space, and until you have seen them next to each other the
    /// sentence "index maintenance is atomic with the write" is just a claim.
    #[must_use]
    pub fn keyspace(&self) -> String {
        serde_json::to_string(&self.keyspace_groups()).expect("the keyspace serialises")
    }

    /// Throw the database away and seed a fresh one.
    ///
    /// The panel needs this because a reader who deletes half the fixture and
    /// reloads the page would otherwise get their own wreckage back — the
    /// store lives in the tab, not on a server, so nothing else restores it.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Run a query and return the rows and the plan.
    ///
    /// Errors come back as JSON `{"error": "..."}` rather than as a thrown
    /// exception, because every one of them is a thing the reader did — an
    /// unparseable number, a pattern the regex engine refuses — and a panel
    /// that prints the refusal is more use than a stack trace in a console.
    #[must_use]
    pub fn run(&self, spec: &str) -> String {
        match self.answer(spec) {
            Ok(answer) => serde_json::to_string(&answer).expect("the answer serialises"),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }
}

/// Which write, for the shared body below.
#[derive(Clone, Copy)]
enum Write {
    Insert,
    Update,
}

impl Playground {
    /// `insert` and `update` differ by one call; everything before it —
    /// finding the table, parsing a string per column against its declared
    /// type — is the same, and duplicating it is how the two drift.
    fn write(&self, table: &str, values: &str, which: Write) -> Result<(String, f64), String> {
        let table = self.table(table)?;
        let raw: Vec<String> = serde_json::from_str(values).map_err(|e| e.to_string())?;
        if raw.len() != table.columns().len() {
            return Err(format!(
                "{} takes {} values, got {}",
                table.name(),
                table.columns().len(),
                raw.len()
            ));
        }

        let mut row = Vec::with_capacity(raw.len());
        for (text, column) in raw.iter().zip(table.columns()) {
            row.push(
                literal(text, column.value_type())
                    .map_err(|e| format!("{}: {e}", column.name()))?,
            );
        }
        let row = Row::new(row);

        // The clock covers the transaction: begin, the write *and its index
        // maintenance*, and the commit. That is the number worth showing for a
        // record layer — "the index is updated inside the write" is the claim,
        // and the cost of keeping it is what a reader cannot otherwise see.
        //
        // Parsing the strings into typed values happens above and is excluded:
        // it is the binding's cost, not the store's.
        let started = now_ms();
        block_on(async {
            let txn = self.store.begin().await?;
            match which {
                Write::Insert => txn.insert(&self.context, &table, &row).await?,
                Write::Update => txn.update(&self.context, &table, &row).await?,
            }
            txn.commit().await?;
            Ok::<_, slate_kernel::KernelError>(())
        })
        .map_err(|e| e.to_string())?;
        let elapsed = now_ms() - started;

        Ok((
            match which {
                Write::Insert => "inserted".to_owned(),
                Write::Update => "updated".to_owned(),
            },
            elapsed,
        ))
    }

    fn remove(&self, table: &str, key: &str) -> Result<(String, f64), String> {
        let table = self.table(table)?;
        let raw: Vec<String> = serde_json::from_str(key).map_err(|e| e.to_string())?;

        let mut values = Vec::with_capacity(raw.len());
        for (text, ordinal) in raw.iter().zip(table.primary_key()) {
            let column = table
                .column(*ordinal)
                .ok_or_else(|| "the primary key names a column that is not there".to_owned())?;
            values.push(
                literal(text, column.value_type())
                    .map_err(|e| format!("{}: {e}", column.name()))?,
            );
        }

        let started = now_ms();
        let gone = block_on(async {
            let txn = self.store.begin().await?;
            let gone = txn.delete(&self.context, &table, &values).await?;
            txn.commit().await?;
            Ok::<_, slate_kernel::KernelError>(gone)
        })
        .map_err(|e| e.to_string())?;
        let elapsed = now_ms() - started;

        Ok((
            if gone {
                "deleted".to_owned()
            } else {
                "no row with that key".to_owned()
            },
            elapsed,
        ))
    }

    /// The body of [`Playground::join`].
    fn joined(&self, spec: &str) -> Result<JoinAnswer, String> {
        let spec: JoinSpec = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        self.joined_spec(spec)
    }

    /// The same, from a spec that is already a value. See [`Self::answer_spec`].
    fn joined_spec(&self, spec: JoinSpec) -> Result<JoinAnswer, String> {
        let authors = self.table(&spec.left)?;
        let books = self.table(&spec.right)?;

        // The joined row is the left table's columns followed by the right's,
        // so a group key on the left keeps its own ordinal and an aggregate on
        // the right has to be shifted past every left column. That shift is
        // the arithmetic `ColumnRef` exists to remove in the clients.
        let mut join = Join::on([JoinKey::new(
            Ordinal(spec.left_key as usize),
            Ordinal(spec.right_key as usize),
        )]);
        join.left = conditions(&spec.left_where, &authors)?;
        join.right = conditions(&spec.right_where, &books)?;
        // Over the joined row, so they are appended after *both* tables and may
        // read either. The left side keeps its ordinals in the joined space and
        // the right is shifted past every left column -- the arithmetic
        // `ColumnRef` removes in the clients, done by hand here because this
        // spec is JSON from a browser rather than a typed builder.
        join.compute = joined_computes(&spec.compute, &authors, &books)?;
        // `LIMIT` and `OFFSET` belong to whichever thing the read returns: the
        // joined rows here, the *groups* in the grouped arm below.
        //
        // **A withdrawn hypothesis.** This used to set them unconditionally,
        // and that looked like the mistake the single-table path is careful to
        // avoid — a window over the rows going *into* a grouping answers a
        // different question, "the boroughs of the first two matched trips"
        // rather than "the first two boroughs". So it was written up as a bug
        // and then mutation-tested by putting it back. Nothing failed:
        // `narrowed_join` in the kernel clears a grouped join's `limit` and
        // `offset` and explains why at length, because honouring them made
        // the answer depend on which join algorithm won — a real defect once,
        // found by two hash build sides disagreeing.
        //
        // So this guard changes no answer, and it stays for legibility: the
        // spec's window is set where it applies rather than everywhere and
        // discarded downstream. The claim that it was a live bug is
        // withdrawn.
        if spec.group_by.is_none() {
            if let Some(limit) = spec.limit {
                join.limit = Some(usize::try_from(limit).unwrap_or(usize::MAX));
            }
            join.offset = usize::try_from(spec.offset).unwrap_or(usize::MAX);
        }

        let grouping = match spec.group_by {
            None => None,
            Some(key) => {
                let mut aggregates = Vec::with_capacity(spec.aggregates.len());
                for wanted in &spec.aggregates {
                    // A right-side ordinal has to be shifted into the joined
                    // row's space, which begins after every left column; a
                    // left-side one is already in it. This is exactly the
                    // arithmetic `ColumnRef` exists to remove in the clients,
                    // and doing it by hand here is the reason the tests below
                    // check a `max(year)` against a value computed
                    // independently.
                    //
                    // It used to shift unconditionally, which is why an
                    // aggregate could only read the right table.
                    let shifted = joined_ordinal(wanted.input, wanted.column, &authors, &books)?;
                    aggregates.push(match wanted.kind.as_str() {
                        "count" => Aggregate::Count,
                        "count_column" => Aggregate::CountColumn(shifted),
                        "count_distinct" => Aggregate::CountDistinct(shifted),
                        "min" => Aggregate::Min(shifted),
                        "max" => Aggregate::Max(shifted),
                        "sum" => Aggregate::Sum(shifted),
                        "avg" => Aggregate::Avg(shifted),
                        other => return Err(format!("no such aggregate: {other}")),
                    });
                }
                if aggregates.is_empty() {
                    aggregates.push(Aggregate::Count);
                }
                let mut grouping = Grouping::by([Ordinal(key as usize)], &aggregates);
                if let Some(limit) = spec.limit {
                    grouping.limit = Some(usize::try_from(limit).unwrap_or(usize::MAX));
                }
                grouping.offset = usize::try_from(spec.offset).unwrap_or(usize::MAX);
                // Over the *group*, not over the joined row: a group is its
                // key followed by its aggregates, and `SortSpec::column`
                // indexes that. Resolved in the parser, which is the only
                // place that knows which aggregate the reader named.
                grouping.sort = spec
                    .sort
                    .iter()
                    .map(|key| {
                        let ordinal = Ordinal(key.column as usize);
                        if key.descending {
                            SortKey::desc(ordinal)
                        } else {
                            SortKey::asc(ordinal)
                        }
                    })
                    .collect();
                Some(grouping)
            }
        };

        let started = now_ms();
        let (explanation, rows, groups) = block_on(async {
            let snapshot = self.store.snapshot().await?;
            match &grouping {
                Some(grouping) => {
                    let explanation = snapshot.explain_grouped_join(
                        &self.context,
                        &authors,
                        &books,
                        &join,
                        grouping,
                    )?;
                    let groups = snapshot
                        .group_by_join(&self.context, &authors, &books, &join, grouping)
                        .await?;
                    Ok::<_, slate_kernel::KernelError>((explanation, Vec::new(), groups))
                }
                None => {
                    let explanation =
                        snapshot.explain_join(&self.context, &authors, &books, &join)?;
                    let mut cursor = snapshot
                        .join(&self.context, &authors, &books, &join)
                        .await?;
                    let mut out = Vec::new();
                    while let Some(row) = cursor.next().await? {
                        // Collected whole and rendered after the clock stops,
                        // for the reason the single-table path does it.
                        out.push(row);
                    }
                    Ok((explanation, out, Vec::new()))
                }
            }
        })
        .map_err(|e| e.to_string())?;
        let elapsed = now_ms() - started;

        let rows: Vec<Vec<String>> =
            rows.iter()
                .map(|row| {
                    let mut line = Vec::new();
                    // Each side padded to *its own* width. This used the left
                    // table's width for both, which is wrong whenever the tables
                    // differ — every cell after the padding would shift, silently,
                    // with the header still naming what was supposed to be there.
                    //
                    // Labelled as latent rather than fixed, because nothing
                    // reaches it: `JoinSpec` carries no join type, so every join
                    // this binding runs is inner and no side is ever missing. The
                    // arm is here so the grid stays aligned the day an outer join
                    // becomes expressible, and it was wrong in a way that would
                    // have been found by a reader rather than by a test. Kept
                    // correct now, while the reason is in front of someone.
                    for (side, table) in [(&row.left, &authors), (&row.right, &books)] {
                        match side {
                            Some(values) => line.extend(render(values)),
                            // An outer join's missing side. Inner joins never
                            // produce one, but rendering it as text rather than
                            // skipping keeps the columns aligned with the header.
                            None => line
                                .extend(std::iter::repeat_n("—".to_owned(), table.columns().len())),
                        }
                    }
                    line
                })
                .collect();

        let rendered_groups: Vec<Vec<String>> = groups
            .iter()
            .map(|group| {
                let mut line: Vec<String> = group.key.iter().map(text).collect();
                line.extend(group.values.iter().map(text));
                line
            })
            .collect();

        // `JoinExplanation` names its sides `left` and `right` rather than
        // holding a list, and the algorithm belongs to the join as a whole
        // rather than to a side — so it is reported once, on the right, which
        // is the side the algorithm describes reading.
        let algorithm = match explanation.algorithm {
            JoinAlgorithm::Hash { .. } => "hash".to_owned(),
            JoinAlgorithm::NestedLoop => "nested loop".to_owned(),
        };
        let inputs = vec![
            InputPlan {
                table: explanation.left.table.clone(),
                access: explanation.left.access.to_string(),
                index_only: explanation.left.is_index_only(),
                decodes: explanation
                    .left
                    .decodes
                    .iter()
                    .map(|o| o.0 as u32)
                    .collect(),
                algorithm: String::new(),
            },
            InputPlan {
                table: explanation.right.table.clone(),
                access: explanation.right.access.to_string(),
                index_only: explanation.right.is_index_only(),
                decodes: explanation
                    .right
                    .decodes
                    .iter()
                    .map(|o| o.0 as u32)
                    .collect(),
                algorithm,
            },
        ];

        Ok(JoinAnswer {
            kernel_ms: elapsed,
            returned: if rendered_groups.is_empty() {
                rows.len()
            } else {
                rendered_groups.len()
            },
            rows,
            groups: rendered_groups,
            inputs,
            display: explanation.to_string(),
        })
    }

    /// The body of [`Playground::sql`], with real types.
    ///
    /// Statements stop at the first refusal. A buffer is usually a sequence —
    /// insert a row, then select it back — so running the rest after one has
    /// failed reports errors about a state the reader did not ask for.
    fn run_sql(&self, buffer: &str) -> Vec<SqlResult> {
        // Every table the store has. A parser that knows fewer tables than the
        // database holds refuses valid SQL with "no such table", which is the
        // most confusing error a query editor can give: the schema tree on the
        // left is listing the table it just said does not exist.
        let tables = [
            fixture::authors(),
            fixture::books(),
            taxi::trips(),
            taxi::zones(),
        ];
        let schema = sql::Schema(&tables);
        let mut out = Vec::new();
        for (offset, statement) in sql::split(buffer) {
            let result = match sql::parse(&statement, &schema) {
                // The parser reports an offset within its own statement; the
                // editor holds the whole buffer, so it is shifted here. Doing
                // it in the parser would make it wrong for every other caller.
                Err(e) => SqlResult::failed(&statement, offset + e.at, &e.message),
                Ok(parsed) => {
                    let mut result = self.statement(&statement, parsed.statement);
                    // Carried through even when the statement failed to run:
                    // an ambiguous name is a plausible reason for the failure,
                    // and dropping the warning at the point it is most useful
                    // would be the worst moment to be quiet.
                    result.warnings = parsed.warnings;
                    result
                }
            };
            let refused = result.error.is_some();
            out.push(result);
            if refused {
                break;
            }
        }
        if out.is_empty() {
            out.push(SqlResult::failed(buffer, 0, "there is nothing to run"));
        }
        out
    }

    /// Run one parsed statement through the binding's existing paths.
    ///
    /// Every arm here calls something that was already there and already
    /// tested. That is deliberate: SQL is a way of *writing* a spec, not a
    /// second way of reaching the kernel, so there is no query path that only
    /// the editor can reach and no plan that only the editor can produce.
    fn statement(&self, text: &str, parsed: sql::Statement) -> SqlResult {
        match parsed {
            sql::Statement::Select(spec) => {
                let spec_json = serde_json::to_value(&spec).unwrap_or(serde_json::Value::Null);
                let grouped = !spec.group_by.is_empty();
                let columns = self
                    .table(&spec.table)
                    .map(|t| {
                        if grouped {
                            // The keys, then one per aggregate. A grouped
                            // answer has nothing to do with the table's own
                            // column list, and printing that header over it
                            // would mislabel every cell.
                            let mut headers: Vec<String> = spec
                                .group_by
                                .iter()
                                .map(|o| column_header(*o, &t, &spec))
                                .collect();
                            headers.extend(labels(&spec.aggregates, &t));
                            headers
                        } else {
                            // The table's own columns, then any computed ones,
                            // in the order the row carries them. A projection
                            // does not narrow this — an unread column comes
                            // back null and the grid shows it as such — so the
                            // header has to cover the whole row either way.
                            let mut headers: Vec<String> =
                                t.columns().iter().map(|c| c.name().to_owned()).collect();
                            headers.extend((0..spec.compute.len()).map(|i| {
                                column_header(
                                    u32::try_from(t.columns().len() + i).unwrap_or(0),
                                    &t,
                                    &spec,
                                )
                            }));
                            headers
                        }
                    })
                    .unwrap_or_default();
                match self.answer_spec(&spec) {
                    Err(message) => SqlResult::failed(text, 0, &message),
                    Ok(answer) => {
                        let mut out =
                            SqlResult::blank(text, if grouped { "group" } else { "select" });
                        out.columns = columns;
                        out.returned = answer.returned;
                        out.rows = answer.rows;
                        out.kernel_ms = answer.kernel_ms;
                        out.plan = Some(answer.plan);
                        out.spec = spec_json;
                        out
                    }
                }
            }
            sql::Statement::Join(spec) => {
                let spec_json = serde_json::to_value(&spec).unwrap_or(serde_json::Value::Null);
                let grouped = spec.group_by;
                let left_table = self
                    .table(&spec.left)
                    .unwrap_or_else(|_| fixture::authors());
                let right_table = self.table(&spec.right).unwrap_or_else(|_| fixture::books());
                let labels = labels(&spec.aggregates, &right_table);
                // A computed group key sits past both tables, so its header
                // comes from the spec rather than from a column that does not
                // exist. Named as it was written — `hour(pickup_time)` — which
                // is what the reader typed and what they will look for.
                let computed_headers: Vec<(u32, String)> = spec
                    .compute
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let width = left_table.columns().len() + right_table.columns().len();
                        let name = left_table
                            .column(Ordinal(c.column as usize))
                            .map_or_else(|| c.column.to_string(), |d| d.name().to_owned());
                        (
                            u32::try_from(width + i).unwrap_or(0),
                            format!("{}({name}{})", c.function, zone_suffix(c)),
                        )
                    })
                    .collect();
                match self.joined_spec(spec) {
                    Err(message) => SqlResult::failed(text, 0, &message),
                    Ok(answer) => {
                        let mut out = SqlResult::blank(
                            text,
                            if grouped.is_some() { "group" } else { "join" },
                        );
                        out.columns = match grouped {
                            Some(key) => {
                                let mut headers = vec![
                                    computed_headers
                                        .iter()
                                        .find(|(at, _)| *at == key)
                                        .map(|(_, name)| name.clone())
                                        .or_else(|| {
                                            left_table
                                                .column(Ordinal(key as usize))
                                                .map(|c| c.name().to_owned())
                                        })
                                        .unwrap_or_else(|| key.to_string()),
                                ];
                                // `count` is always there, added by the
                                // binding when the reader named no aggregate.
                                if labels.is_empty() {
                                    headers.push("count(*)".to_owned());
                                } else {
                                    headers.extend(labels);
                                }
                                headers
                            }
                            None => left_table
                                .columns()
                                .iter()
                                .map(|c| format!("{}.{}", left_table.name(), c.name()))
                                .chain(
                                    right_table
                                        .columns()
                                        .iter()
                                        .map(|c| format!("{}.{}", right_table.name(), c.name())),
                                )
                                .collect(),
                        };
                        out.returned = answer.returned;
                        out.rows = if answer.groups.is_empty() {
                            answer.rows
                        } else {
                            answer.groups
                        };
                        out.inputs = answer.inputs;
                        out.kernel_ms = answer.kernel_ms;
                        out.message = answer.display;
                        out.spec = spec_json;
                        out
                    }
                }
            }
            sql::Statement::Insert { table, values } => {
                let json = serde_json::to_string(&values).unwrap_or_default();
                self.wrote(text, self.write(&table, &json, Write::Insert))
            }
            sql::Statement::Delete { table, key } => {
                let json = serde_json::to_string(&[key]).unwrap_or_default();
                self.wrote(text, self.remove(&table, &json))
            }
            sql::Statement::Update { table, key, set } => {
                self.wrote(text, self.patch(&table, &key, &set))
            }
        }
    }

    fn wrote(&self, text: &str, outcome: Result<(String, f64), String>) -> SqlResult {
        match outcome {
            Err(message) => SqlResult::failed(text, 0, &message),
            Ok((message, elapsed)) => {
                let mut out = SqlResult::blank(text, "write");
                out.message = message;
                out.kernel_ms = elapsed;
                out
            }
        }
    }

    /// `UPDATE ... SET` as a read, an edit, and the binding's whole-row write.
    ///
    /// The kernel's update replaces a row; the SQL says which columns change.
    /// Reading the row first is the only way to bridge that, and it is done
    /// with the *same* query path everything else uses rather than a private
    /// point-get, so a row hidden by a row policy stays hidden here too. That
    /// matters: a read-modify-write that could see more than the reader can is
    /// how a policy gets bypassed by an editor.
    fn patch(
        &self,
        table: &str,
        key: &str,
        set: &[(u32, String)],
    ) -> Result<(String, f64), String> {
        let def = self.table(table)?;
        let pk = *def
            .primary_key()
            .first()
            .ok_or_else(|| format!("`{table}` has no primary key"))?;
        let spec = QuerySpec {
            table: table.to_owned(),
            filters: vec![FilterSpec {
                column: u32::try_from(pk.0).unwrap_or(0),
                op: "eq".to_owned(),
                value: key.to_owned(),
            }],
            ..QuerySpec::default()
        };
        let found = self.answer_spec(&spec)?;
        let read_ms = found.kernel_ms;
        let mut row = found
            .rows
            .into_iter()
            .next()
            .ok_or_else(|| format!("no row in `{table}` with that key"))?;
        for (ordinal, value) in set {
            let slot = row
                .get_mut(*ordinal as usize)
                .ok_or_else(|| format!("`{table}` has no column {ordinal}"))?;
            *slot = value.clone();
        }
        let json = serde_json::to_string(&row).map_err(|e| e.to_string())?;
        // Both halves: an `UPDATE ... SET` here is a read *and* a write, and
        // reporting only the write would understate what the statement cost.
        let (message, write_ms) = self.write(table, &json, Write::Update)?;
        Ok((message, read_ms + write_ms))
    }

    /// One table, grouped.
    ///
    /// Note what this does *not* manage to do, because the first version of
    /// this comment claimed it did: `explain_grouped` takes a `Grouping` and
    /// `group_by` takes the keys and aggregates separately, rebuilding an
    /// equivalent one inside. So unlike the row path — where one `Query` value
    /// goes to both `explain` and `execute` — these are two constructions from
    /// the same two inputs, and only *equivalent by construction*.
    ///
    /// A mutation that narrowed the explained grouping to its first key, and
    /// left the executed one alone, changed no test until one was written for
    /// it. The test is `grouping_by_two_columns_keys_on_the_pair`, which now
    /// asserts the plan as well as the groups.
    fn grouped_spec(
        &self,
        spec: &QuerySpec,
        table: &TableDef,
        query: &Query,
    ) -> Result<Answer, String> {
        let keys: Vec<Ordinal> = spec.group_by.iter().map(|c| Ordinal(*c as usize)).collect();
        // A key may be one of the table's columns or one of the query's
        // computed ones, which sit immediately after them. Checking only
        // `table.column` refused `GROUP BY hour(pickup_time)` with "no such
        // column 11", which is true and unhelpful.
        let width = table.columns().len() + spec.compute.len();
        for key in &keys {
            if key.0 >= width {
                return Err(format!("{} has no column {}", table.name(), key.0));
            }
        }
        let aggregates = aggregates(&spec.aggregates, table)?;
        let mut grouping = Grouping::by(keys.iter().copied(), &aggregates);

        // HAVING before ORDER BY, because it decides which groups exist and
        // the sort only decides what order they come back in. The kernel
        // applies them in that order regardless; setting them in the same
        // order here is so that reading this says what happens.
        if !spec.having.is_empty() {
            grouping = grouping.having(having(&spec.having, &keys, &aggregates, table)?);
        }

        // ORDER BY, LIMIT and OFFSET belong to the *groups* when there is a
        // grouping, not to the rows feeding it. `Grouping` is where the kernel
        // keeps them, and the ordinals here are in group space — the keys,
        // then the aggregates — which is what the parser resolved them
        // against.
        if !spec.sort.is_empty() {
            grouping = grouping.sort_by(spec.sort.iter().map(|s| {
                let column = Ordinal(s.column as usize);
                if s.descending {
                    SortKey::desc(column)
                } else {
                    SortKey::asc(column)
                }
            }));
        }
        if let Some(limit) = spec.limit {
            grouping = grouping.limit(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        if spec.offset > 0 {
            grouping = grouping.offset(usize::try_from(spec.offset).unwrap_or(usize::MAX));
        }

        // One `Grouping` value to both, which is the invariant the row path
        // keeps with its one `Query`. It is reachable here only because
        // `grouped` takes a whole `Grouping`; `group_by` takes the keys and
        // rebuilds one, and going through that is what let an earlier version
        // explain a different grouping than it ran.
        let started = now_ms();
        let (explanation, groups) = block_on(async {
            let snapshot = self.store.snapshot().await?;
            let explanation = snapshot.explain_grouped(&self.context, table, query, &grouping)?;
            let groups = snapshot
                .grouped(&self.context, table, query, &grouping)
                .await?;
            Ok::<_, slate_kernel::KernelError>((explanation, groups))
        })
        .map_err(|e| e.to_string())?;
        let elapsed = now_ms() - started;

        let rows: Vec<Vec<String>> = groups
            .iter()
            .map(|group| {
                let mut line: Vec<String> = group.key.iter().map(text).collect();
                line.extend(group.values.iter().map(text));
                line
            })
            .collect();

        Ok(Answer {
            returned: rows.len(),
            rows,
            kernel_ms: elapsed,
            plan: PlanInfo {
                table: table.name().to_owned(),
                access: explanation.access.to_string(),
                index_only: explanation.is_index_only(),
                sorts: explanation.sorts,
                descending: matches!(explanation.order, ScanOrder::Descending),
                estimated_rows: explanation.estimated_rows,
                estimated_cost: explanation.estimated_cost,
                residual: explanation.residual.clone(),
                decodes: explanation.decodes.iter().map(|o| o.0 as u32).collect(),
                display: explanation.to_string(),
            },
        })
    }

    /// The body of [`Playground::load_trips`].
    fn seed_trips(&mut self, bytes: &[u8]) -> Result<usize, String> {
        let rows = taxi::decode(bytes)?;
        let table = taxi::trips();
        let root = SecurityContext::superuser();
        let count = rows.len();

        block_on(async {
            let txn = self.store.begin().await?;
            // One transaction for 100,000 rows. `insert_many` keeps index
            // maintenance in the same batch, which is the whole point of the
            // bulk path: a hundred thousand separate transactions would each
            // pay a begin and a commit.
            txn.insert_many(&root, &table, &rows).await?;
            txn.commit().await?;
            Ok::<_, slate_kernel::KernelError>(())
        })
        .map_err(|e| e.to_string())?;

        let stats = block_on(analyze(&self.store, &root));
        self.store.set_statistics(stats);
        Ok(count)
    }

    /// The body of [`Playground::keyspace`].
    fn keyspace_groups(&self) -> Vec<KeyGroup> {
        // `entries()` clones the keys and refcounts the values, so this is a
        // few megabytes for 200,000 entries rather than a second copy of the
        // database. It is still the most expensive call in the binding, which
        // is why the viewer asks for it on demand rather than on every query.
        let entries = self.bytes.entries();
        let tables = [
            fixture::authors(),
            fixture::books(),
            taxi::trips(),
            taxi::zones(),
        ];

        let mut groups: Vec<KeyGroup> = Vec::new();
        for (key, value) in &entries {
            let Some((space, id)) = header(key) else {
                continue;
            };
            let (path, label, table_for_key) = describe(space, id, &tables);
            let index = match groups.iter().position(|g| g.path == path) {
                Some(i) => i,
                None => {
                    groups.push(KeyGroup {
                        space: if space == 0x01 { "rows" } else { "index" }.to_owned(),
                        path,
                        label,
                        id,
                        keys: 0,
                        key_bytes: 0,
                        value_bytes: 0,
                        samples: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            let Some(group) = groups.get_mut(index) else {
                continue;
            };
            group.keys += 1;
            group.key_bytes += key.len();
            group.value_bytes += value.len();
            // Three samples per group: enough to see the prefix repeat and the
            // suffix advance, few enough that the panel is not a hex dump.
            if group.samples.len() < 3 {
                group.samples.push(KeySample {
                    key: hex(key),
                    decoded: decode_key(space, key, table_for_key.as_ref()),
                    value_bytes: value.len(),
                });
            }
        }
        groups.sort_by_key(|group| std::cmp::Reverse(group.keys));
        groups
    }

    fn table(&self, name: &str) -> Result<TableDef, String> {
        match name {
            "authors" => Ok(fixture::authors()),
            "books" => Ok(fixture::books()),
            "trips" => Ok(taxi::trips()),
            "zones" => Ok(taxi::zones()),
            other => Err(format!("no such table: {other}")),
        }
    }

    /// The body of [`Playground::run`], with a real error type.
    ///
    /// Split out so the native test suite can assert on the failures rather
    /// than on their JSON rendering.
    fn answer(&self, spec: &str) -> Result<Answer, String> {
        let spec: QuerySpec = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        self.answer_spec(&spec)
    }

    /// The same, from a spec that is already a value.
    ///
    /// The SQL editor lands here, which is the point of taking this split: a
    /// statement is parsed into the very spec the JSON path deserialises, so
    /// there is one query path and one `EXPLAIN`, not one per front end.
    fn answer_spec(&self, spec: &QuerySpec) -> Result<Answer, String> {
        let table = self.table(&spec.table)?;

        let query = build(spec, &table)?;

        if !spec.group_by.is_empty() {
            // Built without them: `build` puts sort, limit and offset on the
            // query, and a LIMIT applied to the rows going into a grouping
            // silently answers a different question — the first 10 rows'
            // groups, not the first 10 groups.
            let ungrouped = build(
                &QuerySpec {
                    sort: Vec::new(),
                    limit: None,
                    offset: 0,
                    ..spec.clone()
                },
                &table,
            )?;
            return self.grouped_spec(spec, &table, &ungrouped);
        }

        // One snapshot answers both, and the *same* `Query` value is handed to
        // `explain` and to `execute`. Explaining a query rebuilt to look like
        // the one that ran is how an `EXPLAIN` comes to describe a plan
        // nothing executes; the kernel makes avoiding that free, so there is
        // no excuse for the other shape.
        //
        // The clock starts here and stops at the end of `block_on`, so it spans
        // the snapshot, the plan and the execution — and excludes turning the
        // rows into JSON, which for a large result is most of the wall clock
        // and none of the database.
        let started = now_ms();
        let (explanation, raw) = block_on(async {
            let snapshot = self.store.snapshot().await?;
            let explanation = snapshot.explain(&self.context, &table, &query)?;
            let mut cursor = snapshot.execute(&self.context, &table, &query).await?;
            let mut out = Vec::new();
            while let Some(row) = cursor.next().await? {
                // The `Row` is pushed, not its rendering. Formatting a hundred
                // thousand rows into strings is a hundred thousand `format!`s
                // and none of them are the database — an earlier version had
                // `render` inside this loop and charged the kernel for them.
                out.push(row);
            }
            Ok::<_, slate_kernel::KernelError>((explanation, out))
        })
        .map_err(|e| e.to_string())?;
        let elapsed = now_ms() - started;

        let rows: Vec<Vec<String>> = raw.iter().map(render).collect();

        Ok(Answer {
            returned: rows.len(),
            rows,
            kernel_ms: elapsed,
            plan: PlanInfo {
                table: table.name().to_owned(),
                access: explanation.access.to_string(),
                index_only: explanation.is_index_only(),
                sorts: explanation.sorts,
                descending: matches!(explanation.order, ScanOrder::Descending),
                estimated_rows: explanation.estimated_rows,
                estimated_cost: explanation.estimated_cost,
                residual: explanation.residual.clone(),
                decodes: explanation.decodes.iter().map(|o| o.0 as u32).collect(),
                display: explanation.to_string(),
            },
        })
    }
}

/// Turn the UI's description into a kernel `Query`.
fn build(spec: &QuerySpec, table: &TableDef) -> Result<Query, String> {
    let mut query = Query::all();

    // First, because everything below may name a computed ordinal and the
    // kernel only knows what those mean once the query carries the
    // expressions that produce them.
    if !spec.compute.is_empty() {
        query = query.computing(computes(&spec.compute, table)?);
    }

    // `filter` and `filters` are both accepted and both ANDed in. The single
    // form is not deprecated shorthand — it is what a one-condition panel
    // sends, and refusing it would break the shape this binding shipped with.
    let conditions: Vec<&FilterSpec> = spec.filter.iter().chain(spec.filters.iter()).collect();
    if !conditions.is_empty() {
        let mut parts = Vec::with_capacity(conditions.len());
        for condition in conditions {
            parts.push(comparison(condition, table)?);
        }
        // `Expr::all` rather than folding with `and`: it is the kernel's own
        // constructor for a conjunction, and the planner reads conjuncts out
        // of it to look for scan bounds. A hand-folded tree of nested `And`s
        // is the same predicate and gives the planner more work to undo.
        query = query.filter(Expr::all(parts));
    }
    if !spec.sort.is_empty() {
        let keys: Vec<SortKey> = spec
            .sort
            .iter()
            .map(|s| {
                let column = Ordinal(s.column as usize);
                if s.descending {
                    SortKey::desc(column)
                } else {
                    SortKey::asc(column)
                }
            })
            .collect();
        query = query.sort_by(keys);
    }
    if !spec.columns.is_empty() {
        query = query.select(spec.columns.iter().map(|c| Ordinal(*c as usize)));
    }
    if let Some(limit) = spec.limit {
        query = query.limit(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    if spec.offset > 0 {
        query = query.offset(usize::try_from(spec.offset).unwrap_or(usize::MAX));
    }
    Ok(query)
}

/// One `column op literal`, with the literal parsed to the column's type.
///
/// Typed rather than coerced, because the kernel's value order is type-first —
/// that is what makes the key encoding sortable — so handing a `Str` to a `U64`
/// column would not fail, it would compare the *types* and match nothing.
/// Getting this wrong is silent, so it is done once, here.
fn comparison(filter: &FilterSpec, table: &TableDef) -> Result<Expr, String> {
    let column = Ordinal(filter.column as usize);
    let def = table
        .column(column)
        .ok_or_else(|| format!("{} has no column {}", table.name(), filter.column))?;

    // Patterns are strings whatever the column is.
    match filter.op.as_str() {
        "like" => return Ok(Expr::like(column, filter.value.clone())),
        "ilike" => return Ok(Expr::ilike(column, filter.value.clone())),
        "matches" => {
            let expr = Expr::matches(column, filter.value.clone());
            if let Some(bad) = expr.regex_error() {
                return Err(format!("that regular expression is not valid: {bad}"));
            }
            return Ok(expr);
        }
        _ => {}
    }

    let value = literal(&filter.value, def.value_type())?;
    // `Expr::compare` rather than the `eq`/`lt` shorthands: those names exist
    // on `Expr` as *combinators over expressions*, not comparison
    // constructors, and reaching for them here compiled into something else
    // entirely.
    let op = match filter.op.as_str() {
        "eq" => CmpOp::Eq,
        "ne" => CmpOp::Ne,
        "lt" => CmpOp::Lt,
        "le" => CmpOp::Le,
        "gt" => CmpOp::Gt,
        "ge" => CmpOp::Ge,
        other => return Err(format!("no such operator: {other}")),
    };
    Ok(Expr::compare(column, op, value))
}

fn literal(text: &str, kind: slate_tuple::ValueType) -> Result<Value, String> {
    use slate_tuple::ValueType as T;
    match kind {
        T::U64 => text
            .trim()
            .parse::<u64>()
            .map(Value::U64)
            .map_err(|_| format!("{text:?} is not a non-negative whole number")),
        T::I64 => text
            .trim()
            .parse::<i64>()
            .map(Value::I64)
            .map_err(|_| format!("{text:?} is not a whole number")),
        T::F64 => text
            .trim()
            .parse::<f64>()
            .map(Value::F64)
            .map_err(|_| format!("{text:?} is not a number")),
        T::Bool => text
            .trim()
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|_| format!("{text:?} is not true or false")),
        _ => Ok(Value::Str(text.to_owned())),
    }
}

/// Measure every table, for the planner.
///
/// One function rather than a list at each call site: the constructor and the
/// trip loader both need this, and a loader that re-analysed `trips` while
/// leaving `books` on the old numbers is the kind of drift that shows up as an
/// unexplainable plan two screens away.
async fn analyze(store: &RecordStore<MemoryStore>, root: &SecurityContext) -> Statistics {
    let txn = store.begin().await.expect("begin");
    let mut stats = Statistics::new();
    for (id, table) in [
        (fixture::AUTHORS, fixture::authors()),
        (fixture::BOOKS, fixture::books()),
        (taxi::TRIPS, taxi::trips()),
        (taxi::ZONES, taxi::zones()),
    ] {
        stats = stats.with(id, txn.analyze(root, &table).await.expect("analyze"));
    }
    stats
}

/// The `<space byte><id : u32 BE>` header every key starts with.
///
/// Documented in `slate_kernel::keys`, and read here rather than imported
/// because the constants are private to that module. If the layout changed,
/// this viewer would show nonsense — which is what
/// `the_viewer_reads_the_layout_the_kernel_writes` is for.
fn header(key: &[u8]) -> Option<(u8, u32)> {
    let space = *key.first()?;
    let id = u32::from_be_bytes([*key.get(1)?, *key.get(2)?, *key.get(3)?, *key.get(4)?]);
    Some((space, id))
}

/// Which table or index a key's header names, and what to call it.
fn describe(space: u8, id: u32, tables: &[TableDef]) -> (String, String, Option<TableDef>) {
    if space == 0x01 {
        for table in tables {
            if table.id().0 == id {
                return (
                    format!("rows/{}", table.name()),
                    table.name().to_owned(),
                    Some(table.clone()),
                );
            }
        }
        return (format!("rows/table {id}"), format!("table {id}"), None);
    }
    for table in tables {
        for index in table.indexes() {
            if index.id().0 == id {
                return (
                    format!("index/{}.{}", table.name(), index.name()),
                    format!("{}.{}", table.name(), index.name()),
                    Some(table.clone()),
                );
            }
        }
    }
    (format!("index/{id}"), format!("index {id}"), None)
}

/// A key as hex, split into the parts the layout defines.
///
/// `01 00000003 | 02 000000000000007b` — the space byte, the big-endian id,
/// then the encoded tuple. The separator is where the header ends, which is
/// the thing worth seeing: everything after it is ordered tuple bytes, and
/// that is why a prefix of the key is a prefix of the tuple.
fn hex(key: &[u8]) -> String {
    let byte = |b: &u8| format!("{b:02x}");
    let head: String = key.iter().take(1).map(&byte).collect();
    let id: String = key.iter().skip(1).take(4).map(&byte).collect();
    let rest: Vec<String> = key.iter().skip(5).map(&byte).collect();
    if rest.is_empty() {
        return format!("{head} {id}");
    }
    format!("{head} {id} | {}", rest.join(""))
}

/// What a key means, through the kernel's own decoders.
///
/// Not a second implementation of the layout: `decode_row_key` and
/// `decode_index_entry` are the functions the read path uses. A key this
/// cannot decode is reported as such rather than guessed at.
fn decode_key(space: u8, key: &[u8], table: Option<&TableDef>) -> String {
    let Some(table) = table else {
        return "unknown table".to_owned();
    };
    if space == 0x01 {
        return match slate_kernel::keys::decode_row_key(table, key) {
            Ok(values) => {
                let named: Vec<String> = table
                    .primary_key()
                    .iter()
                    .zip(&values)
                    .map(|(ordinal, value)| {
                        let name = table
                            .column(*ordinal)
                            .map_or_else(|| ordinal.0.to_string(), |c| c.name().to_owned());
                        format!("{name}={}", text(value))
                    })
                    .collect();
                format!("{} row  {}", table.name(), named.join(", "))
            }
            Err(e) => format!("undecodable row key: {e}"),
        };
    }
    for index in table.indexes() {
        if let Ok((indexed, primary_key)) =
            slate_kernel::keys::decode_index_entry(table, index, key, &[])
        {
            let indexed: Vec<String> = indexed.iter().map(text).collect();
            let key_values: Vec<String> = primary_key.iter().map(text).collect();
            return format!(
                "{} = {}  ->  row {}",
                index.name(),
                indexed.join(", "),
                key_values.join(", ")
            );
        }
    }
    "index entry".to_owned()
}

/// Header text for each aggregate: `count(*)`, `avg(total)`.
///
/// Shared by the single-table and the join paths, because two copies of this
/// is how a grouped join comes to label its columns differently from a grouped
/// scan of the same data.
fn labels(specs: &[AggregateSpec], table: &TableDef) -> Vec<String> {
    if specs.is_empty() {
        return vec!["count(*)".to_owned()];
    }
    specs
        .iter()
        .map(|a| {
            let column = table
                .column(Ordinal(a.column as usize))
                .map_or_else(|| a.column.to_string(), |c| c.name().to_owned());
            match a.kind.as_str() {
                "count" => "count(*)".to_owned(),
                "count_column" => format!("count({column})"),
                "count_distinct" => format!("count(distinct {column})"),
                other => format!("{other}({column})"),
            }
        })
        .collect()
}

/// Lower the UI's aggregate list onto the kernel's, resolving ordinals
/// against the table the aggregates read.
///
/// `count(*)` when the list is empty: a grouped query with no aggregate is a
/// list of distinct keys, and returning nothing beside the key would make the
/// result look broken rather than minimal.
fn aggregates(specs: &[AggregateSpec], table: &TableDef) -> Result<Vec<Aggregate>, String> {
    if specs.is_empty() {
        return Ok(vec![Aggregate::Count]);
    }
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let column = Ordinal(spec.column as usize);
        if !matches!(spec.kind.as_str(), "count") && table.column(column).is_none() {
            return Err(format!("{} has no column {}", table.name(), spec.column));
        }
        out.push(match spec.kind.as_str() {
            "count" => Aggregate::Count,
            // `count(column)` is not `count(*)`: it skips nulls. Keeping them
            // apart here is why the parser bothers to tell them apart.
            "count_column" => Aggregate::CountColumn(column),
            "min" => Aggregate::Min(column),
            "max" => Aggregate::Max(column),
            "sum" => Aggregate::Sum(column),
            "avg" => Aggregate::Avg(column),
            "count_distinct" => Aggregate::CountDistinct(column),
            other => return Err(format!("no such aggregate: {other}")),
        });
    }
    Ok(out)
}

/// The second argument a computed column was written with, if any.
///
/// Part of the header because it is part of the identity: `hour(t)`,
/// `hour(t, '-05:00')` and `hour(t, 'America/New_York')` are three different
/// computed columns and a query may select all three, which without this are
/// three columns headed `hour(t)`. The offset is printed back in the `+HH:MM`
/// form it was written in rather than as a number of seconds, so the header
/// reads as the query does.
fn zone_suffix(spec: &ComputeSpec) -> String {
    if !spec.zone.is_empty() {
        return format!(", '{}'", spec.zone);
    }
    if spec.offset == 0 {
        return String::new();
    }
    let sign = if spec.offset < 0 { '-' } else { '+' };
    let seconds = spec.offset.abs();
    format!(
        ", '{sign}{:02}:{:02}'",
        seconds / 3_600,
        seconds % 3_600 / 60
    )
}

/// What to print above a column, whether the table owns it or the query
/// computed it.
///
/// A computed column has no name in the schema — it does not exist there — so
/// the header is rebuilt from the call that produced it. Falling back to the
/// bare ordinal, which is what this replaced, put `11` above a column of hours.
fn column_header(ordinal: u32, table: &TableDef, spec: &QuerySpec) -> String {
    if let Some(column) = table.column(Ordinal(ordinal as usize)) {
        return column.name().to_owned();
    }
    let computed = (ordinal as usize).checked_sub(table.columns().len());
    computed.and_then(|i| spec.compute.get(i)).map_or_else(
        || ordinal.to_string(),
        |c| {
            let argument = table
                .column(Ordinal(c.column as usize))
                .map_or_else(|| c.column.to_string(), |d| d.name().to_owned());
            format!("{}({argument}{})", c.function, zone_suffix(c))
        },
    )
}

/// One `ComputeSpec` as the kernel's `Scalar`.
///
/// The names are SQL's where SQL has one — `EXTRACT(HOUR FROM t)` and
/// `EXTRACT(DAY FROM t)` mean hour-of-day and day-of-*month*, so `day` here is
/// the day of the month and not the day of the epoch, which is what
/// `TimeUnit::Day` would give. Getting that backwards would be silent: both
/// return an integer and both look plausible in a column.
fn compute_scalar(spec: &ComputeSpec, table: &TableDef, base: usize) -> Result<Scalar, String> {
    use slate_tuple::ValueType as T;
    let column = Ordinal(spec.column as usize);
    let def = table
        .column(column)
        .ok_or_else(|| format!("{} has no column {}", table.name(), spec.column))?;
    let kind = def.value_type();
    // The column is named in its own table's ordinals and read in the space the
    // expression is evaluated in -- the same ordinals on one table or on a
    // join's left side, shifted past every left column on its right side.
    let mut value = Scalar::Column(Ordinal(base + column.0));

    // The zone shift, if there is one. Adding seconds to a timestamp and then
    // reading the calendar out of the result *is* what a fixed-offset
    // conversion is, so this needs no new `Scalar` variant and no new wire
    // field — see `ComputeSpec::offset`.
    //
    // `arithmetic` promotes two integers to `I64` and saturates rather than
    // wrapping, so a `U64` column shifted below the epoch becomes a negative
    // `I64` and `CalendarPart`'s floor division handles it, rather than
    // wrapping to the year 584942417355.
    if spec.offset != 0 || !spec.zone.is_empty() {
        if spec.function == "round" {
            return Err("round() takes no timezone: it is not a time function".to_owned());
        }
        if spec.offset != 0 && !spec.zone.is_empty() {
            // Not reachable from the parser, which produces one or the other.
            // Refused rather than given a precedence, because a spec built by
            // hand with both set means the caller believes something untrue
            // about which one wins, and answering either way confirms it.
            return Err(format!(
                "{}() was given both a fixed offset and the zone {:?}; they are two \
                 answers to one question",
                spec.function, spec.zone
            ));
        }
        value = if spec.zone.is_empty() {
            Scalar::Add(
                Box::new(value),
                Box::new(Scalar::Literal(slate_tuple::Value::I64(spec.offset))),
            )
        } else {
            // Checked here as well as in the parser, because a spec can arrive
            // from JavaScript without passing through the parser at all, and
            // `ZoneShift` answers null for a zone it does not know — which on
            // screen is indistinguishable from an empty column.
            if !slate_kernel::zones::has(&spec.zone) {
                return Err(format!(
                    "no such timezone: {:?}. IANA names are case-sensitive, and \
                     this has {}",
                    spec.zone,
                    slate_kernel::zones::listing()
                ));
            }
            value.in_zone(spec.zone.clone())
        };
    }

    // Checked per function rather than once, because they do not agree on what
    // they take: a timestamp is an integer of seconds — there is no date type
    // — and `round` is for the columns that are not. Refused here rather than
    // left to the kernel, which would return null per row: right for a value
    // of the wrong shape, wrong for a query that could never have worked, and
    // indistinguishable on screen from a column that is genuinely empty.
    let timestamp = |scalar: Scalar| {
        if matches!(kind, T::I64 | T::U64) {
            Ok(scalar)
        } else {
            Err(format!(
                "{}() needs a timestamp, and {} is {:?} — timestamps here are \
                 seconds since the epoch in an integer column",
                spec.function,
                def.name(),
                kind
            ))
        }
    };

    match spec.function.as_str() {
        "hour" => timestamp(value.extract(TimeUnit::Hour)),
        "minute" => timestamp(value.extract(TimeUnit::Minute)),
        "second" => timestamp(value.extract(TimeUnit::Second)),
        "year" => timestamp(value.calendar_part(CalendarPart::Year)),
        "month" => timestamp(value.calendar_part(CalendarPart::Month)),
        // `day` is the day of the *month*, as `EXTRACT(DAY FROM t)` is in SQL
        // — not `TimeUnit::Day`, which counts days since the epoch. Both
        // return an integer and both look plausible in a column, so getting
        // this backwards would be silent.
        "day" => timestamp(value.calendar_part(CalendarPart::DayOfMonth)),
        "day_of_week" => timestamp(value.calendar_part(CalendarPart::DayOfWeek)),
        // Midnight of the day, as epoch seconds — so grouping by it gives one
        // group per calendar day, ordered as the days are.
        "date" => timestamp(value.date_trunc(TimeUnit::Day)),
        // The first instant of the month or the year. `date()` is a division
        // by 86,400; these are not, because neither a month nor a year has a
        // fixed length — see `CalendarUnit`. Grouping by one gives a group per
        // calendar month, ordered as the months are, which is what `year()`
        // and `month()` cannot do on their own: those return 2024 and 2,
        // so ordering by `month()` puts every January of every year together.
        "month_start" => timestamp(value.calendar_trunc(CalendarUnit::Month)),
        "year_start" => timestamp(value.calendar_trunc(CalendarUnit::Year)),
        "round" => {
            if matches!(kind, T::F64 | T::I64 | T::U64) {
                Ok(value.round())
            } else {
                Err(format!(
                    "round() needs a number, and {} is {kind:?}",
                    def.name()
                ))
            }
        }
        other => Err(format!("no such function: {other}")),
    }
}

/// Every computed column a spec asks for, in order.
fn computes(specs: &[ComputeSpec], table: &TableDef) -> Result<Vec<Scalar>, String> {
    specs.iter().map(|c| compute_scalar(c, table, 0)).collect()
}

/// The joined-space ordinal an `input`/`column` pair names.
///
/// Zero is the left table, whose ordinals are already the joined row's; one is
/// the right, shifted past every left column. Any other input is refused rather
/// than treated as one of the two -- a join here has exactly two sides, and
/// guessing would turn a typo into a query about a different column.
fn joined_ordinal(
    input: u32,
    column: u32,
    left: &TableDef,
    right: &TableDef,
) -> Result<Ordinal, String> {
    let (table, base) = match input {
        0 => (left, 0),
        1 => (right, left.columns().len()),
        other => {
            return Err(format!(
                "a join has two sides, 0 and 1; input {other} is neither"
            ));
        }
    };
    if table.column(Ordinal(column as usize)).is_none() {
        return Err(format!("{} has no column {column}", table.name()));
    }
    Ok(Ordinal(base + column as usize))
}

/// The join's computed values, each reading whichever side it names.
fn joined_computes(
    specs: &[ComputeSpec],
    left: &TableDef,
    right: &TableDef,
) -> Result<Vec<Scalar>, String> {
    specs
        .iter()
        .map(|c| match c.input {
            0 => compute_scalar(c, left, 0),
            1 => compute_scalar(c, right, left.columns().len()),
            other => Err(format!(
                "a join has two sides, 0 and 1; input {other} is neither"
            )),
        })
        .collect()
}

/// The type a group-space ordinal holds, for `HAVING`.
///
/// This exists because of one hazard, and it is a silent one. `Value` orders
/// by *class* before it orders by magnitude, and `F64` ranks above the integer
/// variants — so `F64(19.5) > I64(20)` is **true**, by rank, with the numbers
/// playing no part. `I64` and `U64` share a rank and compare through `i128`,
/// so they interoperate; a float against an integer does not.
///
/// `HAVING avg(fare) > 20` therefore has to parse `20` as `F64(20.0)`, and
/// parsing it from the table column's type — as `WHERE` correctly does — would
/// give `I64(20)` and admit every group. Nothing would error and the answer
/// would be wrong, which is why the type comes from the aggregate rather than
/// from the column it reads.
fn group_value_type(
    ordinal: u32,
    keys: &[Ordinal],
    aggregates: &[Aggregate],
    table: &TableDef,
) -> Result<slate_tuple::ValueType, String> {
    use slate_tuple::ValueType as T;
    let index = ordinal as usize;
    let column_type = |c: Ordinal| {
        table
            .column(c)
            .map(|d| d.value_type())
            .ok_or_else(|| format!("{} has no column {}", table.name(), c.0))
    };
    if let Some(key) = keys.get(index) {
        // A group key past the table's own columns is a computed one, and
        // every function `compute_scalar` offers returns an integer.
        //
        // No query can currently tell this branch from reading the *source*
        // column's type, and a mutation replacing it with `if false` passes
        // the whole suite — because `compute_scalar` refuses a non-integer
        // source, so both readings land on `I64` or `U64`, which share a class
        // rank and compare through `i128`. It is here for the first function
        // that returns a double, where the two stop agreeing and the
        // disagreement is silent (see `having`, which explains the rank
        // hazard). Written down rather than deleted, and written down rather
        // than covered by a test that does not exist.
        if key.0 >= table.columns().len() {
            return Ok(T::I64);
        }
        return column_type(*key);
    }
    let aggregate = aggregates.get(index - keys.len()).ok_or_else(|| {
        format!(
            "HAVING names position {index}, and the group has only {} columns",
            keys.len() + aggregates.len()
        )
    })?;
    Ok(match aggregate {
        // A count is a cardinality: unsigned, whatever it counted.
        Aggregate::Count | Aggregate::CountColumn(_) | Aggregate::CountDistinct(_) => T::U64,
        Aggregate::Min(c) | Aggregate::Max(c) => column_type(*c)?,
        // `Total::sum` returns `I64` for any integer column and `F64` for a
        // real one, so a `U64` column's sum is compared as `I64` — same rank,
        // so that is a distinction without a difference here.
        Aggregate::Sum(c) => match column_type(*c)? {
            T::F64 => T::F64,
            _ => T::I64,
        },
        // Always a double, even over integers: `Total::average` divides.
        Aggregate::Avg(_) => T::F64,
    })
}

/// `HAVING`, as one `Expr` over the group.
fn having(
    specs: &[FilterSpec],
    keys: &[Ordinal],
    aggregates: &[Aggregate],
    table: &TableDef,
) -> Result<Expr, String> {
    let mut out = Expr::True;
    for spec in specs {
        let kind = group_value_type(spec.column, keys, aggregates, table)?;
        let column = Ordinal(spec.column as usize);
        let expr = match spec.op.as_str() {
            "like" => Expr::like(column, spec.value.clone()),
            "ilike" => Expr::ilike(column, spec.value.clone()),
            "matches" => {
                let expr = Expr::matches(column, spec.value.clone());
                if let Some(bad) = expr.regex_error() {
                    return Err(format!("that regular expression is not valid: {bad}"));
                }
                expr
            }
            other => {
                let op = match other {
                    "eq" => CmpOp::Eq,
                    "ne" => CmpOp::Ne,
                    "lt" => CmpOp::Lt,
                    "le" => CmpOp::Le,
                    "gt" => CmpOp::Gt,
                    "ge" => CmpOp::Ge,
                    _ => return Err(format!("no such operator: {other}")),
                };
                Expr::compare(column, op, literal(&spec.value, kind)?)
            }
        };
        out = match out {
            Expr::True => expr,
            existing => existing.and(expr),
        };
    }
    Ok(out)
}

/// One side of a join: its conditions, ANDed, as a `Query`.
fn conditions(specs: &[FilterSpec], table: &TableDef) -> Result<Query, String> {
    if specs.is_empty() {
        return Ok(Query::all());
    }
    let mut parts = Vec::with_capacity(specs.len());
    for spec in specs {
        parts.push(comparison(spec, table)?);
    }
    Ok(Query::all().filter(Expr::all(parts)))
}

/// One value as text.
fn text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::U64(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        Value::F64(n) => n.to_string(),
        Value::Str(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// Values as text, for a table in a browser.
fn render(row: &Row) -> Vec<String> {
    row.values().iter().map(text).collect()
}
