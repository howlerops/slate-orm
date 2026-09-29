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

criterion_group!(
    benches,
    planning,
    scan_row_breakdown,
    reads,
    writes,
    soft_delete
);
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

/// What a soft-deleting table costs a read that wants only the live rows.
///
/// `ledger/2026-09-19-a-row-that-is-gone-but-still-there.md` named two costs
/// and measured neither: the extra `IsNull` conjunct every read of such a
/// table carries, and the retired rows that stay in every index on a table
/// with no partial index and are filtered *after* the fetch. They are separate
/// costs with separate shapes — one is per row scanned, the other is per row
/// fetched and discarded — so they are separated here rather than reported as
/// one number.
///
/// Three tables, identical in column layout, row encoding and index set. The
/// only differences are the `soft_delete` declaration and how many rows carry
/// a stamp:
///
/// - `plain` declares `deleted_at` as an ordinary nullable column. It is the
///   control, and it exists so the comparison is against a table of the same
///   *width* — comparing against `events()`, which has one column fewer, would
///   fold a decode difference into the answer and read as the conjunct's cost.
/// - `live` declares it as the soft-delete column with every row live. Against
///   `plain`, the difference is the conjunct and nothing else.
/// - `half` is the same declaration with half the rows retired. Against
///   `live`, the difference is what the retired rows cost — and the row count
///   returned differs, which is why the per-row figure, not the total, is the
///   one the entry quotes.
fn soft_delete(c: &mut Criterion) {
    let runtime = Runtime::new().expect("runtime");

    // The control on its own: `live` is benched in the loop below, beside the
    // retired case it is the baseline for. Benching it here too gave criterion
    // a duplicate id, which it silently renamed to `live #2` rather than
    // refusing — and the two readings of the same configuration differed by
    // 11%, which is the noise floor quoted in the entry. Kept as one `plain`
    // rather than restructured away, because that accident is the only
    // same-configuration spread this measurement has.
    {
        let table = stamped(false);
        let store = stamped_store(&runtime, &table, 0);
        announce_rows(&runtime, &store, &table, "plain", false);
        let mut group = c.benchmark_group("soft_delete");
        group.bench_with_input(BenchmarkId::new("tenant_scan", "plain"), &(), |b, ()| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.query(
                    &root(),
                    &table,
                    Expr::eq(stamped_column("tenant_id"), tenant_value(0)),
                    ScanOrder::Ascending,
                )
                .await
                .expect("query")
                .count()
                .await
                .expect("count")
            });
        });
        group.finish();
    }

    // And the second cost, which needs retired rows to exist at all. Both
    // access paths, because they pay for a retired row differently: a scan has
    // already decoded the row when it tests the conjunct, so a retired row
    // costs one predicate evaluation and one row not emitted, while an index
    // scan tests the conjunct only *after* a point lookup the index entry made
    // it do. The caveat's second sentence is about the second shape, and a
    // scan-only measurement would have answered the wrong question.
    let table = stamped(true);
    let live = stamped_store(&runtime, &table, 0);
    let half = stamped_store(&runtime, &table, ROWS_PER_TENANT / 2);
    let mut group = c.benchmark_group("soft_delete");
    for (label, store) in [("live", &live), ("half_retired", &half)] {
        announce_rows(&runtime, store, &table, label, false);
        announce_rows(&runtime, store, &table, label, true);
        group.bench_with_input(BenchmarkId::new("tenant_scan", label), &label, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.query(
                    &root(),
                    &table,
                    Expr::eq(stamped_column("tenant_id"), tenant_value(0)),
                    ScanOrder::Ascending,
                )
                .await
                .expect("query")
                .count()
                .await
                .expect("count")
            });
        });
        // `by_kind` is not partial, so it still carries an entry for every
        // retired row. 2,500 rows over 25 kinds is ~100 entries walked and
        // ~100 rows fetched either way; with half retired, ~50 of those
        // fetches are thrown away.
        group.bench_with_input(BenchmarkId::new("index_scan", label), &label, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let txn = store.begin().await.expect("begin");
                txn.query(
                    &root(),
                    &table,
                    Expr::eq(stamped_column("tenant_id"), tenant_value(0)).and(Expr::eq(
                        stamped_column("kind"),
                        Value::Str("kind-7".into()),
                    )),
                    ScanOrder::Ascending,
                )
                .await
                .expect("query")
                .count()
                .await
                .expect("count")
            });
        });
    }
    group.finish();
}

/// Run one of the two queries once and print how many rows it answered.
///
/// The per-*useful*-row figures are the whole finding, and dividing a measured
/// time by a row count worked out on paper is how a wrong denominator gets
/// published as a ratio. 100 and 50 were what I expected here; they are
/// printed because expecting them is not observing them.
fn announce_rows(runtime: &Runtime, store: &Store, table: &TableDef, label: &str, indexed: bool) {
    let predicate = if indexed {
        Expr::eq(stamped_column("tenant_id"), tenant_value(0)).and(Expr::eq(
            stamped_column("kind"),
            Value::Str("kind-7".into()),
        ))
    } else {
        Expr::eq(stamped_column("tenant_id"), tenant_value(0))
    };
    let rows = runtime.block_on(async {
        let txn = store.begin().await.expect("begin");
        txn.query(&root(), table, predicate, ScanOrder::Ascending)
            .await
            .expect("query")
            .count()
            .await
            .expect("count")
    });
    let path = if indexed { "index_scan" } else { "tenant_scan" };
    println!("rows: soft_delete/{path}/{label} answers {rows}");
}

/// `events()` plus a nullable `deleted_at`, declared as the soft-delete column
/// or not.
///
/// One function rather than two so the two tables cannot drift in any way
/// except the one under measurement — which is the whole design of the
/// comparison, and the mistake a copy-pasted second builder invites.
fn stamped(declared: bool) -> TableDef {
    let builder = TableDef::builder("stamped", TableId(2))
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("actor", ValueType::Str)
        .column("at", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .nullable_column("deleted_at", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .index(IndexDef::builder("by_actor", IndexId(11)).column("actor"))
        .index(IndexDef::builder("by_at_desc", IndexId(12)).column_with("at", Direction::Desc));
    let builder = if declared {
        builder.soft_delete("deleted_at")
    } else {
        builder
    };
    builder.build().expect("valid schema")
}

fn stamped_column(name: &str) -> Ordinal {
    stamped(false).ordinal_of(name).expect("column exists")
}

/// The same fixture as [`build`], over [`stamped`], with `retired` rows per
/// tenant carrying a stamp.
///
/// The stamped rows are the *low* ids rather than every other one, so that
/// `half_retired`'s surviving rows are a contiguous key range. Interleaving
/// them would measure a scan that skips every other key, which is a different
/// (and cache-hostile) shape, and the entry's claim is about how many rows are
/// discarded rather than about where they sit.
fn stamped_store(runtime: &Runtime, table: &TableDef, retired: u64) -> Store {
    let catalog = Catalog::from_tables([table.clone()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("bench", TableId(2), Action::ALL));
    let backing = MemoryStore::new();
    let loader = RecordStore::new(backing.clone(), catalog.clone(), security.clone());
    let root = SecurityContext::superuser();

    runtime.block_on(async {
        for tenant in 0..TENANTS {
            let txn = loader.begin().await.expect("begin");
            for id in 0..ROWS_PER_TENANT {
                let mut values = row(tenant, id).into_values();
                values.push(Value::Null);
                txn.insert(&root, table, &Row::new(values))
                    .await
                    .expect("insert");
            }
            txn.commit().await.expect("commit");
        }
        // Retired through `delete`, not by writing the stamp: `insert` refuses
        // a row that supplies the soft-delete column at all
        // (`SoftDeleteColumnSupplied`), which is the rule that stops an
        // application forging a deletion time. Seeding around it would have
        // measured a fixture the kernel cannot produce. Found by trying.
        for tenant in 0..TENANTS {
            let txn = loader.begin().await.expect("begin");
            for id in 0..retired {
                txn.delete(&root, table, &[tenant_value(tenant), Value::U64(id)])
                    .await
                    .expect("delete");
            }
            txn.commit().await.expect("commit");
        }
    });

    RecordStore::new(
        LatencyStore::new(backing, LatencyProfile::free()),
        catalog,
        security,
    )
}
