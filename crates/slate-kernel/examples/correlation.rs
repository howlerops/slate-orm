//! How wrong the planner's estimates get when columns are correlated.
//!
//! `Statistics::predicate_selectivity` multiplies the selectivities of a
//! conjunction's parts. That is exact when the columns are independent and
//! wrong when they are not, and real data is full of columns that are not:
//! a city and its country, a status and the timestamp that goes with it, a
//! product and its category.
//!
//! The interesting question is not "is the estimate wrong" — it is wrong, by
//! construction — but **how wrong, and does it change the plan?** An estimate
//! ten times too small that still picks the same access path costs nothing. One
//! that talks the planner out of the right index costs a table scan.
//!
//! Run with:
//!
//! ```sh
//! cargo run --release -p slate-kernel --example correlation
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::print_stdout
)]

use slate_kernel::exec::DEFAULT_PREFETCH;
use slate_kernel::memory::MemoryStore;
use slate_kernel::stats::{SCAN_OPEN_COST, SCAN_ROW_COST, pipelined_read_cost};
use slate_kernel::{
    AccessSummary, Action, Expr, Grant, Query, RecordStore, SecurityCatalog, SecurityContext,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const EVENTS: TableId = TableId(1);
const ROWS: u64 = 20_000;

/// Rows this runs over, overridable with `KERNELBENCH_ROWS`.
///
/// `scripts/run_examples.sh --smoke` sets it small: CI's job is to prove this
/// still executes against the current tree, and the numbers a smoke run prints
/// are worthless — the file says so rather than letting a reader trust them.
/// A run with the variable unset is the recorded size, which is what
/// `docs/performance.md` quotes.
///
/// `row_count` rather than `rows`, which is taken: `fn rows(correlation,
/// domain)` below builds the seeded rows themselves.
fn row_count() -> u64 {
    std::env::var("KERNELBENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS)
}

/// Distinct values per column. Both columns share this domain, so a perfectly
/// correlated pair is `a == b`.
///
/// Two settings, because they answer different questions. With many distinct
/// values the correlated result is still a small slice and an index is right
/// whatever the estimate says. With few, the correlated result is a large
/// fraction of the table — and that is where believing an estimate twenty
/// times too small can talk the planner into an index scan that costs a point
/// read per row when a table scan would have been cheaper.
const DOMAINS: [u64; 2] = [20, 4];

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("id", ValueType::U64)
        .column("a", ValueType::I64)
        .column("b", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_a", IndexId(10)).column("a"))
        .index(IndexDef::builder("by_b", IndexId(11)).column("b"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    events().ordinal_of(name).expect("column exists")
}

const LOOKUP: TableId = TableId(2);

/// A small table to join against, keyed by the value `events.a` holds.
fn lookup() -> TableDef {
    TableDef::builder("lookup", LOOKUP)
        .column("key", ValueType::I64)
        .column("label", ValueType::Str)
        .primary_key(["key"])
        .build()
        .expect("valid schema")
}

/// Deterministic xorshift, so a run is reproducible and a surprising number can
/// be looked at again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Rows where `b` equals `a` with probability `correlation`, and is otherwise
/// drawn independently.
///
/// At 0.0 the columns are independent and the planner's assumption holds
/// exactly. At 1.0 they are the same column twice, which is the worst case and
/// also a shape that occurs in practice — a denormalised copy, or an id and the
/// slug derived from it.
fn rows(correlation: f64, domain: u64) -> Vec<Row> {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let threshold = (correlation * 1000.0) as u64;
    (0..row_count())
        .map(|id| {
            let a = rng.below(domain);
            let b = if rng.below(1000) < threshold {
                a
            } else {
                rng.below(domain)
            };
            Row::new(vec![
                Value::U64(id),
                Value::I64(a as i64),
                Value::I64(b as i64),
            ])
        })
        .collect()
}

async fn seeded(correlation: f64, domain: u64) -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([events(), lookup()]).expect("catalog");
    let security = SecurityCatalog::new()
        .grant(Grant::new("r", EVENTS, Action::ALL))
        .grant(Grant::new("r", LOOKUP, Action::ALL));
    let mut store = RecordStore::new(MemoryStore::new(), catalog, security);
    let table = events();
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    txn.insert_many(&root, &table, &rows(correlation, domain))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    let txn = store.begin().await.unwrap();
    let rows: Vec<Row> = (0..domain)
        .map(|k| Row::new(vec![Value::I64(k as i64), Value::Str(format!("label-{k}"))]))
        .collect();
    txn.insert_many(&root, &lookup(), &rows).await.unwrap();
    txn.commit().await.unwrap();

    // The planner is only as good as what `analyze` told it.
    let (events_stats, lookup_stats) = {
        let txn = store.begin().await.unwrap();
        (
            txn.analyze(&root, &table).await.unwrap(),
            txn.analyze(&root, &lookup()).await.unwrap(),
        )
    };
    let mut all = slate_kernel::Statistics::default();
    all.set(EVENTS, events_stats);
    all.set(LOOKUP, lookup_stats);
    store.set_statistics(all);
    store
}

/// Does a correlated filter on the outer side change the join algorithm?
///
/// `nested_loop_cost` multiplies the outer side's *estimated* row count by the
/// cost of one probe. When that estimate is twenty times too small, a nested
/// loop looks twenty times cheaper than it is — which is the one place in the
/// planner where the independence assumption has a direct lever on a decision
/// rather than on a number nobody compares.
async fn join_choice(store: &RecordStore<MemoryStore>, filter: &Expr) -> (String, f64, usize) {
    let root = SecurityContext::superuser();
    let join = slate_kernel::Join::equating(col("a"), lookup().ordinal_of("key").unwrap())
        .left(Query::all().filter(filter.clone()));

    let txn = store.begin().await.unwrap();
    let explained = txn
        .explain_join(&root, &events(), &lookup(), &join)
        .unwrap();
    let actual = txn
        .join(&root, &events(), &lookup(), &join)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .len();
    (
        format!("{:?}", explained.algorithm),
        explained.left.estimated_rows,
        actual,
    )
}

/// What the planner thinks, what is true, and which path it picked.
struct Outcome {
    estimated: f64,
    actual: usize,
    access: String,
    /// What the chosen plan really costs, and what the cheapest plan would
    /// have cost — both computed from *measured* row counts through the same
    /// cost model, so the comparison is about the choice rather than about the
    /// estimate that drove it. This is what a bad estimate actually costs.
    chosen_cost: f64,
    best_cost: f64,
}

/// The cost model, applied to numbers that are known rather than estimated.
fn true_cost(walked: usize, point_reads: usize) -> f64 {
    SCAN_OPEN_COST
        + walked as f64 * SCAN_ROW_COST
        + pipelined_read_cost(point_reads as f64, DEFAULT_PREFETCH)
}

async fn measure(store: &RecordStore<MemoryStore>, filter: &Expr) -> Outcome {
    let table = events();
    let root = SecurityContext::superuser();
    let query = Query::all().filter(filter.clone());

    let txn = store.begin().await.unwrap();
    let explained = txn.explain(&root, &table, &query).unwrap();
    let actual = txn
        .execute(&root, &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .len();

    // What each path would really cost, from measured row counts. An index
    // scan walks the entries its bound admits and pays a point read for each;
    // a table scan walks everything and pays none.
    let by_a = count_matching(store, &only_a(filter)).await;
    let by_b = count_matching(store, &only_b(filter)).await;
    let costs = [
        ("by_a", true_cost(by_a, by_a)),
        ("by_b", true_cost(by_b, by_b)),
        ("scan", true_cost(row_count() as usize, 0)),
    ];

    let (access, chosen_cost) = match &explained.access {
        AccessSummary::TableScan => ("TableScan", true_cost(row_count() as usize, 0)),
        AccessSummary::IndexScan { index, .. } | AccessSummary::IndexOnlyScan { index, .. } => {
            let cost = costs
                .iter()
                .find(|(name, _)| name == index)
                .map_or(f64::NAN, |(_, c)| *c);
            ("IndexScan", cost)
        }
        _ => ("other", true_cost(row_count() as usize, 0)),
    };

    Outcome {
        estimated: explained.estimated_rows,
        actual,
        access: access.to_owned(),
        chosen_cost,
        best_cost: costs.iter().map(|(_, c)| *c).fold(f64::INFINITY, f64::min),
    }
}

/// The half of a two-column conjunction that touches `a`.
fn only_a(filter: &Expr) -> Expr {
    match filter {
        Expr::And(parts) => parts
            .iter()
            .find(|p| p.columns().contains(&col("a")))
            .cloned()
            .unwrap_or(Expr::True),
        other => other.clone(),
    }
}

fn only_b(filter: &Expr) -> Expr {
    match filter {
        Expr::And(parts) => parts
            .iter()
            .find(|p| p.columns().contains(&col("b")))
            .cloned()
            .unwrap_or(Expr::True),
        other => other.clone(),
    }
}

async fn count_matching(store: &RecordStore<MemoryStore>, filter: &Expr) -> usize {
    let table = events();
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    txn.execute(&root, &table, &Query::all().filter(filter.clone()))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap()
        .len()
}

#[tokio::main]
async fn main() {
    println!(
        "Correlated-column estimates, {} rows. The planner assumes the two\n\
         columns are independent and multiplies their selectivities; the data\n\
         says otherwise by the amount in `corr`.\n",
        row_count()
    );

    for domain in DOMAINS {
        let each = row_count() / domain;
        println!(
            "\n{domain} distinct values per column, so one value is about {each} rows \
             ({:.0}% of the table).",
            100.0 * each as f64 / row_count() as f64
        );
        println!(
            "{:>6}  {:>10}  {:>9}  {:>8}  {:>9}  {:>12}  {:>10}",
            "corr", "predicate", "estimate", "actual", "error", "plan", "scan cost"
        );
        println!("{:-<78}", "");

        for correlation in [0.0, 0.5, 0.9, 1.0] {
            let store = seeded(correlation, domain).await;
            let hit = 1i64;
            let miss = 2i64;

            for (label, filter) in [
                (
                    "a=1 & b=1",
                    Expr::eq(col("a"), Value::I64(hit)).and(Expr::eq(col("b"), Value::I64(hit))),
                ),
                (
                    "a=1 & b=2",
                    Expr::eq(col("a"), Value::I64(hit)).and(Expr::eq(col("b"), Value::I64(miss))),
                ),
            ] {
                let out = measure(&store, &filter).await;
                let error = if out.actual == 0 {
                    f64::INFINITY
                } else {
                    out.estimated / out.actual as f64
                };
                let penalty = if out.best_cost <= 0.0 {
                    1.0
                } else {
                    out.chosen_cost / out.best_cost
                };
                println!(
                    "{correlation:>6.2}  {label:>10}  {:>9.1}  {:>8}  {:>8.2}x  {:>12}  {:>9.2}x",
                    out.estimated, out.actual, error, out.access, penalty
                );
            }
        }
    }

    println!("\n\nJoin algorithm, with the correlated filter on the outer side.");
    println!(
        "{:>6}  {:>8}  {:>14}  {:>12}  {:>20}",
        "corr", "domain", "outer estimate", "outer actual", "algorithm"
    );
    println!("{:-<70}", "");
    for domain in DOMAINS {
        for correlation in [0.0, 1.0] {
            let store = seeded(correlation, domain).await;
            let filter = Expr::eq(col("a"), Value::I64(1)).and(Expr::eq(col("b"), Value::I64(1)));
            let outer_actual = count_matching(&store, &filter).await;
            let (algorithm, outer_estimate, _joined) = join_choice(&store, &filter).await;
            println!(
                "{correlation:>6.2}  {domain:>8}  {outer_estimate:>14.1}  {outer_actual:>12}  {algorithm:>20}"
            );
        }
    }

    println!(
        "\n`error` is estimate / actual: below 1.0 the planner expects fewer rows\n\
         than exist. `scan cost` is what the chosen plan really costs against the\n\
         cheapest plan available, both measured the same way — 1.00x means the\n\
         estimate was wrong but the choice was still right."
    );
}
