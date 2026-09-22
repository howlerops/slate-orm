//! Read and write paths, measured with and without I/O cost.
//!
//! Two profiles are used throughout. `free` isolates CPU: encoding, decoding,
//! planning, predicate evaluation. `io` charges object-storage round trips so
//! that plans which issue many point lookups are distinguishable from plans
//! that scan — the distinction the whole read path is built around, and one an
//! in-memory map hides completely.

// A benchmark that cannot set itself up should stop, loudly.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use criterion::{BenchmarkId, Criterion, criterion_group};
use slate_kernel::latency::{LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, CmpOp, Expr, Grant, RecordStore, ScanOrder, SecurityCatalog, SecurityContext, keys,
    plan,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::Runtime;
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
        // A handful of distinct kinds, so an equality on `kind` selects many
        // rows — the case where the per-row lookup cost shows up.
        Value::Str(format!("kind-{}", id % 25)),
        Value::Str(format!("actor-{}", id % 500)),
        Value::I64(id as i64),
        None::<String>.map_or(Value::Null, Value::Str),
    ])
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("bench", EVENTS, Action::ALL))
}

type Store = RecordStore<LatencyStore<MemoryStore>>;

fn build(runtime: &Runtime, profile: LatencyProfile) -> Store {
    let catalog = Catalog::from_tables([events()]).expect("catalog");
    // Load with no latency charged, then wrap: the fixture is not the thing
    // under measurement.
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), security());
    let table = events();
    let root = SecurityContext::superuser();

    runtime.block_on(async {
        // Batch the load: one transaction per tenant keeps the write set from
        // growing large enough to dominate setup.
        for tenant in 0..TENANTS {
            let txn = loader.begin().await.expect("begin");
            for id in 0..ROWS_PER_TENANT {
                txn.insert(&root, &table, &row(tenant, id))
                    .await
                    .expect("insert");
            }
            txn.commit().await.expect("commit");
        }
    });

    RecordStore::new(LatencyStore::new(backing, profile), catalog, security())
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

fn tenant_value(tenant: u128) -> Value {
    Value::Uuid(Uuid::from_u128(tenant))
}

/// Planning is pure CPU and happens on every query, so it should be noise.
fn planning(c: &mut Criterion) {
    let table = events();
    let predicate = Expr::eq(column("tenant_id"), tenant_value(0))
        .and(Expr::eq(column("kind"), Value::Str("kind-3".into())))
        .and(Expr::compare(column("at"), CmpOp::Ge, Value::I64(100)));

    let mut group = c.benchmark_group("plan");
    group.bench_function("three_term_predicate", |b| {
        b.iter(|| {
            plan(
                black_box(&table),
                black_box(&predicate),
                ScanOrder::Ascending,
            )
        });
    });
    group.bench_function("row_key", |b| {
        let pk = vec![tenant_value(0), Value::U64(1234)];
        b.iter(|| keys::row_key(black_box(&table), black_box(&pk)));
    });
    group.finish();
}

fn reads(c: &mut Criterion) {
    let runtime = Runtime::new().expect("runtime");
    let table = events();

    for (label, profile) in [
        ("free", LatencyProfile::free()),
        ("io", LatencyProfile::object_storage()),
    ] {
        let store = build(&runtime, profile);
        let mut group = c.benchmark_group(format!("read/{label}"));
        // The I/O-charged variants are slow by construction; do not wait for
        // criterion's default sample count.
        if label == "io" {
            group.sample_size(20);
        }

        group.bench_function("point_get", |b| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.get(&root(), &table, &[tenant_value(0), Value::U64(1234)])
                    .await
                    .expect("get")
            });
        });

        // A whole tenant: 2,500 rows, served by one scan of a key prefix.
        group.bench_function("tenant_scan", |b| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.query(
                    &root(),
                    &table,
                    Expr::eq(column("tenant_id"), tenant_value(0)),
                    ScanOrder::Ascending,
                )
                .await
                .expect("query")
                .count()
                .await
                .expect("count")
            });
        });

        // An equality on an indexed column: one index scan, then one point
        // lookup per matching row. 2,500 rows over 25 kinds is ~100 lookups.
        group.bench_function("index_scan_100_rows", |b| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.query(
                    &root(),
                    &table,
                    Expr::eq(column("tenant_id"), tenant_value(0))
                        .and(Expr::eq(column("kind"), Value::Str("kind-7".into()))),
                    ScanOrder::Ascending,
                )
                .await
                .expect("query")
                .count()
                .await
                .expect("count")
            });
        });

        // The same selectivity, but answered by scanning and filtering instead.
        // The comparison between this and the row above is the whole question
        // of when an index is worth using.
        group.bench_function("filtered_scan_100_rows", |b| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.query(
                    &root(),
                    &table,
                    Expr::eq(column("tenant_id"), tenant_value(0))
                        .and(Expr::eq(column("note"), Value::Str("never".into()))),
                    ScanOrder::Ascending,
                )
                .await
                .expect("query")
                .count()
                .await
                .expect("count")
            });
        });

        // Only the first few rows are wanted; the plan should stop early.
        group.bench_function("index_scan_limit_10", |b| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.query(
                    &root(),
                    &table,
                    Expr::eq(column("tenant_id"), tenant_value(0))
                        .and(Expr::eq(column("kind"), Value::Str("kind-7".into()))),
                    ScanOrder::Ascending,
                )
                .await
                .expect("query")
                .limit(10)
                .count()
                .await
                .expect("count")
            });
        });

        group.finish();
    }
}

fn writes(c: &mut Criterion) {
    let runtime = Runtime::new().expect("runtime");
    let table = events();
    let next = AtomicU64::new(1_000_000);

    for (label, profile) in [
        ("free", LatencyProfile::free()),
        ("io", LatencyProfile::object_storage()),
    ] {
        let store = build(&runtime, profile);
        let mut group = c.benchmark_group(format!("write/{label}"));
        if label == "io" {
            group.sample_size(20);
        }

        // One row, three indexes: four keys written and three uniqueness reads
        // avoided (none of these indexes is unique).
        group.bench_function("insert_one", |b| {
            b.to_async(&runtime).iter(|| async {
                let id = next.fetch_add(1, Ordering::Relaxed);
                let txn = store.begin().await.expect("begin");
                txn.insert(&root(), &table, &row(0, id))
                    .await
                    .expect("insert");
                txn.commit().await.expect("commit")
            });
        });

        // Amortising the commit over a batch is the obvious lever for bulk
        // loads; this says how much there is to gain.
        for batch in [10u64, 100] {
            group.bench_with_input(
                BenchmarkId::new("insert_batch", batch),
                &batch,
                |b, &batch| {
                    let store = &store;
                    let table = &table;
                    let next = &next;
                    b.to_async(&runtime).iter(move || async move {
                        let txn = store.begin().await.expect("begin");
                        for _ in 0..batch {
                            let id = next.fetch_add(1, Ordering::Relaxed);
                            txn.insert(&root(), table, &row(0, id))
                                .await
                                .expect("insert");
                        }
                        txn.commit().await.expect("commit")
                    });
                },
            );
        }

        group.finish();
    }
}

criterion_group!(benches, planning, scan_row_breakdown, reads, writes);
/// Spelled out rather than `criterion_main!(benches)`, which is what this
/// was: the generated main runs the group and prints criterion's summary,
/// with nowhere to say what build produced the numbers. A bench whose output
/// cannot be told apart from a debug run's is the defect #280 is about.
fn main() {
    slate_kernel::build::announce();
    benches();
    Criterion::default().configure_from_args().final_summary();
}

/// Where the time in a scanned row actually goes.
///
/// A table scan is now the plan the cost model picks most often, so its
/// per-row cost is the thing worth breaking down: iterator overhead, decoding
/// the key, decoding the body, and evaluating the predicate.
fn scan_row_breakdown(c: &mut Criterion) {
    use slate_kernel::keys;
    use slate_schema::{decode_row, encode_body};

    let table = events();
    let row = row(0, 1234);
    let key = keys::row_key(&table, &row.primary_key_values(&table));
    let body = encode_body(&table, &row);
    let primary_key = row.primary_key_values(&table);

    // A predicate that a scan would have to evaluate on every row.
    let predicate = Expr::eq(column("kind"), Value::Str("kind-7".into())).and(Expr::compare(
        column("at"),
        CmpOp::Ge,
        Value::I64(100),
    ));

    let mut group = c.benchmark_group("scan_row");
    group.bench_function("decode_key", |b| {
        b.iter(|| keys::decode_row_key(black_box(&table), black_box(&key)).expect("key"));
    });
    group.bench_function("decode_body", |b| {
        b.iter(|| {
            decode_row(black_box(&table), black_box(&primary_key), black_box(&body)).expect("row")
        });
    });
    group.bench_function("evaluate_predicate", |b| {
        b.iter(|| black_box(&predicate).admits(black_box(&row)));
    });
    group.finish();
}
