//! What the gRPC head node costs, measured.
//!
//! ```sh
//! cargo run --release -p slate-headbench --example head_report
//! cargo run --release -p slate-headbench --example head_report -- stream lease
//! ```
//!
//! Five sections, each answering a question nobody in this project could
//! answer before:
//!
//! | section | question |
//! |---|---|
//! | `rpc` | what does the same operation cost over gRPC rather than in process? |
//! | `stream` | what does the stream batch size buy, and what does it cost? |
//! | `commit` | autocommit or an explicit transaction? |
//! | `routing` | what do routing and the freshness wait cost under a replica fleet? |
//! | `lease` | what does a renewal cost, and is a fifteen-second term sensible? |
//!
//! Everything is a median over repeated runs with the range printed beside it.
//! Where two measurements' ranges overlap, the difference between them is
//! printed as noise and is not a finding.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use slate_headbench::counting::CountingStore;
use slate_headbench::fixture::{catalog, context, events, principal_request, row, security};
use slate_headbench::harness::{Backend, InProcess, Serving, leading, memory, serve};
use slate_headbench::stats::{Measure, difference, duration};
use slate_kernel::{
    Expr, Freshness, KvReadStore, KvStore, Query, ReadToken, RecordStore, ScanOrder,
};
use slate_schema::{Row, TableDef};
use slate_server::convert::{query_to_proto, row_to_proto};
use slate_server::leadership::{Cadence, Leadership, maintain};
use slate_server::lease::{Lease, ObjectStoreLease};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_server::{Head, Limits};
use slate_tuple::Value;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tonic::transport::Channel;

/// The tenant every request in this report belongs to.
const TENANT: u64 = 1;

/// Independent runs per measurement. Odd by default, so the median is an
/// observed run rather than the average of two.
///
/// Raise it with `HEADBENCH_RUNS` when the machine is busy: the median moves
/// with contention, the minimum much less, and more runs makes the minimum a
/// better estimate of what the code costs when nothing else is on the core.
fn runs() -> usize {
    std::env::var("HEADBENCH_RUNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(7)
}

/// Ids handed out to writes, so no measurement ever collides with another's
/// primary key and measures a duplicate-key refusal instead of an insert.
static NEXT_ID: AtomicU64 = AtomicU64::new(1_000_000);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// One warm-up pass, then [`runs()`] timed passes of `ops` operations each.
///
/// The warm-up matters more here than in a CPU benchmark: the first request on
/// a fresh HTTP/2 connection pays for the flow-control window opening, and the
/// first write on a fresh SlateDB pays for a memtable that does not exist yet.
/// Charging either to run one would show up as a spread and be reported as
/// instability the harness itself caused.
macro_rules! measure {
    ($label:expr, $ops:expr, $body:expr) => {{
        let ops: usize = $ops;
        for _ in 0..ops.min(8) {
            let _ = $body;
        }
        let mut measure = Measure::new($label, ops);
        for _ in 0..runs() {
            let started = Instant::now();
            for _ in 0..ops {
                let _ = $body;
            }
            measure.run(started.elapsed());
        }
        measure
    }};
}

#[tokio::main]
async fn main() {
    let requested: Vec<String> = std::env::args().skip(1).collect();
    let wanted = |name: &str| requested.is_empty() || requested.iter().any(|s| s == name);

    println!("# slate-server head node: baseline");
    println!();
    println!("Backend: SlateDB over an in-memory object store, in this process.");
    println!("Transport: HTTP/2 over loopback TCP, one channel held open.");
    println!(
        "Runs: {} per measurement, `median [min – max]`, ± is the range",
        runs()
    );
    println!("over the median.");
    println!();

    if wanted("rpc") {
        section_rpc().await;
    }
    if wanted("stream") {
        section_stream().await;
    }
    if wanted("commit") {
        section_commit().await;
    }
    if wanted("routing") {
        section_routing().await;
    }
    if wanted("lease") {
        section_lease().await;
    }
}

fn heading(title: &str) {
    println!("\n\n## {title}");
    println!("{:-<86}", "");
}

fn show(measure: &Measure) {
    println!("{}", measure.line());
}

// --- wire helpers ---------------------------------------------------------

fn wire_key(tenant: u64, id: u64) -> pb::Row {
    pb::Row {
        computed: Vec::new(),
        values: vec![
            slate_server::convert::value_to_proto(&Value::U64(tenant)),
            slate_server::convert::value_to_proto(&Value::U64(id)),
        ],
    }
}

fn wire_freshness(freshness: Freshness) -> Option<pb::Freshness> {
    use pb::freshness::Level;
    Some(pb::Freshness {
        level: Some(match freshness {
            Freshness::Any => Level::Any(0),
            Freshness::AtLeast(token) => Level::AtLeast(token.sequence()),
            Freshness::Latest => Level::Latest(0),
        }),
    })
}

/// Drain a query stream, returning the rows and the view that served it.
async fn drain(
    stream: &mut tonic::Streaming<pb::QueryResponse>,
) -> (usize, usize, Option<pb::ServedBy>) {
    let mut rows = 0;
    let mut messages = 0;
    let mut served_by = None;
    while let Some(message) = stream.message().await.expect("a query message") {
        messages += 1;
        if served_by.is_none() {
            served_by = message.served_by.clone();
        }
        rows += message.rows.len();
    }
    (rows, messages, served_by)
}

/// Load `count` rows into `tenant`, in batches, without measuring it.
async fn seed<S: KvStore + KvReadStore>(
    store: &RecordStore<Arc<S>>,
    table: &TableDef,
    tenant: u64,
    from: u64,
    count: u64,
) {
    let context = context(tenant);
    for chunk_start in (0..count).step_by(500) {
        let rows: Vec<Row> = (chunk_start..(chunk_start + 500).min(count))
            .map(|i| row(tenant, from + i))
            .collect();
        let transaction = store.begin().await.expect("begin");
        transaction
            .insert_many(&context, table, &rows)
            .await
            .expect("seed insert");
        transaction.commit().await.expect("seed commit");
    }
}

// --- 1. per-RPC overhead --------------------------------------------------

/// The same operation over gRPC and against the kernel directly.
///
/// The in-process side is deliberately *not* a shortcut through the head node:
/// it is the pool and the record store the head node itself holds, called the
/// way the handler calls them, with no proto type constructed anywhere. If it
/// were still serialising, the difference below would be near zero and the
/// whole section would be worthless — so the `explain` pair is included as the
/// check. `explain` reads no rows at all; over gRPC it still has to convert a
/// query in and an explanation out, and the gap it shows is conversion plus
/// transport with the storage term removed.
async fn section_rpc() {
    heading("1. Per-RPC overhead: gRPC against the same call in process");

    println!("\nBacked by SlateDB (in-memory object store), writes at `Durability::Visible`");
    println!("so a 100 ms WAL flush timer does not bury the thing being measured.\n");
    let backend = Backend::open().await;
    let slate = backend.visible();
    over_one_backend("slatedb", slate).await;

    println!("\nBacked by the kernel's `MemoryStore`. Not a backend anyone deploys — it is");
    println!("here so the same difference is measured with the storage term driven to");
    println!("nearly nothing. If the two agree, the difference is the head node.\n");
    over_one_backend("memory", memory()).await;
}

async fn over_one_backend<S: KvStore + KvReadStore + 'static>(label: &str, writer: Arc<S>) {
    let table = events();
    let ctx = context(TENANT);
    let id_column = table.ordinal_of("id").expect("id column");

    let head: Head<S> = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&writer),
        Vec::new(),
        leading().await,
        Limits::default(),
    );
    let serving = serve(head).await;
    let mut client = serving.client().await;
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());

    seed(&inproc.writer, &table, TENANT, 1, 2_000).await;

    // --- sanity: both sides see the same row ------------------------------
    let probe_key = vec![Value::U64(TENANT), Value::U64(7)];
    let (view, store) = inproc
        .pool
        .snapshot_from(Freshness::Any, Some(&Value::U64(TENANT)))
        .await
        .expect("route");
    let direct = view.get(&ctx, &table, &probe_key).await.expect("get");
    let served = store.replica_name().to_owned();
    drop(view);
    let wired = client
        .get(principal_request(
            pb::GetRequest {
                schema: None,
                transaction: String::new(),
                table: "events".to_owned(),
                primary_key: Some(wire_key(TENANT, 7)),
                freshness: wire_freshness(Freshness::Any),
            },
            TENANT,
        ))
        .await
        .expect("get over gRPC")
        .into_inner();
    assert!(direct.is_some(), "the fixture row is there in process");
    assert!(wired.found, "the fixture row is there over gRPC");
    println!(
        "sanity [{label}]: both paths found the row; the in-process read was served by \
         `{served}`,\n        the wire read by `{}` — the same store, reached two ways.",
        wired.served_by.as_ref().map_or("?", |s| s.replica.as_str())
    );
    println!();

    // --- transport floor ---------------------------------------------------
    //
    // Warm the channel properly first. The eight-iteration warm-up inside
    // `measure!` is not enough for HTTP/2: an earlier version of this section
    // measured the empty RPC first and an insert last, and reported the insert
    // as *cheaper than an empty call* — which cannot be true and was the
    // connection still settling. The floor is therefore measured again at the
    // end of the section, and the two are compared: if they disagree, the
    // ordering is still contaminating everything between them.
    warm(&mut client).await;

    let floor = measure!("Leadership RPC (no storage, no auth)", 200, {
        client
            .leadership(tonic::Request::new(pb::LeadershipRequest {}))
            .await
            .expect("leadership")
    });
    show(&floor);

    // --- get ---------------------------------------------------------------
    let grpc_get = measure!("get by primary key, over gRPC", 200, {
        client
            .get(principal_request(
                pb::GetRequest {
                    schema: None,
                    transaction: String::new(),
                    table: "events".to_owned(),
                    primary_key: Some(wire_key(TENANT, 7)),
                    freshness: wire_freshness(Freshness::Any),
                },
                TENANT,
            ))
            .await
            .expect("get")
    });
    let direct_get = measure!("get by primary key, in process", 200, {
        let (view, _) = inproc
            .pool
            .snapshot_from(Freshness::Any, Some(&Value::U64(TENANT)))
            .await
            .expect("route");
        view.get(&ctx, &table, &probe_key).await.expect("get")
    });
    show(&grpc_get);
    show(&direct_get);
    println!(
        "{:<46} {}",
        "  → head node's share of a get",
        difference(&grpc_get, &direct_get)
    );
    // The comparison that survives a busy machine. `Leadership` does no
    // storage work, no authentication and no conversion, so a `get` costing
    // barely more than it says the head node's own work is not where the time
    // goes — and unlike the ratio above, both sides of this one are paying the
    // same transport, so a load spike moves them together.
    println!(
        "{:<46} {}",
        "  → its own work, above an empty RPC",
        difference(&grpc_get, &floor)
    );

    // --- explain: the storage term removed ---------------------------------
    let query = Query::all()
        .filter(Expr::eq(id_column, Value::U64(7)))
        .order(ScanOrder::Ascending);
    let wire = query_to_proto(&table, &query);
    let grpc_explain = measure!("explain, over gRPC", 200, {
        client
            .explain(principal_request(
                pb::ExplainRequest {
                    transaction: String::new(),
                    query: Some(wire.clone()),
                    freshness: wire_freshness(Freshness::Any),
                },
                TENANT,
            ))
            .await
            .expect("explain")
    });
    let direct_explain = measure!("explain, in process", 200, {
        let (view, _) = inproc
            .pool
            .snapshot_from(Freshness::Any, Some(&Value::U64(TENANT)))
            .await
            .expect("route");
        view.explain(&ctx, &table, &query).expect("explain")
    });
    show(&grpc_explain);
    show(&direct_explain);
    println!(
        "{:<46} {}",
        "  → head node's share of an explain",
        difference(&grpc_explain, &direct_explain)
    );

    // --- a one-row query, which is a stream --------------------------------
    let grpc_query = measure!("query returning 1 row, over gRPC", 200, {
        let mut stream = client
            .query(principal_request(
                pb::QueryRequest {
                    transaction: String::new(),
                    query: Some(wire.clone()),
                    freshness: wire_freshness(Freshness::Any),
                },
                TENANT,
            ))
            .await
            .expect("query")
            .into_inner();
        drain(&mut stream).await
    });
    let direct_query = measure!("query returning 1 row, in process", 200, {
        let (view, _) = inproc
            .pool
            .snapshot_from(Freshness::Any, Some(&Value::U64(TENANT)))
            .await
            .expect("route");
        view.execute(&ctx, &table, &query)
            .await
            .expect("execute")
            .collect()
            .await
            .expect("collect")
    });
    show(&grpc_query);
    show(&direct_query);
    println!(
        "{:<46} {}",
        "  → head node's share of a 1-row query",
        difference(&grpc_query, &direct_query)
    );

    // --- an autocommitted single-row insert --------------------------------
    let grpc_insert = measure!("insert 1 row, autocommit, over gRPC", 100, {
        client
            .insert(principal_request(
                pb::InsertRequest {
                    schema: None,
                    transaction: String::new(),
                    table: "events".to_owned(),
                    rows: vec![row_to_proto(&row(TENANT, next_id()))],
                    upsert: false,
                },
                TENANT,
            ))
            .await
            .expect("insert")
    });
    let direct_insert = measure!("insert 1 row, autocommit, in process", 100, {
        let rows = [row(TENANT, next_id())];
        inproc
            .writer
            .transact_tracked(async |txn| txn.insert_many(&ctx, &table, &rows).await)
            .await
            .expect("insert")
    });
    show(&grpc_insert);
    show(&direct_insert);
    println!(
        "{:<46} {}",
        "  → head node's share of an insert",
        difference(&grpc_insert, &direct_insert)
    );

    // The check on the ordering. Same call, same channel, after everything
    // above has run.
    let floor_again = measure!("Leadership RPC, measured again at the end", 200, {
        client
            .leadership(tonic::Request::new(pb::LeadershipRequest {}))
            .await
            .expect("leadership")
    });
    show(&floor_again);
    println!(
        "{:<46} {}",
        "  → drift in the floor across the section",
        difference(&floor, &floor_again)
    );
}

/// Push enough traffic through a fresh channel that it has finished settling.
///
/// HTTP/2 opens with a small flow-control window and grows it; the first few
/// hundred requests on a connection are not the same request as the ten
/// thousandth. Two thousand is where the cost stopped moving in this harness.
async fn warm(client: &mut RecordsClient<Channel>) {
    for _ in 0..2_000 {
        let _ = client
            .leadership(tonic::Request::new(pb::LeadershipRequest {}))
            .await
            .expect("leadership");
    }
}

// --- 2. stream batch size -------------------------------------------------

/// Throughput and first-row latency as the stream batch size moves.
///
/// `Limits::rows_per_message` is 256 and the comment beside it says framing
/// costs per message and latency costs per batch, and that the number was not
/// measured. Both halves of that trade are measured here: total time to drain
/// a whole table, and time until the first row arrives.
async fn section_stream() {
    heading("2. Stream throughput against the batch size");

    const ROWS: u64 = 20_000;
    let backend = Backend::open().await;
    let writer = backend.visible();
    let table = events();
    let ctx = context(TENANT);

    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    println!("\nLoading {ROWS} rows…");
    let load = Instant::now();
    seed(&inproc.writer, &table, TENANT, 1, ROWS).await;
    println!("loaded in {:?}\n", load.elapsed());

    let query = Query::all().filter(Expr::True).order(ScanOrder::Ascending);
    let wire = query_to_proto(&table, &query);

    // The floor: the same scan with no head node in front of it.
    let direct = measure!("whole table, in process (no gRPC)", 3, {
        let (view, _) = inproc
            .pool
            .snapshot_from(Freshness::Any, Some(&Value::U64(TENANT)))
            .await
            .expect("route");
        let mut cursor = view.execute(&ctx, &table, &query).await.expect("execute");
        let mut seen = 0usize;
        while cursor.next().await.expect("row").is_some() {
            seen += 1;
        }
        assert_eq!(seen as u64, ROWS);
        seen
    });
    show(&direct);
    println!(
        "  → {:.0} rows/s in process\n",
        ROWS as f64 * 1e9 / direct.median()
    );

    println!(
        "{:<8} {:>9} {:>12} {:>12} {:>12} {:>11} {:>12}",
        "batch", "messages", "drain med.", "drain best", "rows/s best", "first row", "first best"
    );
    println!("{:-<86}", "");

    // The sizes either side of 128 are not padding. An earlier run of this
    // sweep showed first-row latency stepping by milliseconds between 64 and
    // 128 and then barely moving to 256, which is the shape of a threshold
    // rather than of work — and 128 is exactly tokio's cooperative-scheduling
    // budget, the number of resource operations a task may perform before it
    // is made to yield. If the step is sharp and sits between 127 and 129, the
    // batch size is interacting with the runtime's scheduler and not only with
    // framing. If it wanders, it was load.
    for batch in batch_sizes() {
        let head = slate_headbench::harness::head(
            catalog(),
            security(),
            Arc::clone(&writer),
            Vec::new(),
            leading().await,
            Limits {
                rows_per_message: batch,
                ..Limits::default()
            },
        );
        let serving = serve(head).await;
        let mut client = serving.client().await;
        warm(&mut client).await;

        let mut messages_seen = 0usize;
        let mut drain_measure = Measure::new(format!("batch {batch}"), 1);
        let mut first_row = Measure::new(format!("batch {batch} first row"), 1);

        for run in 0..runs() + 1 {
            let started = Instant::now();
            let mut stream = client
                .query(principal_request(
                    pb::QueryRequest {
                        transaction: String::new(),
                        query: Some(wire.clone()),
                        freshness: wire_freshness(Freshness::Any),
                    },
                    TENANT,
                ))
                .await
                .expect("query")
                .into_inner();

            let mut rows = 0usize;
            let mut messages = 0usize;
            let mut first = None;
            while let Some(message) = stream.message().await.expect("message") {
                messages += 1;
                if first.is_none() && !message.rows.is_empty() {
                    first = Some(started.elapsed());
                }
                rows += message.rows.len();
            }
            let elapsed = started.elapsed();
            assert_eq!(rows as u64, ROWS, "every batch size returns every row");
            // Run zero is the warm-up.
            if run > 0 {
                messages_seen = messages;
                drain_measure.run(elapsed);
                first_row.run(first.unwrap_or(elapsed));
            }
        }

        // The head sends one header message with no rows, then one message per
        // batch. Checking that rather than trusting it: a batch size that
        // silently did not take effect would otherwise look like a null result.
        let expected = 1 + (ROWS as usize).div_ceil(batch);
        assert_eq!(
            messages_seen, expected,
            "batch {batch} should send {expected} messages"
        );

        // Both the median and the best run are printed. On a machine sharing
        // its cores with a compiler the median moves with whatever else is
        // running; the best run is the one that got a core to itself, and it
        // is the only column here that can be compared between batch sizes
        // without arguing about what the load was at the time.
        println!(
            "{:<8} {:>9} {:>12} {:>12} {:>11.0} {:>11} {:>12}",
            batch,
            messages_seen,
            duration(drain_measure.median()),
            duration(drain_measure.min()),
            ROWS as f64 * 1e9 / drain_measure.min(),
            duration(first_row.median()),
            duration(first_row.min()),
        );
    }
}

// --- 3. autocommit against an explicit transaction ------------------------

async fn section_commit() {
    heading("3. Autocommit against an explicit Begin/Commit");

    let backend = Backend::open().await;

    for (mode, writer) in [
        ("Durability::Visible", backend.visible()),
        ("Durability::Durable", backend.durable()),
    ] {
        println!("\n### {mode}\n");
        let head = slate_headbench::harness::head(
            catalog(),
            security(),
            Arc::clone(&writer),
            Vec::new(),
            leading().await,
            Limits::default(),
        );
        let serving = serve(head).await;
        let mut client = serving.client().await;
        warm(&mut client).await;
        // One durable commit takes up to a WAL flush interval, so the durable
        // arm gets fewer operations per run. Fewer, not none: the whole point
        // is that this is the cost an explicit transaction amortises.
        let ops = if mode.ends_with("Durable") { 12 } else { 100 };

        let autocommit = measure!("1 row, autocommit (1 RPC)", ops, {
            client
                .insert(principal_request(
                    pb::InsertRequest {
                        schema: None,
                        transaction: String::new(),
                        table: "events".to_owned(),
                        rows: vec![row_to_proto(&row(TENANT, next_id()))],
                        upsert: false,
                    },
                    TENANT,
                ))
                .await
                .expect("insert")
        });
        let explicit = measure!("1 row, begin + insert + commit (3 RPCs)", ops, {
            one_row_in_a_transaction(&mut client).await
        });
        show(&autocommit);
        show(&explicit);
        println!(
            "{:<46} {}",
            "  → cost of wrapping one write in a txn",
            difference(&explicit, &autocommit)
        );

        // --- a hundred rows, three ways ------------------------------------
        let batch_runs = if mode.ends_with("Durable") { 3 } else { runs() };
        let mut one_call = Measure::new("100 rows, one autocommit call", 100);
        let mut per_row = Measure::new("100 rows, txn + 100 insert calls", 100);
        let mut one_in_txn = Measure::new("100 rows, txn + 1 insert call", 100);

        for run in 0..batch_runs + 1 {
            let rows: Vec<pb::Row> = (0..100)
                .map(|_| row_to_proto(&row(TENANT, next_id())))
                .collect();
            let started = Instant::now();
            client
                .insert(principal_request(
                    pb::InsertRequest {
                        schema: None,
                        transaction: String::new(),
                        table: "events".to_owned(),
                        rows: rows.clone(),
                        upsert: false,
                    },
                    TENANT,
                ))
                .await
                .expect("insert");
            let elapsed = started.elapsed();
            if run > 0 {
                one_call.run(elapsed);
            }

            let rows: Vec<pb::Row> = (0..100)
                .map(|_| row_to_proto(&row(TENANT, next_id())))
                .collect();
            let started = Instant::now();
            let handle = begin(&mut client).await;
            for wire_row in &rows {
                client
                    .insert(principal_request(
                        pb::InsertRequest {
                            schema: None,
                            transaction: handle.clone(),
                            table: "events".to_owned(),
                            rows: vec![wire_row.clone()],
                            upsert: false,
                        },
                        TENANT,
                    ))
                    .await
                    .expect("insert");
            }
            commit(&mut client, &handle).await;
            let elapsed = started.elapsed();
            if run > 0 {
                per_row.run(elapsed);
            }

            let rows: Vec<pb::Row> = (0..100)
                .map(|_| row_to_proto(&row(TENANT, next_id())))
                .collect();
            let started = Instant::now();
            let handle = begin(&mut client).await;
            client
                .insert(principal_request(
                    pb::InsertRequest {
                        schema: None,
                        transaction: handle.clone(),
                        table: "events".to_owned(),
                        rows,
                        upsert: false,
                    },
                    TENANT,
                ))
                .await
                .expect("insert");
            commit(&mut client, &handle).await;
            let elapsed = started.elapsed();
            if run > 0 {
                one_in_txn.run(elapsed);
            }
        }
        println!();
        show(&one_call);
        show(&per_row);
        show(&one_in_txn);
    }
}

async fn begin(client: &mut RecordsClient<Channel>) -> String {
    client
        .begin(principal_request(pb::BeginRequest {}, TENANT))
        .await
        .expect("begin")
        .into_inner()
        .transaction
}

async fn commit(client: &mut RecordsClient<Channel>, handle: &str) -> Option<u64> {
    client
        .commit(principal_request(
            pb::CommitRequest {
                transaction: handle.to_owned(),
            },
            TENANT,
        ))
        .await
        .expect("commit")
        .into_inner()
        .sequence
}

async fn one_row_in_a_transaction(client: &mut RecordsClient<Channel>) {
    let handle = begin(client).await;
    client
        .insert(principal_request(
            pb::InsertRequest {
                schema: None,
                transaction: handle.clone(),
                table: "events".to_owned(),
                rows: vec![row_to_proto(&row(TENANT, next_id()))],
                upsert: false,
            },
            TENANT,
        ))
        .await
        .expect("insert");
    commit(client, &handle).await;
}

// --- 4. read routing under a replica fleet --------------------------------

async fn section_routing() {
    heading("4. Read routing, and what the freshness wait costs");

    // 50 ms by default, which is what the table in `docs/performance.md` was
    // taken at. Overridable because the number this section reports is set by
    // this interval and nothing else — and because `slate-serverd` does not
    // set it at all, so a deployment gets `DbReaderOptions::default()`, which
    // is **10 seconds**. `HEADBENCH_POLL_MS=10000` measures what that does.
    let poll = Duration::from_millis(
        std::env::var("HEADBENCH_POLL_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(50),
    );
    let backend = Backend::open().await;
    let durable = backend.durable();
    let replicas = backend.replicas(3, poll).await;
    let table = events();
    let ctx = context(TENANT);

    let inproc = InProcess::new(
        catalog(),
        security(),
        Arc::clone(&durable),
        replicas.clone(),
    );
    seed(&inproc.writer, &table, TENANT, 1, 1_000).await;

    let head = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&durable),
        replicas.clone(),
        leading().await,
        Limits::default(),
    );
    let serving = serve(head).await;
    let mut client = serving.client().await;
    warm(&mut client).await;

    // Let every replica reach the seed before anything is timed, so the first
    // "already caught up" measurement is not secretly a catch-up.
    let seeded = durable.visible_sequence().unwrap_or(0);
    for replica in &replicas {
        let _ = replica
            .wait_for_sequence(seeded, Duration::from_secs(30))
            .await;
    }

    println!("\nThree following replicas, manifest poll {poll:?}; writer at sequence {seeded}.\n");

    // --- sanity: the reads really do go where we think --------------------
    let any = grpc_get(&mut client, 7, Freshness::Any).await;
    let latest = grpc_get(&mut client, 7, Freshness::Latest).await;
    let proven = grpc_get(&mut client, 7, Freshness::AtLeast(ReadToken::new(seeded))).await;
    println!(
        "sanity: Freshness::Any served by `{}`, Freshness::Latest by `{}`,\n        \
         AtLeast({seeded}) by `{}` — all three found the row.",
        any.1, latest.1, proven.1
    );
    assert_ne!(any.1, "writer", "an `Any` read should reach a replica");
    assert_eq!(latest.1, "writer", "a `Latest` read must reach the writer");
    println!();

    // --- the routing decision on its own ----------------------------------
    let route_affinity = measure!("pool.route, tenant affinity (in process)", 20_000, {
        inproc
            .pool
            .route(Freshness::Any, Some(&Value::U64(TENANT)))
            .await
            .expect("route")
    });
    let route_rr = measure!("pool.route, round robin (in process)", 20_000, {
        inproc
            .pool
            .route(Freshness::Any, None)
            .await
            .expect("route")
    });
    show(&route_affinity);
    show(&route_rr);
    println!(
        "{:<46} {}",
        "  → rendezvous hashing against round robin",
        difference(&route_affinity, &route_rr)
    );
    println!();

    // --- reads at each freshness ------------------------------------------
    let g_any = measure!("get, Freshness::Any (a replica)", 200, {
        grpc_get(&mut client, 7, Freshness::Any).await
    });
    let g_latest = measure!("get, Freshness::Latest (the writer)", 200, {
        grpc_get(&mut client, 7, Freshness::Latest).await
    });
    let token = ReadToken::new(seeded);
    let g_caught_up = measure!("get, AtLeast(a sequence already reached)", 200, {
        grpc_get(&mut client, 7, Freshness::AtLeast(token)).await
    });
    show(&g_any);
    show(&g_latest);
    show(&g_caught_up);
    println!(
        "{:<46} {}",
        "  → what proving freshness costs when free",
        difference(&g_caught_up, &g_any)
    );
    println!(
        "{:<46} {}",
        "  → replica against writer",
        difference(&g_any, &g_latest)
    );
    println!();

    // --- the same read without the head node -------------------------------
    //
    // Measured here, before the catch-up loop below writes anything: a replica
    // that has taken another hundred and seventy-six commits is a different
    // replica to read from, and comparing a control taken after that against a
    // gRPC number taken before it is comparing two workloads.
    let direct_any = measure!("get, Freshness::Any, in process", 200, {
        let (view, _) = inproc
            .pool
            .snapshot_from(Freshness::Any, Some(&Value::U64(TENANT)))
            .await
            .expect("route");
        view.get(&ctx, &table, &[Value::U64(TENANT), Value::U64(7)])
            .await
            .expect("get")
    });
    show(&direct_any);
    println!(
        "{:<46} {}",
        "  → head node's share of a replica read",
        difference(&g_any, &direct_any)
    );
    println!();

    // --- the case the replica has to catch up ------------------------------
    //
    // Each operation: commit durably (untimed), then read demanding that
    // commit's sequence (timed). No replica can have it yet, so the pool waits
    // on whichever is closest. This is the only measurement here whose value is
    // set by a configured interval rather than by code, which is the finding.
    let mut catch_up = Measure::new("get, AtLeast(a sequence just committed)", 1);
    let mut fell_back = 0usize;
    for run in 0..runs() + 1 {
        let mut total = Duration::ZERO;
        let ops: u32 = 8;
        for _ in 0..ops {
            let rows = [row(TENANT, next_id())];
            let (_, token) = inproc
                .writer
                .transact_tracked(async |txn| txn.insert_many(&ctx, &table, &rows).await)
                .await
                .expect("write");
            let sequence = token.map_or(0, ReadToken::sequence);
            let started = Instant::now();
            let (_, served) =
                grpc_get(&mut client, 7, Freshness::AtLeast(ReadToken::new(sequence))).await;
            total += started.elapsed();
            if served == "writer" {
                fell_back += 1;
            }
        }
        if run > 0 {
            catch_up.run_each(total / ops);
        }
    }
    show(&catch_up);
    println!(
        "{:<46} {}",
        "  → the freshness wait",
        difference(&catch_up, &g_caught_up)
    );
    println!(
        "  fell back to the writer {fell_back} times out of {} (a replica that cannot",
        (runs() + 1) * 8
    );
    println!("  catch up inside `RoutingPolicy::catch_up` sends the read to the writer).");

    // The same read, over gRPC, after all that writing. A replica with more
    // to consult is a slower replica, and the first version of this section
    // measured the in-process control down here — after the catch-up loop had
    // committed a hundred and seventy-six times — while its gRPC counterpart
    // was measured before it. That made the control the *slower* of the two,
    // and the head node's share came out negative once the transport floor was
    // subtracted. The control has moved above; this is what is left of the
    // ordering, measured rather than assumed away.
    let g_any_after = measure!("get, Freshness::Any, after the writing", 200, {
        grpc_get(&mut client, 7, Freshness::Any).await
    });
    println!();
    show(&g_any_after);
    println!(
        "{:<46} {}",
        "  → what 176 commits did to a replica read",
        difference(&g_any_after, &g_any)
    );
}

/// The batch sizes to sweep, overridable with `HEADBENCH_BATCHES`.
///
/// Overridable because locating a step needs a different set from measuring
/// throughput, and a rebuild between the two is a rebuild during which the
/// machine is busy and the next measurement is worse.
fn batch_sizes() -> Vec<usize> {
    match std::env::var("HEADBENCH_BATCHES") {
        Ok(list) => list
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .filter(|n| *n > 0)
            .collect(),
        Err(_) => vec![
            1, 8, 32, 64, 96, 112, 127, 128, 129, 160, 256, 512, 1024, 4096, 16_384,
        ],
    }
}

/// A lease on `store` at `path`, held under `holder`, with a term of `term`.
fn lease_at(
    store: &Arc<dyn ObjectStore>,
    path: &str,
    holder: &str,
    term: Duration,
) -> ObjectStoreLease {
    ObjectStoreLease::with_holder(Arc::clone(store), path, holder.to_owned()).with_term_length(term)
}

/// A `get` over the wire, returning whether the row was found and who served it.
async fn grpc_get(
    client: &mut RecordsClient<Channel>,
    id: u64,
    freshness: Freshness,
) -> (bool, String) {
    let response = client
        .get(principal_request(
            pb::GetRequest {
                schema: None,
                transaction: String::new(),
                table: "events".to_owned(),
                primary_key: Some(wire_key(TENANT, id)),
                freshness: wire_freshness(freshness),
            },
            TENANT,
        ))
        .await
        .expect("get")
        .into_inner();
    let served = response
        .served_by
        .as_ref()
        .map_or_else(String::new, |s| s.replica.clone());
    // Every id this report reads was seeded before the read. A miss would mean
    // a view was serving a snapshot older than it claimed, which would make
    // every latency below a measurement of reading nothing.
    assert!(response.found, "row {id} should be visible to `{served}`");
    (response.found, served)
}

// --- 5. the lease ---------------------------------------------------------

async fn section_lease() {
    heading("5. What a lease renewal costs, and whether 15 s is sensible");

    let inner: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let counting = Arc::new(CountingStore::new(inner));
    let counts = counting.counts();
    let store: Arc<dyn ObjectStore> = counting;

    let lease =
        ObjectStoreLease::with_holder(Arc::clone(&store), "leases/writer", "bench-node".to_owned());
    println!(
        "\nTerm length: {:?} (the crate default).\n",
        lease.term_length()
    );

    counts.reset();
    let acquired = lease.acquire().await.expect("acquire an unheld lease");
    println!(
        "cold acquire: generation {}, {} GET + {} PUT ({} conditional)",
        acquired.generation,
        counts.gets(),
        counts.puts(),
        counts.conditional_puts()
    );

    counts.reset();
    lease.renew().await.expect("renew");
    println!(
        "one renewal:  {} GET + {} PUT ({} conditional)",
        counts.gets(),
        counts.puts(),
        counts.conditional_puts()
    );
    println!(
        "\nThose counts are the number that travels. The clock below is against an\n\
         in-memory object store in this process, so it is the compare-and-set, the\n\
         encode/decode and the mutex — everything except the network, which is all of\n\
         the cost in a bucket.\n"
    );

    let renew = measure!("lease.renew (in-memory object store)", 200, {
        lease.renew().await.expect("renew")
    });
    let observe = measure!("lease.observe (a read, no write)", 200, {
        lease.observe().await.expect("observe")
    });
    show(&renew);
    show(&observe);

    let cadence = Cadence::for_term(lease.term_length());
    println!(
        "\nCadence for that term: renew every {:?}, campaign every {:?}.",
        cadence.renew_every, cadence.campaign_every
    );
    println!(
        "Measured renewal is {} against a {:?} gap: {:.6}% of the interval.",
        duration(renew.median()),
        cadence.renew_every,
        renew.median() * 100.0 / cadence.renew_every.as_nanos() as f64
    );

    // --- what the term actually buys, and what it costs --------------------
    //
    // The renewal above is free, so the term is not chosen to pay for
    // renewals. What it buys is tolerance of a renewal that does not arrive;
    // what it costs is how long a *crashed* leader keeps its successor out,
    // because a process that dies cannot release. Nothing in this project has
    // measured the second half.
    //
    // Measured at short terms, three times each, rather than at fifteen
    // seconds: the mechanism is an expiry compared against a clock, so the
    // relationship is linear by construction, and running it once at 15 s
    // would cost 45 seconds to learn what three short terms already show. The
    // extension to 15 s is arithmetic and is labelled as such below.
    println!("\ntakeover, after a holder stops renewing without releasing:\n");
    println!(
        "{:<12} {:>16} {:>18} {:>16}",
        "term", "crash (tight poll)", "crash (real cadence)", "graceful release"
    );
    println!("{:-<66}", "");
    for term in [
        Duration::from_millis(300),
        Duration::from_millis(600),
        Duration::from_millis(1200),
    ] {
        let mut crashed = Measure::new("crash", 1);
        let mut cadenced = Measure::new("crash at cadence", 1);
        let mut graceful = Measure::new("graceful", 1);
        for attempt in 0..3u32 {
            let path = format!("leases/takeover-{}-{attempt}", term.as_millis());
            // Tight poll: what the expiry itself costs, with the successor
            // asking as often as it likes.
            let held = lease_at(&store, &path, "holder", term);
            held.acquire().await.expect("hold");
            let successor = lease_at(&store, &path, "successor", term);
            let started = Instant::now();
            while successor.acquire().await.is_err() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            crashed.run_each(started.elapsed());

            // The same, with the successor campaigning at the cadence the
            // crate derives from the term rather than as fast as it can.
            let path = format!("{path}-cadence");
            let held = lease_at(&store, &path, "holder", term);
            held.acquire().await.expect("hold");
            let successor = lease_at(&store, &path, "successor", term);
            let gap = Cadence::for_term(term).campaign_every;
            let started = Instant::now();
            while successor.acquire().await.is_err() {
                tokio::time::sleep(gap).await;
            }
            cadenced.run_each(started.elapsed());

            // And the path the head node takes on a clean shutdown.
            let path = format!("{path}-release");
            let held = lease_at(&store, &path, "holder", term);
            held.acquire().await.expect("hold");
            let successor = lease_at(&store, &path, "successor", term);
            held.release().await.expect("release");
            let started = Instant::now();
            successor.acquire().await.expect("acquire a released lease");
            graceful.run_each(started.elapsed());
        }
        println!(
            "{:<12} {:>16} {:>18} {:>16}",
            format!("{:?}", term),
            duration(crashed.median()),
            duration(cadenced.median()),
            duration(graceful.median()),
        );
    }
    println!(
        "\nA crashed holder keeps its successor out for the rest of its term, and the\n\
         successor's own campaign interval (term/3) is added on top. Nothing about\n\
         that is specific to the term chosen, so at the default 15 s it is up to 15 s\n\
         plus up to 5 s — that last sentence is arithmetic from the mechanism above,\n\
         not a measurement. A holder that releases hands over in the time of one\n\
         conditional write."
    );

    // --- the check on the write path ---------------------------------------
    let leadership = leading().await;
    let check = measure!("Leadership::is_leader (the write-path check)", 100_000, {
        leadership.is_leader()
    });
    println!();
    show(&check);

    // --- does renewing interfere with serving? -----------------------------
    //
    // At the real cadence a renewal happens once every five seconds, which no
    // benchmark of this length can see. So the renewal rate is raised by two
    // and a half orders of magnitude — a 60 ms term renews every 20 ms — and
    // the question becomes whether request latency moves at a rate the
    // deployment will never reach. This can only detect gross interference:
    // even at this rate a renewal lands between roughly one request in fifty,
    // so it is a bound rather than a null.
    println!(
        "\nInterference, at 250x the real renewal rate (a 60 ms term, renewing every\n\
         20 ms) so that a rate the deployment never reaches is the one under test:\n"
    );
    let backend = Backend::open().await;
    let writer = backend.visible();
    let table = events();
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    seed(&inproc.writer, &table, TENANT, 1, 500).await;

    let quiet_head = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&writer),
        Vec::new(),
        leading().await,
        Limits::default(),
    );
    let quiet = serve(quiet_head).await;
    let mut quiet_client = quiet.client().await;
    warm(&mut quiet_client).await;
    let idle = measure!("get, no renewal running", 200, {
        grpc_get(&mut quiet_client, 7, Freshness::Any).await
    });

    let busy_lease: Arc<dyn Lease> = Arc::new(
        ObjectStoreLease::with_holder(Arc::clone(&store), "leases/busy", "busy-node".to_owned())
            .with_term_length(Duration::from_millis(60)),
    );
    let busy_leadership = Leadership::new(busy_lease);
    assert!(busy_leadership.campaign().await, "take the busy lease");
    let maintainer = tokio::spawn(maintain(
        Arc::clone(&busy_leadership),
        Cadence::for_term(Duration::from_millis(60)),
    ));
    let busy_head = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&writer),
        Vec::new(),
        busy_leadership,
        Limits::default(),
    );
    let busy: Serving = serve(busy_head).await;
    let mut busy_client = busy.client().await;
    warm(&mut busy_client).await;
    let renewing = measure!("get, renewing every 20 ms", 200, {
        grpc_get(&mut busy_client, 7, Freshness::Any).await
    });
    maintainer.abort();

    show(&idle);
    show(&renewing);
    println!(
        "{:<46} {}",
        "  → renewal's effect on a served read",
        difference(&renewing, &idle)
    );
}
