//! `[limits] request_timeout` and `max_concurrent_requests`, against a live node.
//!
//! `ledger/2026-09-20-the-ceilings-and-the-paths-they-do-not-reach.md` recorded
//! exactly this absence: *"Nothing tests the daemon's `max_concurrent_requests`
//! or `request_timeout`. They are deliberately unset by default, so there is no
//! default to assert, and I did not check that setting them in TOML actually
//! takes effect end to end. That is a different kind of test — a live server —
//! and it is not here."*
//!
//! It is here now. Both settings are read from the file, handed to tonic's
//! builder, and never touched again, so the only question a test can answer is
//! whether the value survives that trip — which means a request that behaves
//! differently with the setting than without it, and a control that shows the
//! difference is the setting rather than the day.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    unreachable_pub
)]

mod harness;

use harness::{Files, Identity, Serving, connect, query, rows};

/// The smallest node that answers a query: one table, one row, no policy.
///
/// Deliberately smaller than `process.rs`'s fixture. Nothing here is about what
/// a declaration does to a row, so a second copy of that fixture would be a
/// second thing to keep in step for no assertion.
fn config(limits: &str) -> String {
    format!(
        r#"
[listen]
address = "127.0.0.1:0"

[auth]
mode = "trusted-header"
require_tenant = true

[shutdown]
grace = "2s"

[storage]
backend = "memory"

{limits}

[[tables]]
name = "docs"
id = 1
columns = [
  {{ name = "tenant_id", type = "u64" }},
  {{ name = "id",        type = "u64" }},
]
primary_key = ["tenant_id", "id"]
tenant_column = "tenant_id"

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["everything"]
"#
    )
}

const SEED: &str = r#"
[[seed]]
table = "docs"
rows = [
  { tenant_id = 1, id = 1 },
  { tenant_id = 1, id = 2 },
]
"#;

const APP: Identity = Identity::app("u64:1", "u64:1");

/// Rows enough that a reader who does not read keeps the stream open.
///
/// **This number is the whole mechanism of the stream-cap test, and two rows
/// were not enough.** A permit that rides on the response body comes back when
/// the body is *done*, and a body is done when the server has finished
/// writing it — not when the client has read it. Two rows fit in one frame, so
/// the first stream's permit was already back before the second request was
/// sent and the refusal never happened.
///
/// What holds a body open is HTTP/2 flow control: the connection and stream
/// windows start at 64 KiB, and a server that has filled them stops writing
/// until the reader consumes. At two `u64`s a row and 256 rows a message this
/// is roughly half a megabyte, which is eight windows — margin enough that the
/// test does not depend on the exact framing.
///
/// So the cap bounds streams the node is **still producing**, which is the
/// resource worth bounding, and not idle handles a client is holding. That
/// distinction is real and is written up in the entry rather than hidden
/// behind a number that happens to work.
const WIDE: usize = 20_000;

/// `WIDE` rows of seed, generated rather than written out.
fn wide_seed() -> String {
    let mut seed = String::from("[[seed]]\ntable = \"docs\"\nrows = [\n");
    for id in 1..=WIDE {
        seed.push_str(&format!("  {{ tenant_id = 1, id = {id} }},\n"));
    }
    seed.push_str("]\n");
    seed
}

fn serving_seeded(files: &Files, limits: &str, tag: &str, seed_text: &str) -> Serving {
    let config = files.write(&format!("{tag}.toml"), &config(limits));
    let seed = files.write(&format!("{tag}-seed.toml"), seed_text);
    Serving::start(&[
        "--config",
        &config.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ])
}

fn serving(files: &Files, limits: &str, tag: &str) -> Serving {
    let config = files.write(&format!("{tag}.toml"), &config(limits));
    let seed = files.write(&format!("{tag}-seed.toml"), SEED);
    Serving::start(&[
        "--config",
        &config.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ])
}

/// **A `request_timeout` is not a reliable bound, and not a reliable no-op.**
///
/// Whether a small timeout cancels a fast request depends on whether the
/// handler's future pends before returning, which depends on the scheduler and
/// the socket rather than on anything here. `GrpcTimeout::poll` polls the inner
/// future *first* and returns its `Poll::Ready` before the sleep is polled:
///
/// ```text
/// if let ready @ Poll::Ready(_) = this.inner.poll(cx) { return ready; }
/// if let Some(sleep) = this.sleep.as_pin_mut() { ready!(sleep.poll(cx)); … }
/// ```
///
/// So a handler that completes on its first poll cannot be cancelled — and a
/// handler that pends once, for a read that is not ready yet, meets an
/// already-elapsed sleep on the next one and is.
///
/// **Measured both ways, which is why nothing below asserts an outcome.** On
/// this container `request_timeout = "0ms"` left a two-row in-memory query
/// answered, five runs out of five, and `1ms` did too. An earlier version of
/// this file asserted that as the behaviour; CI run 443 failed it on the first
/// try with `Cancelled: Timeout expired` on the same query and the same
/// configuration. Two machines, opposite answers, no change in between.
///
/// The claim published from the five-out-of-five reading — "a query answered
/// at once returns its rows whatever the timeout says" — is withdrawn. What
/// stands is the weaker and more useful statement: a timeout below the time a
/// request spends waiting will *sometimes* cancel it and sometimes not, so a
/// small `request_timeout` is neither a latency ceiling nor a harmless
/// setting. A deployment wanting requests bounded cannot get it from here; a
/// deployment setting a small value to be safe is introducing a flake.
///
/// What is asserted instead is the deterministic half: a generous timeout
/// serves, which shows the setting is read and applied without depending on a
/// race. The zero case is left unasserted deliberately — a test whose expected
/// outcome differs by machine is a test that teaches people to re-run CI.
#[tokio::test]
async fn a_generous_request_timeout_serves_the_query() {
    let files = Files::new();
    let serving = serving(&files, "[limits]\nrequest_timeout = \"60s\"", "timeout");
    let mut client = connect(&serving).await;

    let returned = rows(&mut client, &APP, query("docs"))
        .await
        .expect("a timeout far longer than the query cannot cancel it on any machine");
    assert_eq!(returned.len(), 2, "both seeded rows");

    drop(client);
    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

/// The unit the configuration accepts, which is what made the above the only
/// experiment available.
///
/// `1ns` is refused by name. `ms` is the floor, which is why `0ms` was the
/// smallest experiment available and why the answer it gave was a race rather
/// than a measurement. "Does a timeout fire predictably?" cannot be answered
/// from a client against an in-memory store, and is recorded as open rather
/// than asserted.
#[test]
fn a_sub_millisecond_timeout_is_refused_by_name() {
    let files = Files::new();
    let config = files.write("ns.toml", &config("[limits]\nrequest_timeout = \"1ns\""));
    let finished = harness::run(&["--config", &config.display().to_string(), "--check"]);

    assert_ne!(finished.code, Some(0), "`1ns` must not be accepted");
    let said = finished.output();
    assert!(
        said.contains("limits.request_timeout") && said.contains("ns"),
        "the refusal must name the field and the unit; got:\n{said}"
    );
}

/// The control: the same query against the same node with no timeout set.
///
/// Without it the test above proves only that the node serves, which says
/// nothing about the setting having been read. Together they show a node that
/// serves with the setting and without it, which is the whole of what can be
/// asserted deterministically here.
#[tokio::test]
async fn the_same_query_succeeds_with_no_timeout_set() {
    let files = Files::new();
    let serving = serving(&files, "", "no-timeout");
    let mut client = connect(&serving).await;

    let returned = rows(&mut client, &APP, query("docs"))
        .await
        .expect("a query with no request timeout");
    assert_eq!(returned.len(), 2, "both seeded rows");

    drop(client);
    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

/// A node started with `max_concurrent_requests` still serves.
///
/// **It is node-wide now, and still weaker than it looks.**
/// `max_concurrent_requests` was tonic's `concurrency_limit_per_connection`,
/// which gave a caller a fresh allowance per socket; it is now
/// `tower::limit::GlobalConcurrencyLimitLayer`, one semaphore for the process.
/// That fixes the half that was a lie about scope. The half that remains is
/// about *when the permit is released*: a `tower` concurrency limit holds the
/// permit in the response future and drops it when that future resolves, and
/// for a server-streaming RPC — `Query`, `Join` and `Aggregate` are all
/// streaming — that is once the headers are ready, before a single row has
/// been read. So a held-open stream holds no permit, and there is still no way
/// from a client to keep one long enough to watch the limit refuse anything.
///
/// What is asserted is therefore the whole of what a client can see: the value
/// reaches the layer without the node refusing to start, and a request still
/// gets an answer. Whether the limit ever *binds* stays open, because a test
/// that cannot fail for the right reason is worse than an admission.
#[tokio::test]
async fn a_concurrency_limit_in_the_file_still_serves() {
    let files = Files::new();
    let serving = serving(
        &files,
        "[limits]\nmax_concurrent_requests = 1",
        "concurrency",
    );
    let mut client = connect(&serving).await;

    let returned = rows(&mut client, &APP, query("docs"))
        .await
        .expect("a query against a node limited to one concurrent request");
    assert_eq!(returned.len(), 2, "both seeded rows");

    drop(client);
    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

/// `max_concurrent_requests = 0` is refused at startup rather than served.
///
/// The one place the setting has an effect a test can pin without depending on
/// tower's internals: zero would serve nobody, so `main` refuses it by name
/// while somebody is still reading the file.
#[test]
fn a_concurrency_limit_of_zero_is_refused_by_name() {
    let files = Files::new();
    let config = files.write(
        "zero.toml",
        &config("[limits]\nmax_concurrent_requests = 0"),
    );
    let finished = harness::run(&["--config", &config.display().to_string(), "--check"]);

    assert_ne!(finished.code, Some(0), "a zero limit must not be accepted");
    assert!(
        finished.output().contains("max_concurrent_requests"),
        "the refusal must name the field; got:\n{}",
        finished.output()
    );
}

/// How many streams to hold open at once, against a limit of one.
///
/// Ten rather than two, because two could be a scheduling accident and ten
/// could not: if a permit were held for a stream's life, the second `query`
/// would queue behind the first and never return, and the timeout below would
/// fail rather than the assertion.
const OPEN_AT_ONCE: usize = 10;

/// **The limit bounds requests admitted, not streams open**, measured.
///
/// `a_concurrency_limit_in_the_file_still_serves` above says this is so and
/// says it from reading: a `tower` concurrency limit holds its permit in the
/// response future, and a server-streaming handler's future resolves once the
/// stream is *returned*, before a row is read. The entry that shipped the
/// node-wide limit recorded the same thing as read rather than measured, which
/// is the weaker half of a claim about a resource control.
///
/// This is the measurement, and it is a count rather than a duration: ten
/// `Query` streams are opened and none is drained until all ten exist. Under a
/// limit of one, every one of them is admitted.
///
/// It is a *negative* result and it is the useful kind. A caller who opens ten
/// thousand slow streams is bounded by nothing here, so `max_concurrent_requests`
/// is not the control a deployment reaches for to cap that — and a future
/// change that started holding the permit for the stream's life would fail
/// here, deliberately, rather than quietly making the setting mean something
/// else.
#[tokio::test]
async fn open_streams_are_not_bounded_by_the_concurrency_limit() {
    use std::time::Duration;

    let files = Files::new();
    let mut serving = serving(
        &files,
        // The metrics port is what makes `OPEN_AT_ONCE` load-bearing. Without
        // it nothing in this test depends on the number: a mutation reducing
        // ten to one SURVIVED, because every assertion was about the streams
        // that *were* opened and none about how many. The server's own counter
        // is the only witness a client has that ten requests were admitted
        // under a limit of one.
        "[limits]\nmax_concurrent_requests = 1\n\n[observability]\nmetrics_address = \"127.0.0.1:0\"",
        "streams",
    );
    let metrics = serving
        .metrics_address()
        .expect("the node should announce its metrics port");
    let client = connect(&serving).await;

    // Opened sequentially, and that is enough: a permit held for a stream's
    // life would make the *second* call queue behind the first for ever, and
    // `tower`'s limit queues rather than refusing. So the discriminator is
    // whether the call returns at all, not how fast.
    let mut open = Vec::new();
    for n in 0..OPEN_AT_ONCE {
        let mut one = client.clone();
        let request = APP.on(harness::proto::QueryRequest {
            transaction: String::new(),
            query: Some(query("docs")),
            freshness: None,
        });
        let opened = tokio::time::timeout(Duration::from_secs(20), one.query(request))
            .await
            .unwrap_or_else(|_| {
                panic!("stream {n} never opened with {n} already open and a limit of one")
            })
            .expect("a query stream under a concurrency limit of one");
        open.push(opened.into_inner());
    }
    assert_eq!(open.len(), OPEN_AT_ONCE, "every stream was admitted");

    // And each is a usable stream rather than an admitted empty one: the rows
    // arrive after all ten were opened, which is the half that says the
    // permits were released before the data and not that the reads were
    // already finished.
    for (n, mut stream) in open.into_iter().enumerate() {
        let mut seen = 0;
        while let Some(message) = stream
            .message()
            .await
            .unwrap_or_else(|error| panic!("draining stream {n}: {error}"))
        {
            seen += message.rows.len();
        }
        assert_eq!(seen, 2, "stream {n} carried both seeded rows");
    }

    drop(client);

    // Ten admitted, counted by the node rather than inferred from the client
    // having ten handles. The layer that counts wraps the handler, so this is
    // the server saying it let ten `Query` requests in while its concurrency
    // limit was one.
    let scraped = scrape(&metrics, "/metrics").await;
    let wanted =
        format!("slate_requests_total{{method=\"/slate.v1.Records/Query\"}} {OPEN_AT_ONCE}");
    // Whole line, not `contains`. A count is a prefix of a bigger count, so
    // `contains("… 1")` is satisfied by "… 10" — a mutation that replaced
    // `{OPEN_AT_ONCE}` with a literal `1` survived exactly that, and the
    // assertion had been reading as a check on the number while matching any
    // number starting with it.
    assert!(
        scraped.lines().any(|line| line.trim() == wanted),
        "the node should have counted {OPEN_AT_ONCE} admitted queries; got:\n{scraped}"
    );

    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

/// One line of HTTP against the metrics port.
///
/// A copy of `observing.rs`'s, and the duplication is deliberate: a shared
/// helper would go in `harness/`, and the harness is shared by seven test
/// binaries that do not scrape anything. Seven lines here against a module
/// six files import for nothing.
async fn scrape(address: &str, path: &str) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut socket = tokio::net::TcpStream::connect(address)
        .await
        .expect("the metrics port should accept a connection");
    socket
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: m\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .expect("a request should be writable");
    let mut answered = String::new();
    // Bounded, for the reason `observing.rs` gives: a bound-but-unaccepted
    // listener completes the handshake out of the kernel's backlog, so only
    // the read hangs — and a hanging test is a six-hour CI job rather than a
    // red cross.
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        socket.read_to_string(&mut answered),
    )
    .await
    .expect("the metrics endpoint should answer, not hang")
    .expect("the metrics response should be readable");
    answered
}

/// The same ten streams, against a node that bounds *streams*.
///
/// The test above measures the absence: ten open reads under
/// `max_concurrent_requests = 1`, all served, because that permit comes back
/// when the handler returns and every read here is server-streaming. This is
/// the mechanism the entry for that measurement said did not exist —
/// `[limits] max_open_streams`, a node-wide semaphore whose permit rides on
/// the response body.
///
/// **The discriminator is a refusal, not a delay.** Two streams are opened
/// against a cap of one and neither is drained. The first must carry its rows;
/// the second must come back `RESOURCE_EXHAUSTED` rather than block, because a
/// queue in front of a stream has no deadline — see `crate::streams`. Then the
/// first is drained, its permit returns, and a third opens: that half is what
/// says the permit is *released* rather than merely taken, and without it a
/// layer that refused everything after the first request would pass.
#[tokio::test]
async fn a_second_stream_is_refused_when_one_may_be_open() {
    use std::time::Duration;

    let files = Files::new();
    let serving = serving_seeded(
        &files,
        // Both limits, and the concurrency one generous. The refusal below
        // must be attributable to the stream cap: with `max_concurrent_requests`
        // unset a reader could object that admission was never bounded, and
        // with it at 1 a refusal could be either layer.
        "[limits]\nmax_concurrent_requests = 8\nmax_open_streams = 1",
        "open-streams",
        &wide_seed(),
    );
    let client = connect(&serving).await;

    let ask = || {
        APP.on(harness::proto::QueryRequest {
            transaction: String::new(),
            query: Some(query("docs")),
            freshness: None,
        })
    };

    let mut first = client.clone();
    let mut held = tokio::time::timeout(Duration::from_secs(20), first.query(ask()))
        .await
        .expect("the first stream should open at once under a cap of one")
        .expect("the first query under max_open_streams = 1")
        .into_inner();

    // Undrained, so its permit is still out. `tokio::time::timeout` is the
    // assertion that matters here: a layer that *queued* would hang, and a
    // hang is not a failure unless something makes it one.
    let mut second = client.clone();
    let refused = tokio::time::timeout(Duration::from_secs(20), second.query(ask()))
        .await
        .expect("the second query should be refused, not queued behind the first");
    let status = refused.expect_err("a second stream should be refused under a cap of one");
    assert_eq!(
        status.code(),
        tonic::Code::ResourceExhausted,
        "the refusal should name the quota, not the node's health; got {status:?}"
    );
    assert!(
        status.message().contains("open response streams"),
        "the refusal should say which limit refused it; got {:?}",
        status.message()
    );

    // Drain the first, which returns its permit when the body ends.
    let mut seen = 0;
    while let Some(message) = held.message().await.expect("draining the first stream") {
        seen += message.rows.len();
    }
    assert_eq!(seen, WIDE, "the first stream carried every seeded row");
    drop(held);

    // And now a third opens. This is the half that distinguishes a released
    // permit from a layer that refuses everything after the first request —
    // a mutation replacing the semaphore with a "one request ever" flag passes
    // every assertion above and fails here.
    //
    // Retried rather than asserted once: the permit is returned when the body
    // is dropped, which happens on the server's schedule and not on this
    // task's, so a single immediate attempt is a race. Twenty tries at 50 ms
    // is a second, against a drop that should take microseconds.
    let mut third = None;
    for _ in 0..20 {
        let mut again = client.clone();
        match again.query(ask()).await {
            Ok(opened) => {
                third = Some(opened.into_inner());
                break;
            }
            Err(status) if status.code() == tonic::Code::ResourceExhausted => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(status) => panic!("a third stream failed for an unexpected reason: {status:?}"),
        }
    }
    let mut third = third.expect("the permit should return when the first stream is drained");
    let mut after = 0;
    while let Some(message) = third.message().await.expect("draining the third stream") {
        after += message.rows.len();
    }
    assert_eq!(after, WIDE, "the third stream carried every seeded row");

    drop(client);
    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

/// `max_open_streams = 0` is refused at startup rather than served.
///
/// Zero would refuse every read, which is the same reasoning
/// `max_concurrent_requests = 0` is refused for and the same shape of mistake:
/// a number that reads as "no limit" and means "no service".
#[test]
fn an_open_stream_limit_of_zero_is_refused_by_name() {
    let files = Files::new();
    let config = files.write(
        "zero-streams.toml",
        &config("[limits]\nmax_open_streams = 0"),
    );
    let finished = harness::run(&["--config", &config.display().to_string(), "--check"]);

    assert_ne!(finished.code, Some(0), "a zero cap must not be accepted");
    assert!(
        finished.output().contains("max_open_streams"),
        "the refusal must name the field; got:\n{}",
        finished.output()
    );
}
