//! Does the grouped path's second flatten show up? No.
//!
//! A grouped join with a computed column flattens each row twice: `JoinCursor`
//! builds the flat row to evaluate the values against, and `flatten_appending`
//! builds it again for the grouper. That was written up as "a real regression,
//! and it is not measured". This measures it.
//!
//!     cargo run --release -p slate-kernel --example flatten_cost
//!
//! The answer is that it is not a cost. Keeping the cursor's row and reusing it
//! — a `flat: Option<Row>` on `JoinedRow` — gave medians of 325.3 and 310.3 ms
//! over two runs, against 320.9 and 342.2 ms rebuilding it, with a
//! within-variant spread of 290 to 363 ms. The difference is well inside the
//! noise, so the change was reverted rather than kept: both paths clone every
//! value exactly once and only the bookkeeping differs.
//!
//! Kept as an example rather than deleted, because the next person to notice
//! the double flatten will have the same idea, and a number is a shorter
//! answer than a paragraph.
#![allow(clippy::unwrap_used, clippy::print_stdout, clippy::indexing_slicing)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Aggregate, Grouping, Join, JoinSchema, Query, RecordStore, Scalar, SecurityCatalog,
    SecurityContext,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const LEFT: TableId = TableId(1);
const RIGHT: TableId = TableId(2);
const ROWS: u64 = 60_000;

/// Rows this runs over, overridable with `KERNELBENCH_ROWS`.
///
/// `scripts/run_examples.sh --smoke` sets it small: CI's job is to prove this
/// still executes against the current tree, and the numbers a smoke run prints
/// are worthless — the file says so rather than letting a reader trust them.
/// A run with the variable unset is the recorded size, which is what
/// `docs/performance.md` quotes.
fn rows() -> u64 {
    std::env::var("KERNELBENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS)
}

fn left() -> TableDef {
    TableDef::builder("l", LEFT)
        .column("id", ValueType::U64)
        .column("a", ValueType::I64)
        .column("b", ValueType::I64)
        .column("c", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap()
}

fn right() -> TableDef {
    TableDef::builder("r", RIGHT)
        .column("id", ValueType::U64)
        .column("lid", ValueType::U64)
        .column("d", ValueType::I64)
        .column("e", ValueType::Str)
        .primary_key(["id"])
        .build()
        .unwrap()
}

#[tokio::main]
async fn main() {
    let (l, r) = (left(), right());
    let catalog = Catalog::from_tables([l.clone(), r.clone()]).unwrap();
    let store = RecordStore::new(MemoryStore::new(), catalog, SecurityCatalog::new());
    let root = SecurityContext::superuser();

    let txn = store.begin().await.unwrap();
    let lrows: Vec<Row> = (0..rows())
        .map(|i| {
            Row::new(vec![
                Value::U64(i),
                Value::I64(i as i64 % 977),
                Value::I64(i as i64),
                Value::Str(format!("left-{i}")),
            ])
        })
        .collect();
    txn.insert_many(&root, &l, &lrows).await.unwrap();
    let rrows: Vec<Row> = (0..rows())
        .map(|i| {
            Row::new(vec![
                Value::U64(i),
                Value::U64(i),
                Value::I64(i as i64 * 3),
                Value::Str(format!("right-{i}")),
            ])
        })
        .collect();
    txn.insert_many(&root, &r, &rrows).await.unwrap();
    txn.commit().await.unwrap();

    let at = JoinSchema::of(&l, &r);
    let join = Join::equating(Ordinal(0), Ordinal(1))
        .left(Query::all())
        .right(Query::all())
        .computing([Scalar::Add(
            Box::new(Scalar::Column(at.left(Ordinal(1)))),
            Box::new(Scalar::Column(at.right(Ordinal(2)))),
        )]);
    let sum_of = at.right(Ordinal(2));
    let key = at.computing(1).computed(0);
    let grouping = Grouping::by([key], &[Aggregate::Count, Aggregate::Sum(sum_of)]);

    // Warm, then five timed runs.
    for _ in 0..2 {
        let txn = store.begin().await.unwrap();
        txn.group_by_join(&root, &l, &r, &join, &grouping)
            .await
            .unwrap();
    }
    let mut times = Vec::new();
    for _ in 0..9 {
        let txn = store.begin().await.unwrap();
        let started = std::time::Instant::now();
        let groups = txn
            .group_by_join(&root, &l, &r, &join, &grouping)
            .await
            .unwrap();
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        assert!(!groups.is_empty());
    }
    times.sort_by(f64::total_cmp);
    println!(
        "grouped join with a computed column over {} rows: \
         min {:.1} ms, median {:.1} ms, max {:.1} ms",
        rows(),
        times[0],
        times[4],
        times[8]
    );
}
