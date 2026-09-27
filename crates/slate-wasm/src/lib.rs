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

// The landing page says "No `unsafe`", and `site/check/claims.py` checks that
// every crate root says so too. The claim was true here before this line and
// unenforced: nothing stopped the next edit.
#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod fixture;
pub mod taxi;

// The SQL front end and the spec types moved to `slate-sql` when the daemon
// needed them too — see that crate's module docs, and `docs/views.md`, which
// had recorded the move as impossible. Re-exported rather than referenced
// through their new path so the binding's callers, the tests and the site all
// keep the names they already use: this is a change of where the code lives,
// not of what it is.
pub use slate_sql::sql;
pub use slate_sql::{
    AggregateSpec, ChainInputSpec, ChainOnSpec, ChainSpec, ComputeSpec, FilterSpec, JoinSpec,
    PredicateSpec, QuerySpec, SortSpec, WindowSpec,
};

// The lowering — a spec onto the kernel's own types — went with them. What is
// left here turns the same specs into strings for a grid, which is the line
// the split was made on: see `slate_sql::lower`.
use slate_sql::lower::{
    aggregate_of, aggregates, build, chained_computes, chained_ordinal, conditions,
    group_predicate, group_value_type, joined_computes, joined_ordinal, literal,
};

use futures::executor::block_on;
use serde::Serialize;
use slate_kernel::{
    Aggregate, Chain, Explanation, Grouping, Join, JoinAlgorithm, JoinKey, JoinStep, Query,
    RecordStore, ScanOrder, SortKey,
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

    /// Chain three or more tables, and optionally group the result.
    ///
    /// The n-table sibling of [`Playground::join`]. Separate because the
    /// kernel's two entry points are separate and produce different plans —
    /// see [`crate::ChainSpec`] — and because a caller has to decide which it
    /// means anyway: the number of tables is not a knob, it is the query.
    ///
    /// Exposed even though the panel has no n-table form, so that the spec the
    /// editor shows for `FROM a JOIN b JOIN c` is a spec something can be
    /// handed back. A spec pane displaying JSON no entry point accepts is a
    /// pane that documents a shape that does not exist.
    #[must_use]
    pub fn chain(&self, spec: &str) -> String {
        match serde_json::from_str::<ChainSpec>(spec)
            .map_err(|e| format!("that is not a chain spec: {e}"))
            .and_then(|spec| self.chained_spec(spec))
        {
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
                literal(text, column.value_type(), column.scale())
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
                literal(text, column.value_type(), column.scale())
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
        if spec.group_by.is_empty() {
            if let Some(limit) = spec.limit {
                join.limit = Some(usize::try_from(limit).unwrap_or(usize::MAX));
            }
            join.offset = usize::try_from(spec.offset).unwrap_or(usize::MAX);
        }

        let grouping = match spec.group_by.as_slice() {
            [] => None,
            wanted_keys => {
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
                    aggregates.push(aggregate_of(&wanted.kind, shifted)?);
                }
                // No implicit `count(*)`. It was here because a grouping
                // with nothing to compute had no meaning when this was a
                // panel with a group-by picker and no aggregate picker — and
                // it is exactly what `SELECT DISTINCT` needs, so a default
                // that quietly appended a column the caller never asked for
                // is now a wrong answer rather than a convenience. A caller
                // that wants a count asks for one.
                let keys: Vec<Ordinal> = wanted_keys.iter().map(|k| Ordinal(*k as usize)).collect();
                let mut grouping = Grouping::by(keys.clone(), &aggregates);
                if !spec.having.is_empty() || !spec.having_any_of.is_empty() {
                    // Over the group, and its stored keys resolved through the
                    // *joined* ordinal -- which is what `group_value_type` now
                    // walks, and the reason the joined path had no HAVING
                    // before: everything downstream of it took one `TableDef`.
                    //
                    // `group_predicate` rather than `having` so that an ORed
                    // HAVING reaches the kernel as a disjunction here too. The
                    // three call sites went through one function before and
                    // still do; a second spelling on one of them is how the
                    // joined path came to differ from the single-table one.
                    grouping = grouping.having(group_predicate(
                        &spec.having,
                        &spec.having_any_of,
                        &keys,
                        &aggregates,
                        &[&authors, &books],
                    )?);
                }
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
                            Some(values) => line.extend(render(values, &column_scales(&[table]))),
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

        let rendered_groups: Vec<Vec<String>> = {
            let scales = grouping.map_or_else(Vec::new, |g| {
                group_scales(&g.group, &g.aggregates, &[&authors, &books])
            });
            groups
                .iter()
                .map(|group| {
                    let mut line: Vec<String> = Vec::with_capacity(scales.len());
                    for (at, value) in group.key.iter().chain(&group.values).enumerate() {
                        line.push(text(value, scales.get(at).copied().flatten()));
                    }
                    line
                })
                .collect()
        };

        // `JoinExplanation` names its sides `left` and `right` rather than
        // holding a list, and the algorithm belongs to the join as a whole
        // rather than to a side — so it is reported once, on the right, which
        // is the side the algorithm describes reading.
        let algorithm = match explanation.algorithm {
            JoinAlgorithm::Hash { .. } => "hash".to_owned(),
            JoinAlgorithm::NestedLoop => "nested loop".to_owned(),
        };
        let inputs = vec![
            input_plan(&explanation.left, String::new()),
            input_plan(&explanation.right, algorithm),
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

    /// Run a chain of three or more tables, grouped or not.
    ///
    /// The n-table sibling of [`Playground::joined_spec`], and deliberately
    /// its own function rather than a generalisation of it: the kernel has two
    /// entry points and `group_by_chain` cannot narrow a step's projection the
    /// way `group_by_join` narrows a side's. Merging them here would mean
    /// choosing between `Join` and `Chain` at the call site anyway, one level
    /// further from the reason — which is the argument `slate-server`'s
    /// `MultiRead` already makes.
    fn chained_spec(&self, spec: ChainSpec) -> Result<JoinAnswer, String> {
        if spec.inputs.len() < 3 {
            // Not reachable from the parser, which builds a `JoinSpec` for two.
            // Refused rather than run, because a two-input `ChainSpec` would
            // answer the same question through a different kernel path, and two
            // paths for one query is the drift this front end exists to avoid.
            return Err(format!(
                "a chain is three or more tables; this has {}. Two tables are a \
                 join, and take the join path",
                spec.inputs.len()
            ));
        }
        let tables: Vec<TableDef> = spec
            .inputs
            .iter()
            .map(|input| self.table(&input.table))
            .collect::<Result<_, _>>()?;
        let refs: Vec<&TableDef> = tables.iter().collect();

        let mut first = spec
            .inputs
            .first()
            .ok_or("a chain needs a first table")
            .and_then(|input| {
                if input.on.is_some() {
                    // The first table joins to nothing, and a key naming an
                    // earlier input than itself is a caller that has miscounted.
                    Err("the chain's first table joins to nothing, so it takes no ON")
                } else {
                    Ok(input)
                }
            })
            .map_err(str::to_owned)
            .and_then(|input| {
                // `tables` was built from `spec.inputs`, so a first input
                // implies a first table. A lookup rather than `[0]` because
                // the workspace forbids indexing in this crate, and the reason
                // it does is that everything reaching here came from a text box.
                let Some(table) = tables.first() else {
                    return Err("a chain needs a first table".to_owned());
                };
                conditions(&input.filters, table)
            })?;
        // The first table's own window goes where a join's does — on the chain
        // — so nothing here sets one.
        first.limit = None;

        let mut chain = Chain::from(first);
        // Zipped with `tables` rather than indexed by `at`: the two are
        // the same length by construction, and pairing them says so where
        // `tables[at]` three lines apart only assumes it.
        for (at, (input, table)) in spec.inputs.iter().zip(&tables).enumerate().skip(1) {
            let on = input.on.as_ref().ok_or_else(|| {
                format!(
                    "input {at} (`{}`) joins to nothing; every table after the \
                     first needs an ON",
                    input.table
                )
            })?;
            if on.input as usize >= at {
                // A step may join back to *any* earlier table and to no later
                // one: a key naming a later input reads a column that does not
                // exist yet, which the kernel would resolve to null on every
                // row and return no matches rather than an error.
                return Err(format!(
                    "input {at} (`{}`) joins to input {}, which is itself or \
                     later; a step joins to a table already read",
                    input.table, on.input
                ));
            }
            let earlier = chained_ordinal(on.input, on.column, &refs)?;
            let own = table
                .column(Ordinal(on.own as usize))
                .map(|_| Ordinal(on.own as usize))
                .ok_or_else(|| format!("{} has no column {}", table.name(), on.own))?;
            let mut step = JoinStep::on([JoinKey::new(earlier, own)]);
            step.query = conditions(&input.filters, table)?;
            chain.steps.push(step);
        }

        chain.compute = chained_computes(&spec.compute, &refs)?;
        if let Some(limit) = spec.limit {
            chain.limit = Some(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        chain.offset = usize::try_from(spec.offset).unwrap_or(usize::MAX);

        let grouping = match spec.group_by.as_slice() {
            [] => None,
            wanted_keys => {
                let mut aggregates = Vec::with_capacity(spec.aggregates.len());
                for wanted in &spec.aggregates {
                    let shifted = chained_ordinal(wanted.input, wanted.column, &refs)?;
                    aggregates.push(aggregate_of(&wanted.kind, shifted)?);
                }
                // No implicit `count(*)`. It was here because a grouping
                // with nothing to compute had no meaning when this was a
                // panel with a group-by picker and no aggregate picker — and
                // it is exactly what `SELECT DISTINCT` needs, so a default
                // that quietly appended a column the caller never asked for
                // is now a wrong answer rather than a convenience. A caller
                // that wants a count asks for one.
                let keys: Vec<Ordinal> = wanted_keys.iter().map(|k| Ordinal(*k as usize)).collect();
                let mut grouping = Grouping::by(keys.clone(), &aggregates);
                if !spec.having.is_empty() || !spec.having_any_of.is_empty() {
                    grouping = grouping.having(group_predicate(
                        &spec.having,
                        &spec.having_any_of,
                        &keys,
                        &aggregates,
                        &refs,
                    )?);
                }
                if let Some(limit) = spec.limit {
                    grouping.limit = Some(usize::try_from(limit).unwrap_or(usize::MAX));
                }
                grouping.offset = usize::try_from(spec.offset).unwrap_or(usize::MAX);
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
        let (plan, rows, groups) = block_on(async {
            let snapshot = self.store.snapshot().await?;
            match &grouping {
                Some(grouping) => {
                    // `explain_grouped_chain` returns the *narrowed* chain
                    // beside the plan, and the plan is of that chain — which is
                    // the whole reason it returns both. The narrowed chain is
                    // dropped here because nothing displays it; the plan is
                    // what the panel shows, and it describes what ran.
                    let (_narrowed, plan) =
                        snapshot.explain_grouped_chain(&self.context, &refs, &chain, grouping)?;
                    let groups = snapshot
                        .group_by_chain(&self.context, &refs, &chain, grouping)
                        .await?;
                    Ok::<_, slate_kernel::KernelError>((plan, Vec::new(), groups))
                }
                None => {
                    let plan = snapshot.explain_chain(&self.context, &refs, &chain)?;
                    let mut cursor = snapshot.chain(&self.context, &refs, &chain).await?;
                    let mut out = Vec::new();
                    while let Some(row) = cursor.next().await? {
                        out.push(row);
                    }
                    Ok((plan, out, Vec::new()))
                }
            }
        })
        .map_err(|e| e.to_string())?;
        let elapsed = now_ms() - started;

        let rendered: Vec<Vec<String>> =
            rows.iter()
                .map(|row| {
                    let mut line = Vec::new();
                    // Each table padded to its own width, so a step that preserved
                    // an unmatched row keeps the grid aligned with the header. A
                    // `ChainRow` is also *shorter* than the chain when a right
                    // outer step drops earlier tables, which `at` reports as
                    // `None` — the same case, from the other direction.
                    for (at, table) in refs.iter().enumerate() {
                        match row.at(at) {
                            Some(values) => line.extend(render(values, &column_scales(&[table]))),
                            None => line
                                .extend(std::iter::repeat_n("—".to_owned(), table.columns().len())),
                        }
                    }
                    // A chain's computed values carry no column, so no
                    // scale: a computed decimal renders as its units. The
                    // kernel refuses a computed decimal whose scale is not the
                    // one a column already declares, so the units are the only
                    // thing the display path can honestly show without
                    // re-deriving that answer here.
                    line.extend(row.computed().iter().map(|v| text(v, None)));
                    line
                })
                .collect();

        let rendered_groups: Vec<Vec<String>> = {
            let scales =
                grouping.map_or_else(Vec::new, |g| group_scales(&g.group, &g.aggregates, &refs));
            groups
                .iter()
                .map(|group| {
                    let mut line: Vec<String> = Vec::with_capacity(scales.len());
                    for (at, value) in group.key.iter().chain(&group.values).enumerate() {
                        line.push(text(value, scales.get(at).copied().flatten()));
                    }
                    line
                })
                .collect()
        };

        // One `InputPlan` per table, with each step's algorithm on the table
        // that step reads — the first table has none, because nothing is
        // joined to produce it.
        //
        // `ChainPlan` holds raw `Plan`s and no `Display`, so each one is turned
        // into an `Explanation` here — the same thing `slate-server`'s
        // `chain_plan_to_proto` does, for the same reason, and the display line
        // is built the same way so an operator reading either sees one shape.
        let Some(first_table) = tables.first() else {
            return Err("a chain needs a first table".to_owned());
        };
        let first = Explanation::of(first_table, &plan.first, &chain.first);
        let mut display = format!("Chain\n  -> {first}");
        let mut inputs = vec![input_plan(&first, String::new())];
        for (at, step) in plan.steps.iter().enumerate() {
            let Some(table) = tables.get(at + 1) else {
                break;
            };
            let Some(request) = chain.steps.get(at) else {
                break;
            };
            let explanation = Explanation::of(table, &step.plan, &request.query);
            let algorithm = match step.algorithm {
                JoinAlgorithm::Hash { .. } => "hash",
                JoinAlgorithm::NestedLoop => "nested loop",
            };
            display.push_str(&format!(
                "\n  -> {} step {}: {explanation}",
                match step.algorithm {
                    JoinAlgorithm::Hash { .. } => "Hash",
                    JoinAlgorithm::NestedLoop => "Nested Loop",
                },
                at + 1
            ));
            inputs.push(input_plan(&explanation, algorithm.to_owned()));
        }

        Ok(JoinAnswer {
            kernel_ms: elapsed,
            returned: if rendered_groups.is_empty() {
                rendered.len()
            } else {
                rendered_groups.len()
            },
            rows: rendered,
            groups: rendered_groups,
            inputs,
            display,
        })
    }

    /// A join's or a chain's answer, as a [`SqlResult`].
    ///
    /// The two arms differ only in how they name their columns; everything
    /// after the read is identical, and was written out twice in the first
    /// draft. It took about ten minutes for the two copies to disagree about
    /// whether a grouped read reports `answer.returned` or the row count.
    fn answered(
        text: &str,
        grouped: bool,
        columns: Vec<String>,
        answer: JoinAnswer,
        spec: serde_json::Value,
    ) -> SqlResult {
        let mut out = SqlResult::blank(text, if grouped { "group" } else { "join" });
        out.columns = columns;
        out.returned = answer.returned;
        // A grouped read puts its rows in `groups`, and `rows` is empty — the
        // two are the same shape to the reader, and a binding that returned one
        // of two field names depending on the query is how a UI ends up with
        // two rendering paths that drift.
        out.rows = if answer.groups.is_empty() {
            answer.rows
        } else {
            answer.groups
        };
        out.inputs = answer.inputs;
        out.kernel_ms = answer.kernel_ms;
        out.message = answer.display;
        out.spec = spec;
        out
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
            sql::Statement::Select(mut spec) => {
                if let Err(message) = self.resolve_subqueries(&mut spec) {
                    return SqlResult::failed(text, 0, &message);
                }
                let spec_json = serde_json::to_value(&spec).unwrap_or(serde_json::Value::Null);
                // The same test `answer_spec` dispatches on, and it has to be
                // the same test: this decides the headers and that decides the
                // rows, so testing the keys here and "keys or aggregates"
                // there put four table column names over a three-value
                // aggregate row. A grouping with no keys is still a grouping.
                let grouped = !spec.group_by.is_empty() || !spec.aggregates.is_empty();
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
                            // The computed values and then the windows, in
                            // the order the row carries them — one `extend`
                            // over both counts, because a header list that
                            // stops short of the row's width leaves the last
                            // columns unlabelled and one that overshoots
                            // labels cells that are not there.
                            let extra = spec.compute.len() + spec.window.len();
                            headers.extend((0..extra).map(|i| {
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
                // Cloned rather than moved: the headers need the keys and
                // `joined_spec` takes the spec by value.
                let grouped = spec.group_by.clone();
                let left_table = self
                    .table(&spec.left)
                    .unwrap_or_else(|_| fixture::authors());
                let right_table = self.table(&spec.right).unwrap_or_else(|_| fixture::books());
                // By alias where there is one. Two tables need this as much as
                // n do: `trips AS a JOIN trips AS b` heads one half `a.` and
                // the other `b.`, where the table's own name would head both
                // identically and the grid would show two columns with one name
                // holding different values.
                let inputs = [
                    Named::new(&left_table, &spec.left_alias),
                    Named::new(&right_table, &spec.right_alias),
                ];
                let columns = if grouped.is_empty() {
                    joined_headers(&inputs, &spec.compute)
                } else {
                    grouped_headers(&inputs, &spec.compute, &grouped, &spec.aggregates)
                };
                match self.joined_spec(spec) {
                    Err(message) => SqlResult::failed(text, 0, &message),
                    Ok(answer) => {
                        Self::answered(text, !grouped.is_empty(), columns, answer, spec_json)
                    }
                }
            }
            sql::Statement::Chain(spec) => {
                let spec_json = serde_json::to_value(&spec).unwrap_or(serde_json::Value::Null);
                let grouped = spec.group_by.clone();
                // Resolved before the read, because the headers need the
                // widths and a chain has no fixed number of them. A table the
                // catalog does not have is left to `chained_spec` to refuse by
                // name rather than substituted with a fixture the way the join
                // arm does — that fallback exists there because the panel can
                // send a `JoinSpec` for the books fixture before the taxi data
                // has loaded, and nothing sends a `ChainSpec` but the parser,
                // which resolved every table already.
                let resolved: Result<Vec<TableDef>, String> = spec
                    .inputs
                    .iter()
                    .map(|input| self.table(&input.table))
                    .collect();
                let columns = match &resolved {
                    Err(_) => Vec::new(),
                    Ok(tables) => {
                        // Zipped with the spec's inputs rather than indexed, so
                        // an input keeps its own alias: the two lists are the
                        // same length because one was built from the other.
                        let named: Vec<Named<'_>> = tables
                            .iter()
                            .zip(&spec.inputs)
                            .map(|(table, input)| Named::new(table, &input.alias))
                            .collect();
                        if grouped.is_empty() {
                            joined_headers(&named, &spec.compute)
                        } else {
                            grouped_headers(&named, &spec.compute, &grouped, &spec.aggregates)
                        }
                    }
                };
                match resolved.and_then(|_| self.chained_spec(spec)) {
                    Err(message) => SqlResult::failed(text, 0, &message),
                    Ok(answer) => {
                        Self::answered(text, !grouped.is_empty(), columns, answer, spec_json)
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
                ..FilterSpec::default()
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
        if !spec.having.is_empty() || !spec.having_any_of.is_empty() {
            grouping = grouping.having(group_predicate(
                &spec.having,
                &spec.having_any_of,
                &keys,
                &aggregates,
                &[table],
            )?);
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

        let scales = group_scales(&grouping.group, &grouping.aggregates, &[table]);
        let rows: Vec<Vec<String>> = groups
            .iter()
            .map(|group| {
                let mut line: Vec<String> = Vec::with_capacity(scales.len());
                for (at, value) in group.key.iter().chain(&group.values).enumerate() {
                    line.push(text(value, scales.get(at).copied().flatten()));
                }
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
                        space: match space {
                            0x01 => "rows",
                            0x02 => "index",
                            // 0x03 is per-table schema state, written by
                            // `slate_kernel::migrate`. Nothing in the browser
                            // runs a migration today, so no key of this space
                            // reaches here — but calling it an index and then
                            // decoding it as one is the kind of wrong label
                            // that survives for years, and the arm is a line.
                            0x03 => "schema",
                            _ => "unknown",
                        }
                        .to_owned(),
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
    /// Run every `IN (SELECT …)` and turn it into a candidate list.
    ///
    /// Two reads, not a join: the inner query runs once, its single column
    /// becomes `values`, and the outer one is then an ordinary `Expr::In` that
    /// the planner already turns into point gets or an index range. That is the
    /// whole of subquery support, and it is why none of it reached the kernel.
    ///
    /// The list is rendered to text rather than kept as values, because that is
    /// what a `FilterSpec` holds and what the Spec tab shows — the subquery
    /// stays in the spec beside it, so a reader sees both the query they wrote
    /// and the candidates it produced, which is the thing most worth seeing
    /// when the answer is not the one they expected. `text` and `literal` are
    /// inverses for every type this can produce.
    ///
    /// **Nulls are dropped.** A null candidate can never equal anything, so in
    /// a `WHERE` it excludes the row either way — the kernel would answer
    /// `Unknown` where this answers `False`, and both exclude. It would matter
    /// under a `NOT IN`, which is refused for exactly this reason.
    fn resolve_subqueries(&self, spec: &mut QuerySpec) -> Result<(), String> {
        // Every condition of the `WHERE`, flat or nested. `spec.filters` was
        // the whole of it until parentheses arrived; a subquery inside a
        // bracket lands in `spec.predicate` instead, and missing it here is
        // silent — `build` turns an unresolved subquery into an `IN` over an
        // empty list, which matches nothing and reports nothing. The `any_of`
        // list is included for the same reason even though the parser cannot
        // put a subquery there today: a list this walks and a list it does not
        // is exactly the asymmetry that made this a bug once.
        let mut conditions: Vec<&mut FilterSpec> = Vec::new();
        conditions.extend(spec.filters.iter_mut());
        conditions.extend(spec.any_of.iter_mut());
        if let Some(tree) = spec.predicate.as_mut() {
            collect_conditions(tree, &mut conditions);
        }
        for filter in conditions {
            let Some(inner) = filter.subquery.clone() else {
                continue;
            };
            // Nested subqueries are not refused by the parser and would recurse
            // here; one level is what the grammar can produce today, and this
            // is the line that would need to become a depth check if that
            // changes.
            let rows = self.rows_of(&inner)?;
            let column = inner.columns.first().copied().unwrap_or(0) as usize;
            // The subquery's own table, because the scale of the value it
            // produced is that column's. Rendered and reparsed against the
            // *outer* column below, which is how a subquery over a scale-2
            // column filtering a scale-4 one is caught: the text says `12.50`
            // and the outer column reads it as 125000 units, which is 12.5000
            // — the same number, correctly rescaled. A units-to-units hand-off
            // would have been off by a hundred.
            let inner_scales = column_scales(&[&self.table(&inner.table)?]);
            let scale = inner_scales.get(column).copied().flatten();
            let mut values = Vec::with_capacity(rows.len());
            for row in &rows {
                match row.values().get(column) {
                    Some(Value::Null) | None => {}
                    Some(value) => values.push(text(value, scale)),
                }
            }
            filter.values = values;
        }
        Ok(())
    }

    /// The rows one spec returns, without rendering or timing them.
    ///
    /// For a subquery, which wants the values and none of the presentation.
    fn rows_of(&self, spec: &QuerySpec) -> Result<Vec<Row>, String> {
        let table = self.table(&spec.table)?;
        let query = build(spec, &table)?;
        block_on(async {
            let snapshot = self.store.snapshot().await?;
            let mut cursor = snapshot.execute(&self.context, &table, &query).await?;
            let mut out = Vec::new();
            while let Some(row) = cursor.next().await? {
                out.push(row);
            }
            Ok::<_, slate_kernel::KernelError>(out)
        })
        .map_err(|e| e.to_string())
    }

    fn answer_spec(&self, spec: &QuerySpec) -> Result<Answer, String> {
        let table = self.table(&spec.table)?;

        // Before `build`, which is where a filter becomes an `Expr` and a
        // subquery still holding its inner spec would be an `IN` over an empty
        // list — matching nothing, with no error anywhere.
        let mut spec = spec.clone();
        self.resolve_subqueries(&mut spec)?;
        let spec = &spec;

        let query = build(spec, &table)?;

        // Aggregates with no keys are still a grouping — one group, over every
        // row — so the dispatch is on "does this return groups", not on "are
        // there keys". Testing only the keys is what made `SELECT count(*)
        // FROM trips` unreachable.
        if !spec.group_by.is_empty() || !spec.aggregates.is_empty() {
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

        let scales = column_scales(&[&table]);
        let rows: Vec<Vec<String>> = raw.iter().map(|row| render(row, &scales)).collect();

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
    if space == 0x03 {
        let name = tables
            .iter()
            .find(|t| t.id().0 == id)
            .map_or_else(|| format!("table {id}"), |t| t.name().to_owned());
        return (format!("schema/{name}"), name, None);
    }
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
    // Schema state is one key per table with nothing after the header, so there
    // is nothing to decode and the index decoder below would call it corrupt.
    if space == 0x03 {
        return "schema state".to_owned();
    }
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
                        format!(
                            "{name}={}",
                            text(
                                value,
                                table
                                    .column(*ordinal)
                                    .and_then(slate_schema::ColumnDef::scale)
                            )
                        )
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
            // Each indexed column's own scale, then each primary-key
            // column's, so a decimal in an index entry reads as the number
            // rather than as its units.
            let at = |ordinal: &Ordinal| {
                table
                    .column(*ordinal)
                    .and_then(slate_schema::ColumnDef::scale)
            };
            let indexed: Vec<String> = index
                .columns()
                .iter()
                .zip(&indexed)
                .map(|(column, value)| text(value, at(&column.ordinal)))
                .collect();
            let key_values: Vec<String> = table
                .primary_key()
                .iter()
                .zip(&primary_key)
                .map(|(ordinal, value)| text(value, at(ordinal)))
                .collect();
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
/// What each position of a joined row is called: every table's own columns,
/// then every computed value named as it was written.
///
/// `qualified` prefixes a stored column with its table's name. The two callers
/// want different answers and both are right: an **ungrouped** header is a grid
/// of every table's columns side by side, where three unqualified `id`s are
/// unreadable; a **grouped** key is one column the reader named themselves, and
/// echoing their own spelling is what the single-table path does. Computed
/// values are never qualified either way — `hour(pickup_time)` is the whole
/// name and it is already the reader's.
///
/// One function for the join and the chain arms, and it is also a fix. The
/// join arm built its headers inline and resolved *every* name against the
/// left table: a computed value's column name came from `left.column(c.column)`
/// whatever `c.input` said, and a group key's from `left.column(key)` — where
/// `key` is a *joined* ordinal, so a right-table key was labelled with
/// whichever left-table column happened to sit at that position. The values
/// underneath were right; only the names were wrong, which is the kind of
/// defect a test asserting on cells never sees.
fn joined_names(inputs: &[Named<'_>], compute: &[ComputeSpec], qualified: bool) -> Vec<String> {
    let mut out: Vec<String> = inputs
        .iter()
        .flat_map(|input| {
            input.table.columns().iter().map(move |c| {
                if qualified {
                    // The *alias*, not the table. `zones AS pickup` heads its
                    // columns `pickup.borough`, and with `zones` twice in one
                    // query the table's own name would head both halves of the
                    // row identically -- a grid where two columns called
                    // `zones.borough` hold different boroughs.
                    format!("{}.{}", input.name, c.name())
                } else {
                    c.name().to_owned()
                }
            })
        })
        .collect();
    out.extend(compute.iter().map(|c| {
        // Named against the input `input` names, which is the whole of the
        // fix: `hour(zones.updated_at)` was coming back as `hour(id)`.
        let name = inputs
            .get(c.input as usize)
            .and_then(|i| i.table.column(Ordinal(c.column as usize)))
            .map_or_else(|| c.column.to_string(), |d| d.name().to_owned());
        format!("{}({name}{})", c.function, zone_suffix(c))
    }));
    out
}

/// A table and the name a query calls it by, for the header helpers.
///
/// The parser has its own `Input` for the same idea; this is the borrowed
/// version the binding needs, and the two are deliberately not shared: the
/// parser owns a `TableDef` it cloned out of the schema, and the binding
/// borrows one it just resolved. A shared type would have to own or borrow for
/// both, and the alias is three lines of struct either way.
#[derive(Debug, Clone, Copy)]
struct Named<'a> {
    table: &'a TableDef,
    /// The alias where the spec carried one, else the table's own name.
    name: &'a str,
}

impl<'a> Named<'a> {
    /// Resolve a spec's `(table, alias)` pair into what the headers need.
    fn new(table: &'a TableDef, alias: &'a str) -> Self {
        Named {
            table,
            name: if alias.is_empty() {
                table.name()
            } else {
                alias
            },
        }
    }
}

/// The header an ungrouped join or chain gets: [`joined_names`], qualified.
fn joined_headers(inputs: &[Named<'_>], compute: &[ComputeSpec]) -> Vec<String> {
    joined_names(inputs, compute, true)
}

/// The header a *grouped* join or chain gets: the key, then the aggregates.
///
/// The key's name is looked up in [`joined_names`] by its own ordinal, which
/// needs no special case for a computed key and no arithmetic: the names of
/// those positions are what this list is. The inline version had a `find` over
/// the computed headers and an `or_else` onto the left table, which is the same
/// lookup done twice and wrongly the second time.
fn grouped_headers(
    inputs: &[Named<'_>],
    compute: &[ComputeSpec],
    keys: &[u32],
    aggregates: &[AggregateSpec],
) -> Vec<String> {
    // The key echoes the reader's own spelling, unqualified — *unless* that
    // spelling is ambiguous, in which case it is qualified, because then it was
    // not their spelling either.
    //
    // `SELECT pickup.borough ... GROUP BY pickup.borough` over `zones AS pickup
    // JOIN ... zones AS dropoff` came back headed `borough`, which names both
    // inputs and neither. The rule that produced it — echo what was written —
    // is right, and an alias is the case where the bare name is not what was
    // written. So the test is the same one `resolve_side` warns on: does this
    // column name appear on more than one input?
    let bare = joined_names(inputs, compute, false);
    let qualified = joined_names(inputs, compute, true);
    let mut out: Vec<String> = keys
        .iter()
        .map(|key| {
            let at = *key as usize;
            match bare.get(at) {
                None => key.to_string(),
                Some(name) if bare.iter().filter(|other| *other == name).count() > 1 => {
                    qualified.get(at).cloned().unwrap_or_else(|| name.clone())
                }
                Some(name) => name.clone(),
            }
        })
        .collect();
    out.extend(joined_labels(aggregates, inputs));
    out
}

/// [`labels`], but each aggregate's column resolved against the table its own
/// `input` names.
///
/// The join arm called `labels(&spec.aggregates, &right_table)` — from when an
/// aggregate could only read the right table. Since it can read either,
/// `avg(fare)` over `trips JOIN zones` was labelled with whatever `zones`
/// column sits at `fare`'s ordinal, or with the bare ordinal when `zones` is
/// narrower. Again: right numbers, wrong heading.
fn joined_labels(specs: &[AggregateSpec], inputs: &[Named<'_>]) -> Vec<String> {
    // No implicit `count` any more, here or in the binding. See `labels`.
    specs
        .iter()
        .map(|a| {
            let column = inputs
                .get(a.input as usize)
                .and_then(|i| i.table.column(Ordinal(a.column as usize)))
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

fn labels(specs: &[AggregateSpec], table: &TableDef) -> Vec<String> {
    // No header for an aggregate that is not there. This matched the implicit
    // `count(*)` in `aggregates` and had to be removed with it — a header the
    // rows have no value for shifts every cell one column left, which is the
    // failure mode that made `SELECT count(*) FROM books` return the right
    // numbers under the wrong names.
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

/// What to print above a window's column.
///
/// The call, and the word `over`. Not the whole clause: a partition and an
/// order can be several columns each, and a header wide enough to hold them
/// pushes every other column off the screen — which is a worse answer to
/// "which column is this" than the short form plus the spec panel, where the
/// clause is printed in full.
fn window_header(spec: &WindowSpec, table: &TableDef) -> String {
    let named = |ordinal: u32| -> String {
        table
            .column(Ordinal(ordinal as usize))
            .map_or_else(|| ordinal.to_string(), |c| c.name().to_owned())
    };
    let call = match spec.function.as_str() {
        "lag" | "lead" => format!("{}({}, {})", spec.function, named(spec.column), spec.offset),
        "aggregate" => spec.aggregate.as_ref().map_or_else(
            || "aggregate()".to_owned(),
            |a| match a.kind.as_str() {
                "count" => "count(*)".to_owned(),
                "count_column" => format!("count({})", named(a.column)),
                "count_distinct" => format!("count(distinct {})", named(a.column)),
                other => format!("{other}({})", named(a.column)),
            },
        ),
        other => format!("{other}()"),
    };
    format!("{call} over")
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
    // Past the table's own columns and past the computed ones is a window, in
    // the order the query asked for them — the layout `WindowSpec` documents.
    if let Some(i) = (ordinal as usize).checked_sub(table.columns().len() + spec.compute.len())
        && let Some(window) = spec.window.get(i)
    {
        return window_header(window, table);
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

/// One side of a join: its conditions, ANDed, as a `Query`.
/// One input's plan, for the panel.
///
/// Extracted because the join path builds two of these and the chain path
/// builds one per table, and the five fields were written out three times.
fn input_plan(explanation: &Explanation, algorithm: String) -> InputPlan {
    InputPlan {
        table: explanation.table.clone(),
        access: explanation.access.to_string(),
        index_only: explanation.is_index_only(),
        decodes: explanation.decodes.iter().map(|o| o.0 as u32).collect(),
        algorithm,
    }
}

/// One value as text.
fn text(value: &Value, scale: Option<u8>) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::U64(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        Value::F64(n) => n.to_string(),
        Value::Str(s) => s.clone(),
        // The scale is the column's, so it has to be handed in: a decimal
        // value does not know what it is a count of. Rendering one without it
        // used to fall through to the `{other:?}` arm and show `Decimal(895)`
        // in the results grid — and, worse, the SQL `UPDATE` path reads a row,
        // renders it, edits one cell and writes the strings back, so that text
        // was fed to the parser. `Decimal(895)` is not a number, so an
        // `UPDATE` of any other column on a table with a decimal in it failed.
        // `update_changes_the_named_columns_and_leaves_the_rest` caught it the
        // moment the fixture grew a price.
        //
        // `unwrap_or(0)` is the whole-units reading, which is the honest
        // answer for a caller who has no scale to give: it is what a scale-0
        // decimal column means, and it is a number rather than a debug string.
        Value::Decimal(units) => Value::decimal_to_string(*units, scale.unwrap_or(0)),
        other => format!("{other:?}"),
    }
}

/// Values as text, for a table in a browser.
///
/// `scales` is positional and may be shorter than the row — a position past
/// its end renders with no scale, which is what a computed value or an
/// aggregate that is not a decimal wants.
fn render(row: &Row, scales: &[Option<u8>]) -> Vec<String> {
    row.values()
        .iter()
        .enumerate()
        .map(|(at, value)| text(value, scales.get(at).copied().flatten()))
        .collect()
}

/// Each column's scale, in order, for [`render`].
fn column_scales(tables: &[&TableDef]) -> Vec<Option<u8>> {
    tables
        .iter()
        .flat_map(|t| t.columns().iter().map(slate_schema::ColumnDef::scale))
        .collect()
}

/// The scale of each column a *group* returns: its keys, then its aggregates.
///
/// Answered by [`group_value_type`], which `HAVING` already uses, so the
/// rendering and the comparison cannot disagree about what a group column is.
/// A position it cannot resolve renders with no scale rather than failing —
/// this is the display path, and a grid that refuses to draw is worse than one
/// that shows a count of units.
fn group_scales(
    keys: &[Ordinal],
    aggregates: &[Aggregate],
    inputs: &[&TableDef],
) -> Vec<Option<u8>> {
    (0..keys.len() + aggregates.len())
        .map(|at| {
            u32::try_from(at)
                .ok()
                .and_then(|at| group_value_type(at, keys, aggregates, inputs).ok())
                .and_then(|(_, scale)| scale)
        })
        .collect()
}

/// Every leaf condition of a nested predicate, borrowed mutably.
///
/// Free rather than a method on `PredicateSpec`: it exists for exactly one
/// caller — [`Playground::resolve_subqueries`] — and putting a mutable walker
/// on the spec type would make "hand me every condition so I can rewrite it"
/// part of `slate-sql`'s surface for one consumer in another crate.
fn collect_conditions<'a>(spec: &'a mut PredicateSpec, out: &mut Vec<&'a mut FilterSpec>) {
    match spec {
        PredicateSpec::Of(filter) => out.push(filter),
        PredicateSpec::All(parts) | PredicateSpec::Any(parts) => {
            for part in parts {
                collect_conditions(part, out);
            }
        }
    }
}
