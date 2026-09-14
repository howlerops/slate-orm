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

use futures::executor::block_on;
use serde::{Deserialize, Serialize};
use slate_kernel::{
    CmpOp, Expr, Query, RecordStore, ScanOrder, SortKey,
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
}

#[derive(Serialize)]
struct ColumnInfo {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    ordinal: u32,
}

/// What the UI sends. Every field optional, because the panel builds it up.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct QuerySpec {
    table: String,
    filter: Option<FilterSpec>,
    sort: Vec<SortSpec>,
    limit: Option<u64>,
    offset: u64,
    /// Column ordinals to return. Empty means every column — which is also
    /// what makes an index-only scan impossible, so the UI exposes it.
    columns: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterSpec {
    column: u32,
    op: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SortSpec {
    column: u32,
    #[serde(default)]
    descending: bool,
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
            })
            .collect();
        serde_json::to_string(&tables).expect("the schema serialises")
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

impl Playground {
    /// The body of [`Playground::run`], with a real error type.
    ///
    /// Split out so the native test suite can assert on the failures rather
    /// than on their JSON rendering.
    fn answer(&self, spec: &str) -> Result<Answer, String> {
        let spec: QuerySpec = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        let table = match spec.table.as_str() {
            "authors" => fixture::authors(),
            "books" => fixture::books(),
            other => return Err(format!("no such table: {other}")),
        };

        let query = build(&spec, &table)?;

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

    if let Some(filter) = &spec.filter {
        query = query.filter(comparison(filter, table)?);
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

/// Values as text, for a table in a browser.
fn render(row: &Row) -> Vec<String> {
    row.values()
        .iter()
        .map(|value| match value {
            Value::Null => "null".to_owned(),
            Value::Bool(b) => b.to_string(),
            Value::U64(n) => n.to_string(),
            Value::I64(n) => n.to_string(),
            Value::F64(n) => n.to_string(),
            Value::Str(s) => s.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}
