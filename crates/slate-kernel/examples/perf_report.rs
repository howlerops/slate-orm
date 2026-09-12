//! What each query shape actually costs in I/O.
//!
//! Wall-clock numbers move with the machine; the number of point reads a plan
//! issues does not. This prints both, so a change can be argued for on the
//! count and confirmed on the clock.
//!
//! ```sh
//! cargo run --release -p slate-kernel --example perf_report
//! ```
//!
//! Charges are modelled at object-storage scale: a point read costs a
//! millisecond, and a scan pays that once to open plus once per block.

#![allow(clippy::expect_used, clippy::print_stdout)]

use slate_kernel::latency::{LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, Projection, Query, RecordStore, ScanOrder, SecurityCatalog,
    SecurityContext, Statistics,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};
use std::time::Instant;
use uuid::Uuid;

const EVENTS: TableId = TableId(1);
const TENANTS: u128 = 4;
const ROWS_PER_TENANT: u64 = 2_500;

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("actor", ValueType::Str)
        .column("at", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .index(IndexDef::builder("by_actor", IndexId(11)).column("actor"))
        .index(IndexDef::builder("by_at_desc", IndexId(12)).column_with("at", Direction::Desc))
        .build()
        .expect("valid schema")
}

fn column(name: &str) -> Ordinal {
    events().ordinal_of(name).expect("column exists")
}

fn row(tenant: u128, id: u64) -> Row {
    Row::new(vec![
        Value::Uuid(Uuid::from_u128(tenant)),
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 25)),
        Value::Str(format!("actor-{}", id % 500)),
        Value::I64(id as i64),
        Value::Null,
    ])
}

#[tokio::main]
async fn main() {
    let table = events();
    let catalog = Catalog::from_tables([table.clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("bench", EVENTS, Action::ALL));
    let root = SecurityContext::superuser();

    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), security.clone());
    for tenant in 0..TENANTS {
        let txn = loader.begin().await.expect("begin");
        for id in 0..ROWS_PER_TENANT {
            txn.insert(&root, &table, &row(tenant, id))
                .await
                .expect("insert");
        }
        txn.commit().await.expect("commit");
    }

    // Statistics first: without them the planner has to guess how many rows a
    // predicate selects, and guessing structurally is what made it pick a plan
    // 30x slower than the alternative.
    let analyzed = {
        let txn = loader.begin().await.expect("begin");
        txn.analyze(&root, &table).await.expect("analyze")
    };
    println!(
        "analyzed {} rows; kind has {} distinct values, at has {}\n",
        analyzed.row_count,
        analyzed.column(column("kind")).distinct,
        analyzed.column(column("at")).distinct,
    );

    let slow = LatencyStore::new(backing, LatencyProfile::object_storage());
    let counters = slow.counters();
    let store = RecordStore::new(slow, catalog, security)
        .with_statistics(Statistics::new().with(EVENTS, analyzed));

    let tenant = Value::Uuid(Uuid::from_u128(0));
    let by_tenant = || Expr::eq(column("tenant_id"), tenant.clone());
    let kind_7 = || Expr::eq(column("kind"), Value::Str("kind-7".into()));
    let early = || Expr::compare(column("at"), CmpOp::Lt, Value::I64(500));

    let all = Projection::All;
    let keys_only = Projection::Columns(vec![column("id"), column("kind")]);
    // Counting needs no columns of its own, only the predicate's.
    let nothing = Projection::none();

    struct Case<'a> {
        label: &'a str,
        filter: Expr,
        limit: Option<usize>,
        projection: &'a Projection,
    }

    let cases = vec![
        Case {
            label: "point get by primary key",
            filter: by_tenant().and(Expr::eq(column("id"), Value::U64(1234))),
            limit: None,
            projection: &all,
        },
        Case {
            label: "whole tenant (2500 rows)",
            filter: by_tenant(),
            limit: None,
            projection: &all,
        },
        Case {
            label: "indexed equality (~100 rows)",
            filter: by_tenant().and(kind_7()),
            limit: None,
            projection: &all,
        },
        Case {
            label: "indexed equality, limit 10",
            filter: by_tenant().and(kind_7()),
            limit: Some(10),
            projection: &all,
        },
        Case {
            label: "indexed range (~500 rows)",
            filter: by_tenant().and(early()),
            limit: None,
            projection: &all,
        },
        Case {
            label: "unindexed filter (0 rows, full scan)",
            filter: by_tenant().and(Expr::eq(column("note"), Value::Str("never".into()))),
            limit: None,
            projection: &all,
        },
        // The same two queries, asking only for columns an index already holds.
        Case {
            label: "covered: indexed equality, keys only",
            filter: by_tenant().and(kind_7()),
            limit: None,
            projection: &keys_only,
        },
        Case {
            label: "covered: count over an index",
            filter: by_tenant().and(early()),
            limit: None,
            projection: &nothing,
        },
    ];

    println!(
        "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12}  plan",
        "query", "rows", "gets", "scans", "scan rows", "wall"
    );
    println!("{:-<118}", "");

    for query in cases {
        let mut request = Query::all()
            .filter(query.filter)
            .order(ScanOrder::Ascending);
        request.projection = query.projection.clone();
        if let Some(limit) = query.limit {
            request = request.limit(limit);
        }

        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        let described = txn
            .explain(&root, &table, &request)
            .expect("explain")
            .access
            .to_string();
        let rows = txn
            .execute(&root, &table, &request)
            .await
            .expect("query")
            .count()
            .await
            .expect("count");
        let elapsed = started.elapsed();

        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}  {}",
            query.label,
            rows,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            elapsed,
            described
        );
    }

    println!("\nwrites");
    println!("{:-<118}", "");
    let mut next_id = 1_000_000u64;
    for (label, batch) in [
        ("insert 1 row", 1u64),
        ("insert 100 rows, one at a time", 100),
    ] {
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        for _ in 0..batch {
            txn.insert(&root, &table, &row(0, next_id))
                .await
                .expect("insert");
            next_id += 1;
        }
        txn.commit().await.expect("commit");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}",
            label,
            batch,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed()
        );
    }

    // The same work, with the reads issued together instead of in turn.
    for batch in [100u64, 1000] {
        let rows: Vec<_> = (0..batch)
            .map(|_| {
                let r = row(0, next_id);
                next_id += 1;
                r
            })
            .collect();
        counters.reset();
        let started = Instant::now();
        let txn = store.begin().await.expect("begin");
        txn.insert_many(&root, &table, &rows)
            .await
            .expect("insert_many");
        txn.commit().await.expect("commit");
        println!(
            "{:<38} {:>6} {:>8} {:>7} {:>10} {:>12?}",
            format!("insert {batch} rows, batched"),
            batch,
            counters.gets(),
            counters.scans(),
            counters.scan_rows(),
            started.elapsed()
        );
    }
}
