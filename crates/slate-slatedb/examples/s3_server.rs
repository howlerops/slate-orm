//! A real S3 server on a real socket, for standing the whole system up locally.
//!
//! `tests/common/s3server.rs` has run inside the test process since the S3
//! backend was written — the reason it gives is that "a Docker service
//! container makes the suite unrunnable on a machine without one", and the same
//! is true of a machine wanting to *run* the thing rather than test it.
//!
//! So this is that server as a process: `s3s` speaking actual signed S3 over a
//! socket, `s3s-fs` behind it, and the credentials printed so a head node can
//! be pointed at it. It exists for `examples/deployed/run.sh`, which runs
//! `slate-serverd` on top of SlateDB on top of this — the full stack a
//! deployment has, with nothing mocked between the gRPC socket and the object
//! store.
//!
//! What it is **not** is a production-grade S3. `s3s-fs` is a filesystem
//! pretending to be a bucket; it does not have MinIO's or R2's conditional-write
//! semantics, its multipart thresholds, or its error shapes. The `minio` job in
//! CI exists for exactly that layer, and this does not replace it.
//!
//!     cargo run -p slate-slatedb --example s3_server -- --bucket slate-orm
//!
//! Prints `LISTENING <addr>` once bound and before serving, which is the same
//! handshake `slate-serverd` uses — a caller waits for the line rather than
//! polling the port, which closes the race between bind and accept.

#![allow(clippy::expect_used, clippy::print_stdout)]

#[path = "../tests/common/s3server.rs"]
mod s3server;

use std::io::Write as _;

#[tokio::main]
async fn main() {
    let mut bucket = "slate-orm".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--bucket" => bucket = args.next().expect("--bucket needs a name"),
            other => {
                eprintln!("usage: s3_server [--bucket NAME]; `{other}` is not an option");
                std::process::exit(2);
            }
        }
    }

    let server = s3server::LocalS3::start(&bucket).await;
    let endpoint = server.endpoint();

    // Everything a caller needs to point `SLATE_S3_*` at this, on stdout in a
    // shape a shell can `eval`. The credentials are the fixed test ones and are
    // printed rather than hidden: this server has no data worth protecting and
    // a caller that had to read the source to find them would copy them wrong.
    println!("LISTENING {endpoint}");
    println!("SLATE_S3_BUCKET={bucket}");
    println!("SLATE_S3_ENDPOINT={endpoint}");
    println!("SLATE_S3_REGION=us-east-1");
    println!("SLATE_S3_ACCESS_KEY_ID={}", s3server::ACCESS_KEY);
    println!("SLATE_S3_SECRET_ACCESS_KEY={}", s3server::SECRET_KEY);
    println!("SLATE_S3_ALLOW_HTTP=true");
    std::io::stdout().flush().expect("flush");

    // Serve until killed. The server's own task does the accepting; this just
    // keeps the process and its temporary directory alive.
    std::future::pending::<()>().await;
    drop(server);
}
