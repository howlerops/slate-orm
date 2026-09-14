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

use futures::executor::block_on;
use serde::{Deserialize, Serialize};
use slate_kernel::{
    Aggregate, CmpOp, Expr, Grouping, Join, JoinAlgorithm, JoinKey, Query, RecordStore, ScanOrder,
    SortKey,
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

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: u32,
    #[serde(default)]
    pub descending: bool,
}

/// What comes back: the rows, and the plan that produced them.
#[derive(Serialize)]
struct Answer {
    rows: Vec<Vec<String>>,
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
/// The join is always `books.author_id = authors.id` — the only one the
/// fixture has, and hard-coding it keeps the panel from offering key choices
/// that produce nothing. What varies is the filter on each side, and whether
/// the result is grouped.
#[derive(Deserialize, Serialize, Default, Debug, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct JoinSpec {
    /// Conditions on `authors`, ANDed.
    pub authors: Vec<FilterSpec>,
    /// Conditions on `books`, ANDed.
    pub books: Vec<FilterSpec>,
    /// Group by this column of `authors` (ordinal in the *authors* table).
    /// Absent means return joined rows rather than groups.
    pub group_by: Option<u32>,
    /// `count`, and optionally `min`/`max` over a `books` column.
    pub aggregates: Vec<AggregateSpec>,
    pub limit: Option<u64>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AggregateSpec {
    pub kind: String,
    /// Ordinal within `books`, ignored by `count`.
    #[serde(default)]
    pub column: u32,
}

/// Joined rows, or groups, with the plan for either.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JoinAnswer {
    /// One entry per row: the author's columns, then the book's.
    rows: Vec<Vec<String>>,
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

/// A seeded kernel: two tables, an index, and the rows the site talks about.
///
/// `Debug` is hand-written rather than derived: `RecordStore` holds the whole
/// seeded dataset, so a derived one would print several thousand rows the
/// first time anybody put this in a `dbg!`.
#[wasm_bindgen]
pub struct Playground {
    store: RecordStore<MemoryStore>,
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
            .grant(Grant::new("app", fixture::BOOKS, Action::EVERYTHING));
        let store = RecordStore::new(MemoryStore::new(), fixture::catalog(), security);

        // Seeded as the superuser and queried as `app`, so the playground
        // exercises the ordinary authorised path rather than the one that
        // skips the checks.
        let root = SecurityContext::superuser();
        let store = block_on(async {
            let txn = store.begin().await.expect("begin");
            txn.insert_many(&root, &fixture::authors(), &fixture::author_rows())
                .await
                .expect("seed authors");
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
            let stats = {
                let txn = store.begin().await.expect("begin");
                let authors = txn
                    .analyze(&root, &fixture::authors())
                    .await
                    .expect("analyze authors");
                let books = txn
                    .analyze(&root, &fixture::books())
                    .await
                    .expect("analyze books");
                Statistics::new()
                    .with(fixture::AUTHORS, authors)
                    .with(fixture::BOOKS, books)
            };
            store.with_statistics(stats)
        });

        Self {
            store,
            context: SecurityContext::new(
                Principal::new(Value::Str("reader".to_owned())).with_role("app"),
            ),
        }
    }

    /// The schema, as JSON, so the UI can build its controls from the kernel's
    /// own catalog rather than from a copy that can drift from it.
    #[must_use]
    pub fn schema(&self) -> String {
        let tables: Vec<TableInfo> = [fixture::authors(), fixture::books()]
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
            Ok(message) => serde_json::json!({ "ok": message }).to_string(),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }

    /// Replace a row that already exists, by primary key.
    #[must_use]
    pub fn update(&self, table: &str, values: &str) -> String {
        match self.write(table, values, Write::Update) {
            Ok(message) => serde_json::json!({ "ok": message }).to_string(),
            Err(message) => serde_json::json!({ "error": message }).to_string(),
        }
    }

    /// Delete by primary key. `values` is the key alone, not a whole row.
    #[must_use]
    pub fn delete(&self, table: &str, key: &str) -> String {
        match self.remove(table, key) {
            Ok(message) => serde_json::json!({ "ok": message }).to_string(),
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
    fn write(&self, table: &str, values: &str, which: Write) -> Result<String, String> {
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

        Ok(match which {
            Write::Insert => "inserted".to_owned(),
            Write::Update => "updated".to_owned(),
        })
    }

    fn remove(&self, table: &str, key: &str) -> Result<String, String> {
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

        let gone = block_on(async {
            let txn = self.store.begin().await?;
            let gone = txn.delete(&self.context, &table, &values).await?;
            txn.commit().await?;
            Ok::<_, slate_kernel::KernelError>(gone)
        })
        .map_err(|e| e.to_string())?;

        Ok(if gone {
            "deleted".to_owned()
        } else {
            "no row with that key".to_owned()
        })
    }

    /// The body of [`Playground::join`].
    fn joined(&self, spec: &str) -> Result<JoinAnswer, String> {
        let spec: JoinSpec = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        self.joined_spec(spec)
    }

    /// The same, from a spec that is already a value. See [`Self::answer_spec`].
    fn joined_spec(&self, spec: JoinSpec) -> Result<JoinAnswer, String> {
        let authors = fixture::authors();
        let books = fixture::books();

        // `authors.id = books.author_id`. The left table is `authors`, so the
        // joined row is the author's columns followed by the book's, and a
        // group key of `authors.country` is ordinal 2 in both spaces.
        let mut join = Join::on([JoinKey::new(Ordinal(0), Ordinal(1))]);
        join.left = conditions(&spec.authors, &authors)?;
        join.right = conditions(&spec.books, &books)?;
        if let Some(limit) = spec.limit {
            join.limit = Some(usize::try_from(limit).unwrap_or(usize::MAX));
        }

        let grouping = match spec.group_by {
            None => None,
            Some(key) => {
                let mut aggregates = Vec::with_capacity(spec.aggregates.len());
                for wanted in &spec.aggregates {
                    // A `books` ordinal has to be shifted into the joined
                    // row's space, which begins after every `authors` column.
                    // This is exactly the arithmetic `ColumnRef` exists to
                    // remove in the clients, and doing it by hand here is the
                    // reason the tests below check a `max(year)` against a
                    // value computed independently.
                    let shifted = Ordinal(authors.columns().len() + wanted.column as usize);
                    aggregates.push(match wanted.kind.as_str() {
                        "count" => Aggregate::Count,
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
                Some(Grouping::by([Ordinal(key as usize)], &aggregates))
            }
        };

        let tables = [&authors, &books];
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
                        let mut line = Vec::new();
                        for side in [&row.left, &row.right] {
                            match side {
                                Some(values) => line.extend(render(values)),
                                // An outer join's missing side. Inner joins
                                // never produce one, but rendering it as text
                                // rather than skipping keeps the columns
                                // aligned with the header.
                                None => line.extend(std::iter::repeat_n(
                                    "—".to_owned(),
                                    tables[0].columns().len(),
                                )),
                            }
                        }
                        out.push(line);
                    }
                    Ok((explanation, out, Vec::new()))
                }
            }
        })
        .map_err(|e| e.to_string())?;

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
        let tables = [fixture::authors(), fixture::books()];
        let schema = sql::Schema(&tables);
        let mut out = Vec::new();
        for (offset, statement) in sql::split(buffer) {
            let result = match sql::parse(&statement, &schema) {
                // The parser reports an offset within its own statement; the
                // editor holds the whole buffer, so it is shifted here. Doing
                // it in the parser would make it wrong for every other caller.
                Err(e) => SqlResult::failed(&statement, offset + e.at, &e.message),
                Ok(parsed) => self.statement(&statement, parsed),
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
                let columns = self
                    .table(&spec.table)
                    .map(|t| t.columns().iter().map(|c| c.name().to_owned()).collect())
                    .unwrap_or_default();
                match self.answer_spec(&spec) {
                    Err(message) => SqlResult::failed(text, 0, &message),
                    Ok(answer) => {
                        let mut out = SqlResult::blank(text, "select");
                        out.columns = columns;
                        out.returned = answer.returned;
                        out.rows = answer.rows;
                        out.plan = Some(answer.plan);
                        out.spec = spec_json;
                        out
                    }
                }
            }
            sql::Statement::Join(spec) => {
                let spec_json = serde_json::to_value(&spec).unwrap_or(serde_json::Value::Null);
                let grouped = spec.group_by;
                let labels: Vec<String> = spec
                    .aggregates
                    .iter()
                    .map(|a| match a.kind.as_str() {
                        "count" => "count(*)".to_owned(),
                        other => format!(
                            "{other}({})",
                            fixture::books()
                                .column(Ordinal(a.column as usize))
                                .map_or_else(|| a.column.to_string(), |c| c.name().to_owned())
                        ),
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
                                    fixture::authors()
                                        .column(Ordinal(key as usize))
                                        .map_or_else(|| key.to_string(), |c| c.name().to_owned()),
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
                            None => fixture::authors()
                                .columns()
                                .iter()
                                .map(|c| format!("authors.{}", c.name()))
                                .chain(
                                    fixture::books()
                                        .columns()
                                        .iter()
                                        .map(|c| format!("books.{}", c.name())),
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

    fn wrote(&self, text: &str, outcome: Result<String, String>) -> SqlResult {
        match outcome {
            Err(message) => SqlResult::failed(text, 0, &message),
            Ok(message) => {
                let mut out = SqlResult::blank(text, "write");
                out.message = message;
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
    fn patch(&self, table: &str, key: &str, set: &[(u32, String)]) -> Result<String, String> {
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
        self.write(table, &json, Write::Update)
    }

    fn table(&self, name: &str) -> Result<TableDef, String> {
        match name {
            "authors" => Ok(fixture::authors()),
            "books" => Ok(fixture::books()),
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

        // One snapshot answers both, and the *same* `Query` value is handed to
        // `explain` and to `execute`. Explaining a query rebuilt to look like
        // the one that ran is how an `EXPLAIN` comes to describe a plan
        // nothing executes; the kernel makes avoiding that free, so there is
        // no excuse for the other shape.
        let (explanation, rows) = block_on(async {
            let snapshot = self.store.snapshot().await?;
            let explanation = snapshot.explain(&self.context, &table, &query)?;
            let mut cursor = snapshot.execute(&self.context, &table, &query).await?;
            let mut out = Vec::new();
            while let Some(row) = cursor.next().await? {
                out.push(render(&row));
            }
            Ok::<_, slate_kernel::KernelError>((explanation, out))
        })
        .map_err(|e| e.to_string())?;

        Ok(Answer {
            returned: rows.len(),
            rows,
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
