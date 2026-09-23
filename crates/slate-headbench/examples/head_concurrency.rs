//! What the head node does when more than one caller is talking to it.
//!
//! ```sh
//! cargo run --release -p slate-headbench --example head_concurrency
//! cargo run --release -p slate-headbench --example head_concurrency -- scale
//! cargo run --release -p slate-headbench --example head_concurrency -- limit slow lease
//! ```
//!
//! Every number in `head_report` is one request at a time. This one is the
//! other side: throughput and latency percentiles as the number of concurrent
//! clients rises, what happens at and past `max_transactions`, whether a slow
//! or abandoned stream consumer costs anybody else anything, and whether a
//! lease renewal shows up in the tail.
//!
//! # Two runtimes, and why
//!
//! The server and the load generator get **separate tokio runtimes with
//! different thread names**, sharing the same four cores through the OS
//! scheduler. Not to isolate them — they still compete, which is the honest
//! arrangement for a benchmark whose client is on the same box — but so that
//! `/proc/self/task/*/schedstat` can say *which side* spent the CPU. Without
//! that, "the box is the bottleneck" is a hunch. With it, it is a column.
//!
//! The cost of the arrangement is that it is not the arrangement `head_report`
//! measured under, so [`section_scale`] re-measures a single-client `get` and
//! prints it next to the figure `head_report` records. If the two disagree the
//! whole section is comparing against a different baseline and says so.
//!
//! # What a reader has to keep in mind
//!
//! - The load is **closed loop**. Each client waits for its reply before
//!   sending again, so offered load falls as the server slows. Latency
//!   percentiles are therefore honest and throughput is a *completion* rate,
//!   not a saturation curve.
//! - The object store is in memory, as everywhere else in this crate. A
//!   durable commit still waits for SlateDB's real WAL flush timer, which is
//!   the one storage cost that survives.
//! - Other agents build on this machine while it runs. Every row is a median
//!   over repeated runs with the range printed, and the control at concurrency
//!   1 is re-measured at the end of the sweep so drift is visible rather than
//!   assumed away.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use slate_headbench::fixture::{catalog, context, events, principal_request, row, security};
use slate_headbench::harness::{Backend, InProcess, leading, serve, serve_with_nagle};
use slate_headbench::load::{Repeated, cpu_between, cpu_now, drive, tcp_ext};
use slate_headbench::stats::duration;
use slate_kernel::{Expr, KvReadStore, KvStore, Query, RecordStore, ScanOrder};
use slate_schema::{Row, TableDef};
use slate_server::convert::{query_to_proto, row_to_proto};
use slate_server::leadership::{Cadence, Leadership};
use slate_server::lease::{Lease, LeaseError, ObjectStoreLease, Term};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_server::{Head, Limits};
use slate_slatedb::SlateStore;
use slate_tuple::Value;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tonic::transport::Channel;

/// The tenant every request belongs to.
const TENANT: u64 = 1;

/// Rows seeded into the read fixture.
const SEEDED: u64 = 20_000;

/// Ids handed to writes, so no two measurements collide on a primary key.
static NEXT_ID: AtomicU64 = AtomicU64::new(5_000_000);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn env_usize(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(fallback)
}

/// Timed windows per point. Three is enough to see a spread and cheap enough
/// to sweep eight concurrency levels across five workloads.
fn runs() -> usize {
    env_usize("HEADBENCH_RUNS", 3)
}

/// How long one timed window lasts.
fn window() -> Duration {
    Duration::from_millis(env_usize("HEADBENCH_WINDOW_MS", 1_200) as u64)
}

/// The concurrency levels swept.
fn levels() -> Vec<usize> {
    match std::env::var("HEADBENCH_LEVELS") {
        Ok(list) => list
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .filter(|n| *n > 0)
            .collect(),
        Err(_) => vec![1, 2, 4, 8, 16, 32, 64, 128],
    }
}

// --- runtimes --------------------------------------------------------------

/// The two runtimes, named so their CPU can be told apart.
struct Runtimes {
    head: Runtime,
    load: Runtime,
}

fn runtimes() -> Runtimes {
    let head = tokio::runtime::Builder::new_multi_thread()
        .thread_name("hd-worker")
        .enable_all()
        .build()
        .expect("build the head runtime");
    let load = tokio::runtime::Builder::new_multi_thread()
        .thread_name("ld-worker")
        .enable_all()
        .build()
        .expect("build the load runtime");
    Runtimes { head, load }
}

fn main() {
    slate_slatedb::announce();
    let requested: Vec<String> = std::env::args().skip(1).collect();
    let wanted = |name: &str| requested.is_empty() || requested.iter().any(|s| s == name);

    let rt = runtimes();

    println!("# slate-server head node: under concurrency");
    println!();
    println!("Backend: SlateDB over an in-memory object store, in this process.");
    println!("Transport: HTTP/2 over loopback TCP, one connection per client.");
    println!("Runtimes: server threads `hd-worker`, load threads `ld-worker`, sharing");
    println!("          {} cores through the OS scheduler.", cores());
    println!(
        "Windows: {} runs of {:?} per point; load is closed loop.",
        runs(),
        window()
    );
    println!();

    if wanted("nagle") {
        section_nagle(&rt);
    }
    if wanted("scale") {
        section_scale(&rt);
    }
    if wanted("limit") {
        section_limit(&rt);
    }
    if wanted("slow") {
        section_slow(&rt);
    }
    if wanted("lease") {
        section_lease(&rt);
    }
}

fn cores() -> usize {
    std::thread::available_parallelism().map_or(0, std::num::NonZero::get)
}

fn heading(title: &str) {
    println!("\n\n## {title}");
    println!("{:-<100}", "");
}

// --- shared helpers --------------------------------------------------------

fn wire_key(tenant: u64, id: u64) -> pb::Row {
    pb::Row {
        computed: Vec::new(),
        windowed: Vec::new(),
        values: vec![
            slate_server::convert::value_to_proto(&Value::U64(tenant)),
            slate_server::convert::value_to_proto(&Value::U64(id)),
        ],
    }
}

fn any_freshness() -> Option<pb::Freshness> {
    Some(pb::Freshness {
        level: Some(pb::freshness::Level::Any(0)),
    })
}

/// Load `count` rows without measuring it.
async fn seed<S: KvStore + KvReadStore>(
    store: &RecordStore<Arc<S>>,
    table: &TableDef,
    tenant: u64,
    from: u64,
    count: u64,
) {
    let ctx = context(tenant);
    for chunk in (0..count).step_by(1_000) {
        let rows: Vec<Row> = (chunk..(chunk + 1_000).min(count))
            .map(|i| row(tenant, from + i))
            .collect();
        let transaction = store.begin().await.expect("begin");
        transaction
            .insert_many(&ctx, table, &rows)
            .await
            .expect("seed insert");
        transaction.commit().await.expect("seed commit");
    }
}

/// Open `count` connections and push enough through each that HTTP/2 has
/// finished growing its flow-control window.
///
/// One channel per client, not one channel cloned: sixty-four clients sharing
/// one connection is a different question (HTTP/2 multiplexing) from
/// sixty-four clients, and this benchmark is about the second.
async fn connect_many(address: std::net::SocketAddr, count: usize) -> Vec<RecordsClient<Channel>> {
    let mut clients = Vec::with_capacity(count);
    for _ in 0..count {
        clients.push(
            RecordsClient::connect(format!("http://{address}"))
                .await
                .expect("connect to the head node"),
        );
    }
    let warmed = clients.clone();
    let handles: Vec<_> = warmed
        .into_iter()
        .map(|mut client| {
            tokio::spawn(async move {
                for _ in 0..2_000 {
                    let _ = client
                        .leadership(principal_request(pb::LeadershipRequest {}, TENANT))
                        .await
                        .expect("leadership");
                }
            })
        })
        .collect();
    for handle in handles {
        let _ = handle.await;
    }
    clients
}

fn column(value: f64) -> String {
    duration(value)
}

/// One row of the sweep table.
fn print_point(level: usize, repeated: &Repeated) {
    let (throughput, low, high) = repeated.throughput();
    println!(
        "{:>7} {:>10.0} {:>16} {:>10} {:>10} {:>10} {:>10} {:>7.2} {:>7.2} {:>7.2} {:>9} {:>6}",
        level,
        throughput,
        format!("[{low:.0} – {high:.0}]"),
        column(repeated.p50().0),
        column(repeated.p90().0),
        column(repeated.p99().0),
        column(repeated.max().0),
        repeated.cores(),
        repeated.cores_of("hd-"),
        repeated.cores_of("ld-"),
        column(cpu_per_op(repeated)),
        repeated.errors(),
    );
}

/// CPU spent per operation, in nanoseconds, across both sides.
///
/// The column to read on a box that is not this benchmark's alone. Throughput
/// is `cores the process was given / this`, and only the first of those two is
/// decided by whatever else is compiling at the time. A throughput that stops
/// rising while this stays flat is a box that ran out of cores; a `cpu/op` that
/// climbs with the client count is contention inside the thing under test.
fn cpu_per_op(repeated: &Repeated) -> f64 {
    let (throughput, ..) = repeated.throughput();
    if throughput <= 0.0 {
        return f64::NAN;
    }
    repeated.cores() * 1e9 / throughput
}

/// Every thread-name group that spent CPU in the last run of a measurement.
fn print_cpu_groups(repeated: &Repeated) {
    let Some(run) = repeated.runs.last() else {
        return;
    };
    let wall = run.cpu.wall.as_secs_f64();
    let mut groups: Vec<(&String, &f64)> = run.cpu.by_group.iter().collect();
    groups.sort_by(|a, b| b.1.total_cmp(a.1));
    let named: String = groups
        .iter()
        .filter(|(_, seconds)| **seconds / wall >= 0.005)
        .map(|(name, seconds)| format!("{name} {:.2}", **seconds / wall))
        .collect::<Vec<_>>()
        .join(", ");
    println!("  cpu by thread group (cores, last run): {named}");
}

fn print_header() {
    println!(
        "{:>7} {:>10} {:>16} {:>10} {:>10} {:>10} {:>10} {:>7} {:>7} {:>7} {:>9} {:>6}",
        "clients",
        "ops/s",
        "[min – max]",
        "p50",
        "p90",
        "p99",
        "worst",
        "cores",
        "srv",
        "cli",
        "cpu/op",
        "errs"
    );
    println!("{:-<118}", "");
}

// --- 0. the socket option this harness was not setting ---------------------

/// A fourth harness bug, and the one that had to be found before anything else
/// in this file meant anything.
///
/// `tonic::transport::Server` sets `TCP_NODELAY` by default. Its
/// `serve_with_incoming` — which a harness that binds its own port must call —
/// documents that the setting "is ignored when using this method". So every
/// number this crate has recorded was taken against a server socket with
/// **Nagle's algorithm on**, and every real deployment runs with it off.
///
/// Nagle holds a small write until the previous one is acknowledged, and Linux
/// delays an acknowledgement by up to 40 ms. A unary reply is a single write
/// and never notices. A **stream** is at least two — the header message that
/// names the replica, then the first batch of rows — and when the second is not
/// ready in the same poll as the first, it waits for an ACK that is not coming
/// for tens of milliseconds.
///
/// Both arms are measured here rather than the fix being applied quietly,
/// because the size of the difference is the evidence that this is what it is.
fn section_nagle(rt: &Runtimes) {
    heading("0. TCP_NODELAY, which this harness was not setting");

    let backend = rt.head.block_on(Backend::open());
    let writer = backend.visible();
    let table = events();
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    rt.head
        .block_on(seed(&inproc.writer, &table, TENANT, 1, 2_000));

    let id_column = table.ordinal_of("id").expect("id column");
    // Two query shapes, because the prediction distinguishes them. A query the
    // planner answers with a Point Get has its row ready in the same poll as
    // the header, so both messages leave in one write and Nagle cannot bite. A
    // scan has to go and fetch, so the two messages are two writes.
    let point_shaped = Query::all()
        .filter(Expr::eq(id_column, Value::U64(7)))
        .order(ScanOrder::Ascending);
    let scan_shaped = Query::all().order(ScanOrder::Ascending).limit(10);
    let wire_point = Arc::new(query_to_proto(&table, &point_shaped));
    let wire_scan = Arc::new(query_to_proto(&table, &scan_shaped));

    println!(
        "\n{:>36} {:>10} {:>10} {:>10} {:>10} {:>10} {:>9}",
        "", "ops/s", "p50", "p90", "p99", "worst", "dACKs/op"
    );
    println!("{:-<102}", "");

    for nagle in [true, false] {
        let head: Head<SlateStore> = slate_headbench::harness::head(
            catalog(),
            security(),
            Arc::clone(&writer),
            Vec::new(),
            rt.head.block_on(leading()),
            Limits::default(),
        );
        let serving = rt.head.block_on(serve_with_nagle(head, nagle));
        let clients = rt.load.block_on(connect_many(serving.address, 4));
        let label = if nagle { "Nagle on " } else { "TCP_NODELAY" };

        let (unary, unary_acks) = with_delayed_acks(|| {
            measure_here(rt, &clients, 1, |mut client, sequence| async move {
                let id = (sequence % 2_000) + 1;
                let ok = client
                    .get(principal_request(
                        pb::GetRequest {
                            schema: None,
                            transaction: String::new(),
                            table: "events".to_owned(),
                            primary_key: Some(wire_key(TENANT, id)),
                            freshness: any_freshness(),
                        },
                        TENANT,
                    ))
                    .await
                    .is_ok_and(|reply| reply.into_inner().found);
                (client, ok)
            })
        });
        report_row_with_acks(&format!("{label}: unary get"), &unary, unary_acks);

        for (shape, wire, expected) in [
            (
                "1-row query (Point Get plan)",
                Arc::clone(&wire_point),
                1usize,
            ),
            ("10-row query (scan plan)", Arc::clone(&wire_scan), 10usize),
        ] {
            let (repeated, acks) = with_delayed_acks(|| {
                measure_here(rt, &clients, 1, move |mut client, _| {
                    let wire = Arc::clone(&wire);
                    async move {
                        let Ok(stream) = client
                            .query(principal_request(
                                pb::QueryRequest {
                                    transaction: String::new(),
                                    query: Some((*wire).clone()),
                                    freshness: any_freshness(),
                                },
                                TENANT,
                            ))
                            .await
                        else {
                            return (client, false);
                        };
                        let mut stream = stream.into_inner();
                        let mut rows = 0usize;
                        while let Ok(Some(message)) = stream.message().await {
                            rows += message.rows.len();
                        }
                        (client, rows == expected)
                    }
                })
            });
            report_row_with_acks(&format!("{label}: {shape}"), &repeated, acks);
        }
        drop(serving);
    }
    println!(
        "\nEvery other measurement in this crate, and every stream number already in\n\
         `docs/performance.md`, was taken in the first of those two states."
    );
}

#[allow(dead_code)]
fn report_row(label: &str, repeated: &Repeated) {
    println!(
        "{:>36} {:>10.0} {:>10} {:>10} {:>10} {:>10}",
        label,
        repeated.throughput().0,
        column(repeated.p50().0),
        column(repeated.p90().0),
        column(repeated.p99().0),
        column(repeated.max().0),
    );
}

/// The same, with the kernel's delayed-acknowledgement counter beside it.
fn report_row_with_acks(label: &str, repeated: &Repeated, delayed_acks: u64) {
    let ops = repeated.ops().max(1);
    println!(
        "{:>36} {:>10.0} {:>10} {:>10} {:>10} {:>10} {:>9.2}",
        label,
        repeated.throughput().0,
        column(repeated.p50().0),
        column(repeated.p90().0),
        column(repeated.p99().0),
        column(repeated.max().0),
        delayed_acks as f64 / ops as f64,
    );
}

/// Run `measure` while watching `TcpExt: DelayedACKs`.
fn with_delayed_acks<T>(measure: impl FnOnce() -> T) -> (T, u64) {
    let before = tcp_ext("DelayedACKs");
    let value = measure();
    (value, tcp_ext("DelayedACKs").saturating_sub(before))
}

/// [`sweep_point`] without the sweep: one concurrency level, [`runs()`] windows.
fn measure_here<F, Fut>(
    rt: &Runtimes,
    clients: &[RecordsClient<Channel>],
    level: usize,
    op: F,
) -> Repeated
where
    F: Fn(RecordsClient<Channel>, u64) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = (RecordsClient<Channel>, bool)> + Send + 'static,
{
    sweep_point(rt, clients, level, op)
}

// --- 1. throughput and latency against concurrency -------------------------

fn section_scale(rt: &Runtimes) {
    heading("1. Throughput and latency as the client count rises");

    let backend = rt.head.block_on(Backend::open());
    let writer = backend.visible();
    let table = events();

    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    println!("\nSeeding {SEEDED} rows…");
    let load_started = Instant::now();
    rt.head
        .block_on(seed(&inproc.writer, &table, TENANT, 1, SEEDED));
    println!("seeded in {:?}\n", load_started.elapsed());

    let head: Head<SlateStore> = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&writer),
        Vec::new(),
        rt.head.block_on(leading()),
        Limits::default(),
    );
    let serving = rt.head.block_on(serve(head));
    let address = serving.address;

    let sweep = levels();
    let widest = sweep.iter().copied().max().unwrap_or(1);
    println!("Opening {widest} connections and warming each…");
    let clients = rt.load.block_on(connect_many(address, widest));
    println!("connected.\n");

    // --- the floor: an RPC that touches no storage and does not authenticate
    println!("### Empty RPC (`Leadership`) — the transport-and-scheduler ceiling\n");
    println!(
        "Nothing below this line can go faster than this. Where *this* stops rising,\n\
         the box is full and no head-node number past it means anything.\n"
    );
    print_header();
    let mut floor_points: Vec<(usize, Repeated)> = Vec::new();
    for &level in &sweep {
        let repeated = sweep_point(rt, &clients, level, |mut client, _| async move {
            let ok = client
                .leadership(principal_request(pb::LeadershipRequest {}, TENANT))
                .await
                .is_ok();
            (client, ok)
        });
        print_point(level, &repeated);
        if level == sweep.last().copied().unwrap_or(0) {
            // The attribution, printed once and only once. `srv` and `cli`
            // above are sums over thread names, and a thread pool this harness
            // did not name — SlateDB's own, say — would be counted in `cores`
            // and in neither column. Printing the groups is how that is caught
            // rather than assumed away.
            print_cpu_groups(&repeated);
        }
        floor_points.push((level, repeated));
    }

    // --- point reads -------------------------------------------------------
    println!("\n### Point read (`Get` by primary key)\n");
    print_header();
    let mut read_points: Vec<(usize, Repeated)> = Vec::new();
    for &level in &sweep {
        let repeated = sweep_point(
            rt,
            &clients,
            level,
            move |mut client, sequence| async move {
                let id = (sequence % SEEDED) + 1;
                let response = client
                    .get(principal_request(
                        pb::GetRequest {
                            schema: None,
                            transaction: String::new(),
                            table: "events".to_owned(),
                            primary_key: Some(wire_key(TENANT, id)),
                            freshness: any_freshness(),
                        },
                        TENANT,
                    ))
                    .await;
                // A read that found nothing is a read of nothing, and would make
                // every latency below a measurement of an empty answer.
                let ok = response.is_ok_and(|reply| reply.into_inner().found);
                (client, ok)
            },
        );
        print_point(level, &repeated);
        read_points.push((level, repeated));
    }

    // --- a small streaming query ------------------------------------------
    let id_column = table.ordinal_of("id").expect("id column");
    let one_row = Query::all()
        .filter(Expr::eq(id_column, Value::U64(7)))
        .order(ScanOrder::Ascending);
    let ten_rows = Query::all().order(ScanOrder::Ascending).limit(10);
    let wire_ten = Arc::new(query_to_proto(&table, &ten_rows));
    let _ = one_row;

    println!("\n### Streaming query, 10 rows (task + oneshot + `mpsc::channel(2)`)\n");
    print_header();
    let mut stream_points: Vec<(usize, Repeated)> = Vec::new();
    for &level in &sweep {
        let wire = Arc::clone(&wire_ten);
        let repeated = sweep_point(rt, &clients, level, move |mut client, _| {
            let wire = Arc::clone(&wire);
            async move {
                let Ok(stream) = client
                    .query(principal_request(
                        pb::QueryRequest {
                            transaction: String::new(),
                            query: Some((*wire).clone()),
                            freshness: any_freshness(),
                        },
                        TENANT,
                    ))
                    .await
                else {
                    return (client, false);
                };
                let mut stream = stream.into_inner();
                let mut rows = 0usize;
                while let Ok(Some(message)) = stream.message().await {
                    rows += message.rows.len();
                }
                (client, rows == 10)
            }
        });
        print_point(level, &repeated);
        stream_points.push((level, repeated));
    }

    // --- a write, with the flush timer out of the way ----------------------
    println!("\n### Write: 1-row autocommit insert, `Durability::Visible`\n");
    print_header();
    let mut write_points: Vec<(usize, Repeated)> = Vec::new();
    for &level in &sweep {
        let repeated = sweep_point(rt, &clients, level, |mut client, _| async move {
            let response = client
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
                .await;
            let ok = response.is_ok_and(|reply| reply.into_inner().affected == 1);
            (client, ok)
        });
        print_point(level, &repeated);
        write_points.push((level, repeated));
    }

    // --- the control, re-measured -----------------------------------------
    //
    // `head_report` measures a single-client `get` over the same backend and
    // records 132 µs. This section runs the server on a different runtime from
    // the client, which is a different arrangement; if the two disagree, the
    // comparison between this section and that one is invalid and the reader
    // needs to know before reading anything above.
    println!("\n### The control, re-measured at the end\n");
    let redone = sweep_point(rt, &clients, 1, move |mut client, sequence| async move {
        let id = (sequence % SEEDED) + 1;
        let ok = client
            .get(principal_request(
                pb::GetRequest {
                    schema: None,
                    transaction: String::new(),
                    table: "events".to_owned(),
                    primary_key: Some(wire_key(TENANT, id)),
                    freshness: any_freshness(),
                },
                TENANT,
            ))
            .await
            .is_ok_and(|reply| reply.into_inner().found);
        (client, ok)
    });
    let first = read_points
        .first()
        .map_or(f64::NAN, |(_, point)| point.p50().0);
    println!(
        "single-client `get` p50: {} at the start of the section, {} at the end\n\
         (`head_report` records 132.4 µs for the same read on one runtime).",
        column(first),
        column(redone.p50().0)
    );

    drop(serving);

    // --- durable writes, which is where queueing has to show up ------------
    section_durable_writes(rt);

    let _ = (floor_points, stream_points, write_points);
}

/// One point of a sweep: `level` clients, [`runs()`] windows, plus an unmeasured
/// warm-up window at that level so the first timed window is not paying for the
/// runtime spinning up `level` tasks.
fn sweep_point<F, Fut>(
    rt: &Runtimes,
    clients: &[RecordsClient<Channel>],
    level: usize,
    op: F,
) -> Repeated
where
    F: Fn(RecordsClient<Channel>, u64) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = (RecordsClient<Channel>, bool)> + Send + 'static,
{
    let pick = |level: usize| -> Vec<RecordsClient<Channel>> {
        (0..level)
            .map(|index| {
                clients
                    .get(index % clients.len())
                    .expect("a connected client")
                    .clone()
            })
            .collect()
    };
    rt.load.block_on(async {
        let _ = drive(pick(level), Duration::from_millis(400), op.clone()).await;
        let mut runs_out = Vec::new();
        for _ in 0..runs() {
            runs_out.push(drive(pick(level), window(), op.clone()).await);
        }
        Repeated::new(runs_out)
    })
}

/// Durable commits under concurrency: do many writers share a flush, or queue
/// behind one another?
///
/// `head_report` measures a durable commit at 101.10 ms, which is SlateDB's
/// 100 ms WAL flush interval. One writer at a time therefore gets ten commits a
/// second no matter what the head node does. The question concurrency asks is
/// whether the eleventh writer gets a share of the same flush — throughput
/// rising with the client count at flat latency — or waits for its own, which
/// would pin throughput at ten a second however many callers there are.
fn section_durable_writes(rt: &Runtimes) {
    println!("\n### Write: 1-row autocommit insert, `Durability::Durable`\n");
    println!(
        "SlateDB's WAL flush interval is 100 ms and `head_report` measures a single\n\
         durable commit at 101.10 ms. If concurrent commits share a flush, throughput\n\
         rises with the client count and latency stays at ~101 ms. If they do not, the\n\
         line is flat at about ten commits a second.\n"
    );

    let backend = rt.head.block_on(Backend::open());
    let durable = backend.durable();
    let table = events();
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&durable), Vec::new());
    rt.head
        .block_on(seed(&inproc.writer, &table, TENANT, 1, 200));

    let head: Head<SlateStore> = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&durable),
        Vec::new(),
        rt.head.block_on(leading()),
        Limits::default(),
    );
    let serving = rt.head.block_on(serve(head));
    let sweep: Vec<usize> = levels().into_iter().filter(|n| *n <= 64).collect();
    let widest = sweep.iter().copied().max().unwrap_or(1);
    let clients = rt.load.block_on(connect_many(serving.address, widest));

    // A durable commit is 101 ms, so a 1.2 s window gives one client twelve
    // samples. The window is stretched here rather than the run count raised:
    // p99 over twelve samples is the worst sample wearing a percentile's name.
    let long = Duration::from_millis(env_usize("HEADBENCH_DURABLE_MS", 4_000) as u64);
    print_header();
    for level in sweep {
        let picked: Vec<RecordsClient<Channel>> = (0..level)
            .map(|index| clients.get(index % clients.len()).expect("client").clone())
            .collect();
        let repeated = rt.load.block_on(async {
            let mut out = Vec::new();
            for _ in 0..2 {
                out.push(
                    drive(picked.clone(), long, |mut client, _| async move {
                        let ok = client
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
                            .is_ok_and(|reply| reply.into_inner().affected == 1);
                        (client, ok)
                    })
                    .await,
                );
            }
            Repeated::new(out)
        });
        print_point(level, &repeated);
    }
    drop(serving);
}

// --- 2. the transaction limit ----------------------------------------------

/// What happens at and past `max_transactions`, and what an open transaction
/// costs while it sits there.
fn section_limit(rt: &Runtimes) {
    heading("2. The transaction limit, and what an open transaction costs");

    let backend = rt.head.block_on(Backend::open());
    let writer = backend.visible();
    let table = events();
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    rt.head
        .block_on(seed(&inproc.writer, &table, TENANT, 1, 2_000));

    // --- past the limit ----------------------------------------------------
    const CAP: usize = 16;
    const CALLERS: usize = 64;
    println!("\n### {CALLERS} clients opening transactions against a node capped at {CAP}\n");
    let head: Head<SlateStore> = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&writer),
        Vec::new(),
        rt.head.block_on(leading()),
        Limits {
            max_transactions: CAP,
            ..Limits::default()
        },
    );
    let serving = rt.head.block_on(serve(head));
    let clients = rt.load.block_on(connect_many(serving.address, CALLERS));

    let accepted = Arc::new(AtomicU64::new(0));
    let refused = Arc::new(AtomicU64::new(0));
    let other = Arc::new(AtomicU64::new(0));
    let refused_ns = Arc::new(AtomicU64::new(0));
    let accepted_ns = Arc::new(AtomicU64::new(0));

    let run = {
        let accepted = Arc::clone(&accepted);
        let refused = Arc::clone(&refused);
        let other = Arc::clone(&other);
        let refused_ns = Arc::clone(&refused_ns);
        let accepted_ns = Arc::clone(&accepted_ns);
        rt.load.block_on(async move {
            drive(clients, window(), move |mut client, _| {
                let accepted = Arc::clone(&accepted);
                let refused = Arc::clone(&refused);
                let other = Arc::clone(&other);
                let refused_ns = Arc::clone(&refused_ns);
                let accepted_ns = Arc::clone(&accepted_ns);
                async move {
                    let at = Instant::now();
                    match client
                        .begin(principal_request(pb::BeginRequest {}, TENANT))
                        .await
                    {
                        Ok(reply) => {
                            accepted_ns
                                .fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
                            accepted.fetch_add(1, Ordering::Relaxed);
                            let handle = reply.into_inner().transaction;
                            let _ = client
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
                                .await;
                            let _ = client
                                .commit(principal_request(
                                    pb::CommitRequest {
                                        transaction: handle,
                                    },
                                    TENANT,
                                ))
                                .await;
                            (client, true)
                        }
                        Err(status) if status.code() == tonic::Code::ResourceExhausted => {
                            refused_ns.fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
                            refused.fetch_add(1, Ordering::Relaxed);
                            (client, true)
                        }
                        Err(_) => {
                            other.fetch_add(1, Ordering::Relaxed);
                            (client, false)
                        }
                    }
                }
            })
            .await
        })
    };

    let accepted_n = accepted.load(Ordering::Relaxed);
    let refused_n = refused.load(Ordering::Relaxed);
    let other_n = other.load(Ordering::Relaxed);
    println!(
        "begin+insert+commit attempts in {:?}: {} accepted, {} refused with\n\
         ResourceExhausted, {} failed some other way.",
        run.wall, accepted_n, refused_n, other_n
    );
    if accepted_n > 0 {
        println!(
            "  mean latency of an accepted Begin: {}",
            column(accepted_ns.load(Ordering::Relaxed) as f64 / accepted_n as f64)
        );
    }
    if refused_n > 0 {
        println!(
            "  mean latency of a refused Begin:   {}",
            column(refused_ns.load(Ordering::Relaxed) as f64 / refused_n as f64)
        );
    }
    println!(
        "  whole-attempt p50 {} p99 {} worst {}",
        column(run.latency.p50),
        column(run.latency.p99),
        column(run.latency.max)
    );
    println!(
        "  {:.2} cores used ({:.2} server, {:.2} client)",
        run.cpu.cores(),
        run.cpu.cores_of("hd-"),
        run.cpu.cores_of("ld-")
    );
    drop(serving);

    // --- what an idle open transaction costs -------------------------------
    println!("\n### A point read with N transactions sitting open\n");
    println!(
        "Each open transaction is a task, an `mpsc::channel(1)` and a pinned snapshot.\n\
         `max_transactions` defaults to 1024, so this is what the default permits.\n"
    );
    let head: Head<SlateStore> = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&writer),
        Vec::new(),
        rt.head.block_on(leading()),
        // A thousand transactions have to *stay* open across the measurement
        // below, which takes longer than the 30 s default idle timeout. A
        // transaction rolled back mid-sweep would make the "open" column a
        // fiction, and the harness would not notice.
        Limits {
            idle_timeout: Duration::from_secs(900),
            ..Limits::default()
        },
    );
    let serving = rt.head.block_on(serve(head));
    let mut clients = rt.load.block_on(connect_many(serving.address, 4));
    let mut holder = rt
        .load
        .block_on(async { RecordsClient::connect(format!("http://{}", serving.address)).await })
        .expect("connect a holder");

    let mut open: Vec<String> = Vec::new();
    println!(
        "{:>10} {:>12} {:>10} {:>10} {:>10} {:>10}",
        "open txns", "ops/s", "p50", "p90", "p99", "worst"
    );
    println!("{:-<66}", "");
    for target in [0usize, 64, 512, 1023] {
        while open.len() < target {
            let handle = rt.load.block_on(async {
                holder
                    .begin(principal_request(pb::BeginRequest {}, TENANT))
                    .await
                    .map(|reply| reply.into_inner().transaction)
            });
            match handle {
                Ok(handle) => open.push(handle),
                Err(status) => {
                    println!("  (could not open transaction {}: {status})", open.len());
                    break;
                }
            }
        }
        let picked: Vec<RecordsClient<Channel>> = clients.iter().take(4).cloned().collect();
        let repeated = rt.load.block_on(async {
            let mut out = Vec::new();
            for _ in 0..runs() {
                out.push(
                    drive(
                        picked.clone(),
                        window(),
                        |mut client, sequence| async move {
                            let id = (sequence % 2_000) + 1;
                            let ok = client
                                .get(principal_request(
                                    pb::GetRequest {
                                        schema: None,
                                        transaction: String::new(),
                                        table: "events".to_owned(),
                                        primary_key: Some(wire_key(TENANT, id)),
                                        freshness: any_freshness(),
                                    },
                                    TENANT,
                                ))
                                .await
                                .is_ok_and(|reply| reply.into_inner().found);
                            (client, ok)
                        },
                    )
                    .await,
                );
            }
            Repeated::new(out)
        });
        println!(
            "{:>10} {:>12.0} {:>10} {:>10} {:>10} {:>10}",
            open.len(),
            repeated.throughput().0,
            column(repeated.p50().0),
            column(repeated.p90().0),
            column(repeated.p99().0),
            column(repeated.max().0),
        );
    }

    // The 1025th. The registry check happens before the task is spawned, so
    // this should be a refusal and not a hang.
    while open.len() < 1024 {
        let handle = rt.load.block_on(async {
            holder
                .begin(principal_request(pb::BeginRequest {}, TENANT))
                .await
                .map(|reply| reply.into_inner().transaction)
        });
        match handle {
            Ok(handle) => open.push(handle),
            Err(_) => break,
        }
    }
    let at = Instant::now();
    let over = rt.load.block_on(async {
        holder
            .begin(principal_request(pb::BeginRequest {}, TENANT))
            .await
    });
    let refusal = at.elapsed();
    match over {
        Ok(_) => println!("\n  the {}th Begin was accepted", open.len() + 1),
        Err(status) => println!(
            "\n  with {} open, the next Begin was refused in {} with {:?}",
            open.len(),
            duration(refusal.as_nanos() as f64),
            status.code()
        ),
    }
    clients.clear();
    drop(serving);
}

// --- 3. slow and abandoned stream consumers --------------------------------

/// Whether a client that stops reading costs anybody else anything.
///
/// Two different questions wearing one name. A **slow** consumer stops draining
/// its stream; `mpsc::channel(2)` means the server's scan task parks after two
/// batches, so the question is whether a parked scan holds anything a
/// non-streaming caller needs. An **abandoned** consumer drops the stream
/// entirely; `Scan::run` returns when its send fails, so the question is
/// whether the scan actually stops — which is answered here in CPU rather than
/// in wall time, because a scan that kept running would spend cores nobody can
/// see from the client side.
fn section_slow(rt: &Runtimes) {
    heading("3. A slow consumer, and an abandoned one");

    let backend = rt.head.block_on(Backend::open());
    let writer = backend.visible();
    let table = events();
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    println!("\nSeeding {SEEDED} rows…");
    rt.head
        .block_on(seed(&inproc.writer, &table, TENANT, 1, SEEDED));

    let head: Head<SlateStore> = slate_headbench::harness::head(
        catalog(),
        security(),
        Arc::clone(&writer),
        Vec::new(),
        rt.head.block_on(leading()),
        Limits::default(),
    );
    let serving = rt.head.block_on(serve(head));
    let clients = rt.load.block_on(connect_many(serving.address, 4));
    let whole_table = Query::all().filter(Expr::True).order(ScanOrder::Ascending);
    let wire_scan = Arc::new(query_to_proto(&table, &whole_table));

    let point_read = |rt: &Runtimes, clients: &[RecordsClient<Channel>]| -> Repeated {
        let picked: Vec<RecordsClient<Channel>> = clients.iter().take(4).cloned().collect();
        rt.load.block_on(async {
            let mut out = Vec::new();
            for _ in 0..runs() {
                out.push(
                    drive(
                        picked.clone(),
                        window(),
                        |mut client, sequence| async move {
                            let id = (sequence % SEEDED) + 1;
                            let ok = client
                                .get(principal_request(
                                    pb::GetRequest {
                                        schema: None,
                                        transaction: String::new(),
                                        table: "events".to_owned(),
                                        primary_key: Some(wire_key(TENANT, id)),
                                        freshness: any_freshness(),
                                    },
                                    TENANT,
                                ))
                                .await
                                .is_ok_and(|reply| reply.into_inner().found);
                            (client, ok)
                        },
                    )
                    .await,
                );
            }
            Repeated::new(out)
        })
    };

    println!(
        "\n{:>26} {:>12} {:>10} {:>10} {:>10} {:>10}",
        "point reads at 4 clients", "ops/s", "p50", "p90", "p99", "worst"
    );
    println!("{:-<84}", "");
    let baseline = point_read(rt, &clients);
    println!(
        "{:>26} {:>12.0} {:>10} {:>10} {:>10} {:>10}",
        "nothing else running",
        baseline.throughput().0,
        column(baseline.p50().0),
        column(baseline.p90().0),
        column(baseline.p99().0),
        column(baseline.max().0),
    );

    // Writes with nothing stalled, for the comparison above to mean anything.
    let picked: Vec<RecordsClient<Channel>> = clients.iter().take(4).cloned().collect();
    let clean_writes = rt.load.block_on(async {
        let mut out = Vec::new();
        for _ in 0..runs() {
            out.push(
                drive(picked.clone(), window(), |mut client, _| async move {
                    let ok = client
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
                        .is_ok_and(|reply| reply.into_inner().affected == 1);
                    (client, ok)
                })
                .await,
            );
        }
        Repeated::new(out)
    });
    println!(
        "{:>26} {:>12.0} {:>10} {:>10} {:>10} {:>10}   (writes)",
        "nothing stalled",
        clean_writes.throughput().0,
        column(clean_writes.p50().0),
        column(clean_writes.p90().0),
        column(clean_writes.p99().0),
        column(clean_writes.max().0),
    );

    for stalled in [8usize, 32usize] {
        // Hold `stalled` streams open, each having read exactly one message and
        // then stopped. The stream stays alive — the task is parked on a full
        // channel, which is the state the design intends and has never been
        // measured in.
        let held = rt.load.block_on({
            let wire = Arc::clone(&wire_scan);
            let address = serving.address;
            async move {
                let mut streams = Vec::new();
                for _ in 0..stalled {
                    let mut client = RecordsClient::connect(format!("http://{address}"))
                        .await
                        .expect("connect a stalled client");
                    let mut stream = client
                        .query(principal_request(
                            pb::QueryRequest {
                                transaction: String::new(),
                                query: Some((*wire).clone()),
                                freshness: any_freshness(),
                            },
                            TENANT,
                        ))
                        .await
                        .expect("open a scan")
                        .into_inner();
                    // Read the header and one batch, then stop reading.
                    let _ = stream.message().await;
                    let _ = stream.message().await;
                    streams.push((client, stream));
                }
                streams
            }
        });
        let with_stalled = point_read(rt, &clients);
        println!(
            "{:>26} {:>12.0} {:>10} {:>10} {:>10} {:>10}",
            format!("{stalled} stalled streams"),
            with_stalled.throughput().0,
            column(with_stalled.p50().0),
            column(with_stalled.p90().0),
            column(with_stalled.p99().0),
            column(with_stalled.max().0),
        );
        // Also: can the writer still write while those snapshots are pinned?
        let picked: Vec<RecordsClient<Channel>> = clients.iter().take(4).cloned().collect();
        let writes = rt.load.block_on(async {
            let mut out = Vec::new();
            for _ in 0..runs() {
                out.push(
                    drive(picked.clone(), window(), |mut client, _| async move {
                        let ok = client
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
                            .is_ok_and(|reply| reply.into_inner().affected == 1);
                        (client, ok)
                    })
                    .await,
                );
            }
            Repeated::new(out)
        });
        println!(
            "{:>26} {:>12.0} {:>10} {:>10} {:>10} {:>10}   (writes)",
            format!("  with {stalled} stalled"),
            writes.throughput().0,
            column(writes.p50().0),
            column(writes.p90().0),
            column(writes.p99().0),
            column(writes.max().0),
        );
        rt.load.block_on(async move { drop(held) });
        // Give the server a moment to notice the hang-ups before the next arm.
        std::thread::sleep(Duration::from_millis(300));
    }

    // --- the abandoned scan, measured in CPU -------------------------------
    println!("\n### Does an abandoned scan stop reading?\n");
    println!(
        "Counted in server CPU over a fixed 600 ms window rather than in wall time:\n\
         a scan that kept running after its client left would spend cores and show\n\
         up nowhere else. Eight whole-table scans of {SEEDED} rows each.\n"
    );
    let idle = server_cpu_over(rt, Duration::from_millis(600), |_| {});
    println!("  {:>34}  {:.3} core-seconds", "nothing running", idle);

    let drained = server_cpu_over(rt, Duration::from_millis(600), |rt| {
        let wire = Arc::clone(&wire_scan);
        let address = serving.address;
        rt.load.block_on(async move {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let wire = Arc::clone(&wire);
                handles.push(tokio::spawn(async move {
                    let mut client = RecordsClient::connect(format!("http://{address}"))
                        .await
                        .expect("connect");
                    let mut stream = client
                        .query(principal_request(
                            pb::QueryRequest {
                                transaction: String::new(),
                                query: Some((*wire).clone()),
                                freshness: any_freshness(),
                            },
                            TENANT,
                        ))
                        .await
                        .expect("scan")
                        .into_inner();
                    let mut rows = 0usize;
                    while let Ok(Some(message)) = stream.message().await {
                        rows += message.rows.len();
                    }
                    rows
                }));
            }
            let mut counts = Vec::new();
            for handle in handles {
                counts.push(handle.await.unwrap_or(0));
            }
            // Not compared against `SEEDED`: the write arms above have grown
            // the table, and asserting a stale constant is how a harness
            // panics on a fixture it changed itself. What has to hold is that
            // all eight saw the same table.
            let first = counts.first().copied().unwrap_or(0);
            assert!(
                first >= SEEDED as usize,
                "a drained scan returned {first} rows"
            );
            assert!(
                counts.iter().all(|count| *count == first),
                "the eight drained scans disagreed: {counts:?}"
            );
            println!("  ({first} rows per scan)");
        });
    });
    println!(
        "  {:>34}  {:.3} core-seconds",
        "8 scans drained to the end", drained
    );

    let abandoned = server_cpu_over(rt, Duration::from_millis(600), |rt| {
        let wire = Arc::clone(&wire_scan);
        let address = serving.address;
        rt.load.block_on(async move {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let wire = Arc::clone(&wire);
                handles.push(tokio::spawn(async move {
                    let mut client = RecordsClient::connect(format!("http://{address}"))
                        .await
                        .expect("connect");
                    let mut stream = client
                        .query(principal_request(
                            pb::QueryRequest {
                                transaction: String::new(),
                                query: Some((*wire).clone()),
                                freshness: any_freshness(),
                            },
                            TENANT,
                        ))
                        .await
                        .expect("scan")
                        .into_inner();
                    // One header, one batch, then drop everything.
                    let _ = stream.message().await;
                    let _ = stream.message().await;
                    drop(stream);
                    drop(client);
                }));
            }
            for handle in handles {
                let _ = handle.await;
            }
        });
    });
    println!(
        "  {:>34}  {:.3} core-seconds",
        "8 scans abandoned after one batch", abandoned
    );
    println!(
        "\n  A scan that ignored the hang-up would cost the same as a drained one.\n\
         The difference between those two rows is how much of the scan the head\n\
         node declined to do."
    );

    drop(serving);
}

/// Server-runtime CPU spent over `window`, while `during` runs.
fn server_cpu_over(rt: &Runtimes, window: Duration, during: impl FnOnce(&Runtimes)) -> f64 {
    let before = cpu_now();
    let started = Instant::now();
    during(rt);
    let remaining = window.saturating_sub(started.elapsed());
    std::thread::sleep(remaining);
    let wall = started.elapsed();
    let delta = cpu_between(&before, &cpu_now(), wall);
    delta.cores_of("hd-") * wall.as_secs_f64()
}

// --- 4. lease renewal in the tail ------------------------------------------

/// A lease whose conditional PUT takes as long as one to a bucket.
///
/// The object store here is in memory, so a renewal is under a microsecond and
/// cannot possibly show in a tail. That answers a question nobody asked. What a
/// deployment has is a renewal that takes tens of milliseconds, and the
/// question worth answering is whether *that* is visible to a caller — whether
/// the renewal path holds anything the request path needs while it waits.
///
/// The delay is a `sleep`, which is what an async network wait is from the
/// runtime's point of view. It models the wait, not the bytes.
#[derive(Debug)]
struct SlowLease {
    inner: ObjectStoreLease,
    delay: Duration,
}

#[tonic::async_trait]
impl Lease for SlowLease {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        tokio::time::sleep(self.delay).await;
        self.inner.acquire().await
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        tokio::time::sleep(self.delay).await;
        self.inner.renew().await
    }

    async fn release(&self) -> Result<(), LeaseError> {
        self.inner.release().await
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        self.inner.observe().await
    }

    fn held(&self) -> Option<Term> {
        self.inner.held()
    }

    fn holder(&self) -> &str {
        self.inner.holder()
    }
}

fn section_lease(rt: &Runtimes) {
    heading("4. Is a lease renewal visible in the tail?");

    let backend = rt.head.block_on(Backend::open());
    let writer = backend.visible();
    let table = events();
    let inproc = InProcess::new(catalog(), security(), Arc::clone(&writer), Vec::new());
    rt.head
        .block_on(seed(&inproc.writer, &table, TENANT, 1, 2_000));

    println!(
        "\n`head_report` asked this at one request at a time and found nothing at 250x\n\
         the real renewal rate. Under concurrent load the question is sharper, because\n\
         a stall that a single caller averages away lands on whichever requests are in\n\
         flight when it happens. Three arms, 8 clients doing point reads.\n"
    );

    let arms: Vec<(&str, Option<(Duration, Duration)>)> = vec![
        ("no renewal at all", None),
        (
            "renew every 20 ms, in-memory PUT",
            Some((Duration::from_millis(60), Duration::ZERO)),
        ),
        (
            "renew every 100 ms, 15 ms PUT",
            Some((Duration::from_millis(300), Duration::from_millis(15))),
        ),
    ];

    println!(
        "{:>34} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "arm", "ops/s", "p50", "p90", "p99", "worst"
    );
    println!("{:-<88}", "");

    for (label, renewal) in arms {
        let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        // How many renewals happened during the arm, and how long the slowest
        // took. A renewal count of zero would make the arm a duplicate of the
        // first one wearing a different label, which is a way for this section
        // to report a null result it never actually tested.
        let renewals = Arc::new(AtomicU64::new(0));
        let slowest_ns = Arc::new(AtomicU64::new(0));
        let mut maintainer: Option<tokio::task::JoinHandle<()>> = None;
        let leadership = match renewal {
            None => rt.head.block_on(leading()),
            Some((term, delay)) => {
                let base = ObjectStoreLease::with_holder(
                    Arc::clone(&object_store),
                    "leases/writer",
                    "bench".to_owned(),
                )
                .with_term_length(term);
                let lease: Arc<dyn Lease> = if delay.is_zero() {
                    Arc::new(base)
                } else {
                    Arc::new(SlowLease { inner: base, delay })
                };
                let leadership = Leadership::new(lease);
                assert!(
                    rt.head.block_on(leadership.campaign()),
                    "take the bench lease"
                );
                let cadence = Cadence::for_term(term);
                let renew_every = cadence.renew_every;
                maintainer = Some({
                    let leadership = Arc::clone(&leadership);
                    let renewals = Arc::clone(&renewals);
                    let slowest_ns = Arc::clone(&slowest_ns);
                    // `leadership::maintain`'s Leader branch, written out here
                    // so the renewals can be counted and timed. Nothing else
                    // differs: sleep the cadence, renew, repeat.
                    rt.head.spawn(async move {
                        loop {
                            tokio::time::sleep(renew_every).await;
                            let at = Instant::now();
                            if !leadership.renew().await {
                                return;
                            }
                            let took = at.elapsed().as_nanos() as u64;
                            renewals.fetch_add(1, Ordering::Relaxed);
                            slowest_ns.fetch_max(took, Ordering::Relaxed);
                        }
                    })
                });
                leadership
            }
        };

        let head: Head<SlateStore> = slate_headbench::harness::head(
            catalog(),
            security(),
            Arc::clone(&writer),
            Vec::new(),
            leadership,
            Limits::default(),
        );
        let serving = rt.head.block_on(serve(head));
        let clients = rt.load.block_on(connect_many(serving.address, 8));
        let repeated = rt.load.block_on(async {
            let mut out = Vec::new();
            for _ in 0..runs() {
                out.push(
                    drive(
                        clients.clone(),
                        Duration::from_millis(1_500),
                        |mut client, sequence| async move {
                            let id = (sequence % 2_000) + 1;
                            let ok = client
                                .get(principal_request(
                                    pb::GetRequest {
                                        schema: None,
                                        transaction: String::new(),
                                        table: "events".to_owned(),
                                        primary_key: Some(wire_key(TENANT, id)),
                                        freshness: any_freshness(),
                                    },
                                    TENANT,
                                ))
                                .await
                                .is_ok_and(|reply| reply.into_inner().found);
                            (client, ok)
                        },
                    )
                    .await,
                );
            }
            Repeated::new(out)
        });
        println!(
            "{:>34} {:>10.0} {:>10} {:>10} {:>10} {:>10}",
            label,
            repeated.throughput().0,
            column(repeated.p50().0),
            column(repeated.p90().0),
            column(repeated.p99().0),
            column(repeated.max().0),
        );
        if let Some(handle) = maintainer {
            handle.abort();
            println!(
                "{:>34}   {} renewals during the arm, slowest {}",
                "",
                renewals.load(Ordering::Relaxed),
                duration(slowest_ns.load(Ordering::Relaxed) as f64)
            );
        }
        drop(serving);
    }

    println!(
        "\nA renewal that stalled the request path would raise the worst sample of the\n\
         arm it runs in by about the length of the stall. A 15 ms conditional PUT is\n\
         at the fast end of what S3 does; if 15 ms of it reaches a caller, so would\n\
         a slower one."
    );
}
