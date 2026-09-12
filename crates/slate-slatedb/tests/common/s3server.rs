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
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const ACCESS_KEY: &str = "slateorm";
const SECRET_KEY: &str = "slateormsecret";

/// A running S3 server. Dropping it stops the server and deletes its storage.
pub struct LocalS3 {
    address: SocketAddr,
    bucket: String,
    server: JoinHandle<()>,
    // Held so the backing directory outlives the server.
    _directory: tempfile::TempDir,
}

impl LocalS3 {
    /// Start a server with `bucket` already created.
    pub async fn start(bucket: &str) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(directory.path().join(bucket)).expect("create bucket");

        let filesystem = s3s_fs::FileSystem::new(directory.path()).expect("filesystem backend");
        let mut builder = S3ServiceBuilder::new(filesystem);
        builder.set_auth(SimpleAuth::from_single(ACCESS_KEY, SECRET_KEY));
        let service = builder.build();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
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
            _directory: directory,
        }
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
