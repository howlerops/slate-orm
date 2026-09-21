//! The in-process S3 server has Nagle's algorithm on, and every measurement
//! taken against it has paid for it.
//!
//! ```sh
//! cargo run --release -p slate-headbench --example s3_nodelay
//! ```
//!
//! # Why this exists
//!
//! `crates/slate-slatedb/tests/common/s3server.rs` binds its own
//! `TcpListener`, accepts a `TcpStream` and hands it straight to
//! `hyper_util::server::conn::auto`. Nothing in that path sets `TCP_NODELAY`:
//! hyper does not own the socket and never touches its options, and unlike
//! `tonic::transport::Server` there is not even a default to be ignored. That
//! is the same shape as the head node's Nagle bug one layer down — a server
//! that binds its own listener and serves through a Nagled socket — and it is
//! the server under `scan_tuning`, `cost_calibration` and `cost_at_scale`,
//! which is to say under the only wall-clock numbers in this project that are
//! *not* taken against a latency model.
//!
//! # The measurement
//!
//! Two servers, identical but for one line, each with its own storage and its
//! own SlateDB. The same scan is run against both, at two readahead settings,
//! with the server's own request counter as the control: **the request count
//! must not move**, because the socket option cannot change how many blocks a
//! scan reads. `TcpExt: DelayedACKs` is read across each scan, because that is
//! the kernel event a Nagled sender waits on and it turns a timing correlation
//! into a mechanism.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use slate_headbench::load::tcp_ext;
use slate_headbench::stats::{Measure, difference};
use slate_kernel::{Action, Grant, Query, RecordStore, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, Row, TableDef, TableId};
use slate_slatedb::{S3Config, ScanTuning, SlateStore};
use slate_tuple::{Value, ValueType};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const ACCESS_KEY: &str = "slateorm";
const SECRET_KEY: &str = "slateormsecret";
const EVENTS: TableId = TableId(1);

/// Rows in the fixture. The same count `scan_tuning` uses, so the numbers here
/// can be read against the table in `docs/performance.md`.
///
/// The default is that count and is unchanged. `HEADBENCH_ROWS` overrides it
/// only so `run.sh` can prove this still runs in seconds; at a smoke size the
/// comparison between the two servers is meaningless, which is why the small
/// size is asked for explicitly and never defaulted to.
const ROWS: u64 = 20_000;

fn rows() -> u64 {
    std::env::var("HEADBENCH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(ROWS)
}

fn runs() -> usize {
    std::env::var("HEADBENCH_RUNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(5)
}

// --- the fixture schema, copied from `scan_tuning` so the two compare -------

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("body", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("kind-{}", id % 8)),
        Value::Str("x".repeat(200)),
    ])
}

fn catalog() -> Catalog {
    Catalog::from_tables([events()]).expect("catalog")
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("bench", EVENTS, Action::ALL))
}

// --- the server -------------------------------------------------------------

#[derive(Debug, Default)]
struct Requests(AtomicU64);

impl Requests {
    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    fn reset(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct Counting<S> {
    inner: S,
    counters: Arc<Requests>,
}

impl<S, B> hyper::service::Service<hyper::Request<B>> for Counting<S>
where
    S: hyper::service::Service<hyper::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn call(&self, request: hyper::Request<B>) -> Self::Future {
        if request.method() == hyper::Method::GET {
            self.counters.0.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.call(request)
    }
}

struct LocalS3 {
    address: SocketAddr,
    bucket: String,
    counters: Arc<Requests>,
    server: JoinHandle<()>,
    _directory: tempfile::TempDir,
}

impl Drop for LocalS3 {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl LocalS3 {
    /// Start a server. `nodelay` is the whole experiment: `false` is the line
    /// `s3server.rs` has today, `true` is the one-line fix.
    async fn start(bucket: &str, nodelay: bool) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(directory.path().join(bucket)).expect("create bucket");

        let filesystem = s3s_fs::FileSystem::new(directory.path()).expect("filesystem backend");
        let mut builder = S3ServiceBuilder::new(filesystem);
        builder.set_auth(SimpleAuth::from_single(ACCESS_KEY, SECRET_KEY));
        let counters = Arc::new(Requests::default());
        let service = Counting {
            inner: builder.build(),
            counters: Arc::clone(&counters),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                if nodelay {
                    let _ = stream.set_nodelay(true);
                }
                let service = service.clone();
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let _ = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .serve_connection(io, service)
                    .await;
                });
            }
        });

        Self {
            address,
            bucket: bucket.to_owned(),
            counters,
            server,
            _directory: directory,
        }
    }

    fn config(&self) -> S3Config {
        S3Config::new(&self.bucket)
            .with_endpoint(format!("http://{}", self.address))
            .with_credentials(ACCESS_KEY, SECRET_KEY)
            .with_region("us-east-1")
            .allow_http(true)
    }
}

// --- the workload -----------------------------------------------------------

async fn load(path: &str, config: S3Config) {
    let backend = SlateStore::open_s3(path.to_owned(), config)
        .await
        .expect("open");
    let store = RecordStore::new(backend.clone(), catalog(), security());
    let root = SecurityContext::superuser();
    for chunk in (0..rows()).collect::<Vec<_>>().chunks(1_000) {
        let rows: Vec<Row> = chunk.iter().map(|id| row(*id)).collect();
        let txn = store.begin().await.expect("begin");
        txn.insert_many(&root, &events(), &rows)
            .await
            .expect("write");
        txn.commit().await.expect("commit");
    }
    backend.close().await.expect("close");
}

/// One scan of the whole table over a freshly reopened database.
async fn scan_once(path: &str, config: S3Config, tuning: ScanTuning) -> f64 {
    let backend = SlateStore::open_s3(path.to_owned(), config)
        .await
        .expect("reopen")
        .with_scan_tuning(tuning);
    let store = RecordStore::new(backend.clone(), catalog(), security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.expect("begin");
    let started = Instant::now();
    let counted = txn
        .execute(&root, &events(), &Query::all())
        .await
        .expect("scan")
        .count()
        .await
        .expect("count");
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(counted as u64, rows(), "a scan lost rows");
    drop(txn);
    backend.close().await.expect("close");
    elapsed
}

struct Result_ {
    label: String,
    time: Measure,
    gets: u64,
    acks: u64,
}

async fn arm(label: &str, nodelay: bool, tuning: ScanTuning) -> Result_ {
    let server = LocalS3::start("slate-orm", nodelay).await;
    let path = "/s3-nodelay";
    load(path, server.config()).await;

    // One untimed pass: the reopen path warms the filesystem backend's page
    // cache, and charging that to run one would show up as a spread.
    let _ = scan_once(path, server.config(), tuning).await;

    let mut time = Measure::new(label.to_owned(), 1);
    server.counters.reset();
    let acks_before = tcp_ext("DelayedACKs");
    for _ in 0..runs() {
        let elapsed = scan_once(path, server.config(), tuning).await;
        time.run_each(std::time::Duration::from_secs_f64(elapsed / 1000.0));
    }
    let acks = tcp_ext("DelayedACKs").saturating_sub(acks_before);
    let gets = server.counters.get() / runs() as u64;
    Result_ {
        label: label.to_owned(),
        time,
        gets,
        acks: acks / runs() as u64,
    }
}

#[tokio::main]
async fn main() {
    println!("# The in-process S3 server's socket\n");
    println!(
        "{} rows, scanned end to end over a real S3 server in this process.\n\
         Two servers, identical but for `set_nodelay(true)` on each accepted\n\
         connection. Runs: {}, `median [min – max]`.\n",
        rows(),
        runs()
    );

    let tuned = ScanTuning::default();
    let conservative = ScanTuning::conservative();

    let mut results = Vec::new();
    // Interleaved, and then repeated in the other order, so a machine that
    // gets busier over the run cannot be mistaken for a slower socket.
    results.push(arm("Nagle on,      1 MiB readahead", false, tuned).await);
    results.push(arm("TCP_NODELAY,   1 MiB readahead", true, tuned).await);
    results.push(arm("TCP_NODELAY,   SlateDB defaults", true, conservative).await);
    results.push(arm("Nagle on,      SlateDB defaults", false, conservative).await);

    println!(
        "{:<34} {:>10} {:>26} {:>16}",
        "arm", "S3 GETs", "wall clock", "delayed ACKs/scan"
    );
    println!("{:-<90}", "");
    for result in &results {
        println!(
            "{:<34} {:>10} {:>26} {:>16}",
            result.label,
            result.gets,
            result.time.summary(),
            result.acks
        );
    }

    println!("\n## Verdict\n");
    println!("The GET count is the control: the socket option cannot change how many");
    println!("blocks a scan reads, so if it moves, the two arms are not the same test.\n");
    for pair in [(0usize, 1usize), (3, 2)] {
        let (slow, fast) = (&results[pair.0], &results[pair.1]);
        println!(
            "  {} vs {}: {} GETs vs {} GETs, {}",
            slow.label.trim(),
            fast.label.trim(),
            slow.gets,
            fast.gets,
            difference(&slow.time, &fast.time)
        );
    }
}
