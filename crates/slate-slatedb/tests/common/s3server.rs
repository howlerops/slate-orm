//! A real S3 server, in this process.
//!
//! The S3 code path deserves to be exercised for real rather than only
//! compiled, but a Docker service container makes the suite unrunnable on a
//! machine without one. `s3s` implements the S3 protocol in Rust, so the tests
//! can speak actual signed S3 over a socket and stay hermetic.
//!
//! Point the same tests at MinIO, Tigris or R2 by setting `SLATE_S3_*`; see
//! `S3Config::from_env`.

#![allow(dead_code, unreachable_pub, clippy::expect_used, clippy::unwrap_used)]

use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use slate_slatedb::S3Config;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Requests the server has served, by method.
///
/// Wall-clock numbers move with the machine; "this scan made 1100 object-store
/// requests and that one made 5" does not. Counting them here is the same
/// discipline the kernel's `IoCounters` follow, one layer further down.
#[derive(Debug, Default)]
pub struct S3Counters {
    gets: AtomicU64,
    puts: AtomicU64,
    other: AtomicU64,
}

impl S3Counters {
    /// `GET` requests, which is what reading does.
    pub fn gets(&self) -> u64 {
        self.gets.load(Ordering::Relaxed)
    }

    /// `PUT` and `POST` requests, which is what writing does.
    pub fn puts(&self) -> u64 {
        self.puts.load(Ordering::Relaxed)
    }

    /// Everything else: `HEAD`, `DELETE`, listings.
    pub fn other(&self) -> u64 {
        self.other.load(Ordering::Relaxed)
    }

    /// Every request.
    pub fn total(&self) -> u64 {
        self.gets() + self.puts() + self.other()
    }

    /// Start counting again.
    pub fn reset(&self) {
        self.gets.store(0, Ordering::Relaxed);
        self.puts.store(0, Ordering::Relaxed);
        self.other.store(0, Ordering::Relaxed);
    }

    fn record(&self, method: &hyper::Method) {
        match *method {
            hyper::Method::GET => &self.gets,
            hyper::Method::PUT | hyper::Method::POST => &self.puts,
            _ => &self.other,
        }
        .fetch_add(1, Ordering::Relaxed);
    }
}

/// Wraps the S3 service to count what reaches it.
#[derive(Clone)]
struct Counting<S> {
    inner: S,
    counters: Arc<S3Counters>,
}

impl<S, B> hyper::service::Service<hyper::Request<B>> for Counting<S>
where
    S: hyper::service::Service<hyper::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn call(&self, request: hyper::Request<B>) -> Self::Future {
        self.counters.record(request.method());
        self.inner.call(request)
    }
}

/// The fixed credentials this server accepts.
///
/// Public so `examples/s3_server.rs` can print them for a head node to use.
/// There is no secret here: the server holds a temporary directory that is
/// deleted when it stops.
pub const ACCESS_KEY: &str = "slateorm";
pub const SECRET_KEY: &str = "slateormsecret";

/// A running S3 server. Dropping it stops the server and deletes its storage.
pub struct LocalS3 {
    address: SocketAddr,
    bucket: String,
    server: JoinHandle<()>,
    counters: Arc<S3Counters>,
    // Held so the backing directory outlives the server.
    directory: tempfile::TempDir,
}

impl LocalS3 {
    /// Where the server keeps its objects, on real disk.
    ///
    /// `s3s_fs` is a filesystem backend, not an in-memory one, so everything
    /// written through this server lands under a temporary directory and
    /// competes for the same free space as the build tree. That is invisible
    /// from the request counters — they count requests, not bytes — and
    /// `performance.md` has carried "the disk under the in-process S3 server"
    /// as an unruled-out explanation for the loader cliff since #118. An
    /// example cannot weigh what it cannot see.
    pub fn directory(&self) -> &std::path::Path {
        self.directory.path()
    }

    /// Start a server with `bucket` already created.
    pub async fn start(bucket: &str) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(directory.path().join(bucket)).expect("create bucket");

        let filesystem = s3s_fs::FileSystem::new(directory.path()).expect("filesystem backend");
        let mut builder = S3ServiceBuilder::new(filesystem);
        builder.set_auth(SimpleAuth::from_single(ACCESS_KEY, SECRET_KEY));
        let service = builder.build();
        let counters = Arc::new(S3Counters::default());
        let service = Counting {
            inner: service,
            counters: Arc::clone(&counters),
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                // Nagle off, and it is worth more here than anywhere else in
                // the repo: this server sits under `scan_tuning`,
                // `cost_calibration` and `cost_at_scale`, so a delay here is a
                // delay inside a *published measurement*. hyper never touches
                // socket options — unlike tonic there is not even a default
                // being ignored — so nothing was setting it.
                //
                // Measured (`slate-headbench`'s `s3_nodelay`): the
                // SlateDB-defaults arm of the readahead comparison took 47.14 s
                // Nagled against 827 ms with this line, over 1,377 requests —
                // 34.2 ms per GET, which is the delayed-ACK timer rather than
                // any code path, confirmed by 1,127 timer expiries per scan.
                // That arm is in `docs/performance.md`'s readahead table, whose
                // wall-clock ratios are corrected there from 158x-409x to about
                // 8x. The 31x request-count ratio that section actually quotes
                // is unaffected, because request counts do not care about ACKs.
                let _ = stream.set_nodelay(true);
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
            server,
            counters,
            directory,
        }
    }

    /// What this server has been asked to do.
    pub fn counters(&self) -> Arc<S3Counters> {
        Arc::clone(&self.counters)
    }

    /// Where this server is listening, as a URL.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    /// A config pointing at this server.
    pub fn config(&self) -> S3Config {
        S3Config::new(&self.bucket)
            .with_endpoint(format!("http://{}", self.address))
            .with_credentials(ACCESS_KEY, SECRET_KEY)
            .with_region("us-east-1")
            .allow_http(true)
    }
}

impl Drop for LocalS3 {
    fn drop(&mut self) {
        self.server.abort();
    }
}
