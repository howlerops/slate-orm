//! Starting the real binary, and talking to it.
//!
//! Every test in this directory runs the process. Nothing here constructs a
//! `Head` in-process: the thing being tested is a *binary* — that a file turns
//! into a serving node, that a bad file turns into a refusal and an exit code
//! — and an in-process assembly would test the half of that which was never in
//! doubt.
//!
//! The banner is the synchronisation point. `slate-serverd` prints
//! `LISTENING <address>` after binding and before serving, so a client that
//! connects the instant it reads that line finds a socket already accepting.
//! Polling the port instead is a race that fails about once a week.

#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_update
)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long to wait for the banner, and for an exit.
///
/// Generous: a debug-profile SlateDB open on a loaded machine is slow, and a
/// flaky timeout is worse than a slow suite.
const PATIENCE: Duration = Duration::from_secs(30);

/// A configuration and seed file on disk, kept alive with them.
pub struct Files {
    directory: tempfile::TempDir,
}

impl Files {
    pub fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("a temporary directory"),
        }
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    /// Write `contents` to `name` and return its path.
    pub fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.directory.path().join(name);
        std::fs::write(&path, contents).expect("write a fixture file");
        path
    }
}

impl Default for Files {
    fn default() -> Self {
        Self::new()
    }
}

/// The binary under test.
pub fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_slate-serverd")
}

/// Run the binary to completion and collect what it said.
pub struct Finished {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Finished {
    /// Both streams, for an assertion that does not care which one carried the
    /// message.
    pub fn output(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// Run the binary with `arguments` and wait for it to exit.
///
/// Bounded, and the bound is load-bearing rather than defensive. Several of
/// these tests assert that the process *refuses* — a bad configuration, a
/// lease somebody else holds — and a regression there does not turn a refusal
/// into a different refusal, it turns it into a server that starts and keeps
/// running. Waiting for that one to exit would hang the suite forever, which
/// is how a broken test looks like an infrastructure problem. It fails instead.
pub fn run(arguments: &[&str]) -> Finished {
    let mut child = Command::new(binary())
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run slate-serverd");

    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        match child.try_wait().expect("wait for slate-serverd") {
            Some(_) => break,
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "`slate-serverd {}` was still running after {PATIENCE:?}; it was expected to exit",
                    arguments.join(" ")
                );
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }

    let output = child
        .wait_with_output()
        .expect("collect slate-serverd's output");
    Finished {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// A running head node.
pub struct Serving {
    child: Child,
    /// The address the banner reported, which for port 0 is the one that was
    /// actually bound.
    pub address: String,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
}

impl Serving {
    /// Start the binary and wait for its banner.
    pub fn start(arguments: &[&str]) -> Self {
        let mut child = Command::new(binary())
            .args(arguments)
            // Merged so a startup refusal, which goes to stderr, shows up in
            // the failure message of a test that was waiting for a banner.
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn slate-serverd");

        let stdout = child.stdout.take().expect("a piped stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });

        let mut seen = Vec::new();
        let address = loop {
            match lines.recv_timeout(PATIENCE) {
                Ok(line) => {
                    if let Some(rest) = line.strip_prefix("LISTENING ") {
                        let address = rest.trim().to_owned();
                        seen.push(line);
                        break address;
                    }
                    seen.push(line);
                }
                Err(_) => {
                    let _ = child.kill();
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        use std::io::Read;
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    panic!("no banner within {PATIENCE:?}; the process said:\n{stderr}");
                }
            }
        };

        Self {
            child,
            address,
            lines,
            seen,
        }
    }

    /// A gRPC endpoint for this node.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The address the metrics endpoint bound, if the node was asked for one.
    ///
    /// Read from the `METRICS` banner rather than from the configuration,
    /// which is the only way a test can use `:0` — and using `:0` is the only
    /// way several of these can run at once on one machine, which they do.
    pub fn metrics_address(&mut self) -> Option<String> {
        // The line is printed immediately after `LISTENING`, so it is either
        // already in `seen` or one `recv` away. Bounded rather than looping on
        // the channel: a node with no metrics endpoint prints nothing here,
        // and a test asking a node that has none should get `None` quickly
        // rather than after the full patience.
        if let Some(found) = self.banner("METRICS ") {
            return Some(found);
        }
        match self.lines.recv_timeout(core::time::Duration::from_secs(2)) {
            Ok(line) => {
                self.seen.push(line);
                self.banner("METRICS ")
            }
            Err(_) => None,
        }
    }

    /// The first banner line with this prefix, if it has been seen.
    fn banner(&self, prefix: &str) -> Option<String> {
        self.seen
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
            .map(|rest| rest.trim().to_owned())
    }

    /// Send `SIGTERM` and wait for the process to exit.
    ///
    /// Through `kill(1)` rather than `Child::kill`, which sends `SIGKILL` and
    /// would test nothing about the shutdown path. There is no portable way to
    /// send `SIGTERM` from the standard library, and a `libc` dependency for
    /// one call in one test is not worth it.
    pub fn terminate(mut self) -> Finished {
        let pid = self.child.id().to_string();
        let sent = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .expect("send SIGTERM");
        assert!(sent.success(), "kill -TERM {pid} failed");

        let status = wait_for(&mut self.child);
        let mut stdout = self.seen.join("\n");
        // Drained to end-of-pipe, not for a fixed gap. The child has already
        // exited by here, so its stdout is closed and the reader thread ends —
        // which disconnects this channel and is the only thing that can end
        // this loop. Waiting 200 ms for the next line instead used to end the
        // drain early on a loaded machine, and `STOPPING SIGTERM` is written
        // *before* the exit this already waited for, so the assertion looking
        // for it failed for want of reading rather than for want of the line.
        // The deadline is a backstop against a wedged reader thread, not a
        // timing assumption: hitting it means something is broken, and the
        // panic says so rather than quietly returning a short transcript.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match self.lines.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    stdout.push('\n');
                    stdout.push_str(&line);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the child exited but its stdout never closed"
                    );
                }
            }
        }
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr);
        }
        Finished {
            code: status,
            stdout,
            stderr,
        }
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        // A test that panicked before `terminate` would otherwise leave a
        // process holding a port for the rest of the run.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Poll for an exit rather than blocking, so a wedged process fails the test
/// instead of hanging the suite.
fn wait_for(child: &mut Child) -> Option<i32> {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    panic!("the process did not exit within {PATIENCE:?} of SIGTERM");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(why) => panic!("cannot wait for the process: {why}"),
        }
    }
}

// ── the wire ────────────────────────────────────────────────────────────────

use proto::records_client::RecordsClient;
pub use slate_server::proto;
use tonic::Request;
use tonic::transport::Channel;

/// Connect to a running node.
pub async fn connect(serving: &Serving) -> RecordsClient<Channel> {
    RecordsClient::connect(serving.endpoint())
        .await
        .expect("connect to the head node")
}

/// Who the request is from, as the `trusted-header` mode reads it.
pub struct Identity {
    pub principal: &'static str,
    pub tenant: Option<&'static str>,
    pub roles: &'static str,
    /// A bearer token, for `token` mode.
    pub bearer: Option<String>,
}

impl Identity {
    pub const fn app(principal: &'static str, tenant: &'static str) -> Self {
        Self {
            principal,
            tenant: Some(tenant),
            roles: "app",
            bearer: None,
        }
    }

    pub const fn nobody() -> Self {
        Self {
            principal: "",
            tenant: None,
            roles: "",
            bearer: None,
        }
    }

    pub fn token(secret: &str) -> Self {
        Self {
            principal: "",
            tenant: None,
            roles: "",
            bearer: Some(secret.to_owned()),
        }
    }

    /// Attach this identity to a request.
    pub fn on<T>(&self, message: T) -> Request<T> {
        let mut request = Request::new(message);
        let metadata = request.metadata_mut();
        if let Some(bearer) = &self.bearer {
            metadata.insert(
                "authorization",
                format!("Bearer {bearer}").parse().expect("ascii"),
            );
            return request;
        }
        if !self.principal.is_empty() {
            metadata.insert("slate-principal", self.principal.parse().expect("ascii"));
        }
        if let Some(tenant) = self.tenant {
            metadata.insert("slate-tenant", tenant.parse().expect("ascii"));
        }
        if !self.roles.is_empty() {
            metadata.insert("slate-roles", self.roles.parse().expect("ascii"));
        }
        request
    }
}

/// A `u64` wire value.
pub fn u64_value(value: u64) -> proto::Value {
    proto::Value {
        kind: Some(proto::value::Kind::Uint64Value(value)),
    }
}

/// An `i64` wire value.
pub fn i64_value(value: i64) -> proto::Value {
    proto::Value {
        kind: Some(proto::value::Kind::Int64Value(value)),
    }
}

/// A string wire value.
pub fn str_value(value: &str) -> proto::Value {
    proto::Value {
        kind: Some(proto::value::Kind::StringValue(value.to_owned())),
    }
}

/// A null wire value.
pub fn null_value() -> proto::Value {
    proto::Value {
        kind: Some(proto::value::Kind::NullValue(0)),
    }
}

/// A row of the given values.
pub fn row(values: Vec<proto::Value>) -> proto::Row {
    proto::Row {
        values,
        ..Default::default()
    }
}

/// `column <op> value` against input 0.
pub fn compare(column: u32, op: proto::CmpOp, value: proto::Value) -> proto::Expr {
    proto::Expr {
        node: Some(proto::expr::Node::Compare(proto::Compare {
            column: Some(proto::ColumnRef {
                input: 0,
                of: Some(proto::column_ref::Of::Column(column)),
            }),
            op: op as i32,
            value: Some(value),
            ..Default::default()
        })),
    }
}

/// `column IS NOT NULL` against input 0.
pub fn is_not_null(column: u32) -> proto::Expr {
    proto::Expr {
        node: Some(proto::expr::Node::IsNull(proto::IsNull {
            column: Some(proto::ColumnRef {
                input: 0,
                of: Some(proto::column_ref::Of::Column(column)),
            }),
            negated: true,
            ..Default::default()
        })),
    }
}

/// Every conjunct of `parts`.
pub fn all_of(parts: Vec<proto::Expr>) -> proto::Expr {
    proto::Expr {
        node: Some(proto::expr::Node::Conjunction(proto::ExprList {
            exprs: parts,
        })),
    }
}

/// A whole-table query.
pub fn query(table: &str) -> proto::Query {
    proto::Query {
        table: table.to_owned(),
        ..Default::default()
    }
}

/// Run a query and collect every row.
pub async fn rows(
    client: &mut RecordsClient<Channel>,
    identity: &Identity,
    query: proto::Query,
) -> Result<Vec<proto::Row>, tonic::Status> {
    let request = identity.on(proto::QueryRequest {
        transaction: String::new(),
        query: Some(query),
        freshness: None,
    });
    let mut stream = client.query(request).await?.into_inner();
    let mut out = Vec::new();
    while let Some(message) = stream.message().await? {
        out.extend(message.rows);
    }
    Ok(out)
}
