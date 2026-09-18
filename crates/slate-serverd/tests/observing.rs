//! What the node says about the requests it served.
//!
//! Through the real binary rather than against `Counters` directly. The unit
//! tests beside `observe.rs` cover the arithmetic — a mean divided by the
//! right count, a failure counted once — and would all still pass if the layer
//! were never applied to the server at all, which was the defect worth
//! guarding against: the counters are easy and the wiring is where this can
//! quietly do nothing.
//!
//! Every assertion here is about *stderr of a process*, which is the thing an
//! operator actually has.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_update,
    unreachable_pub
)]

mod harness;

use harness::{Files, Identity, Serving, connect, proto, query, rows, u64_value};

/// A node that says nothing extra. The `[observability]` section is absent on
/// purpose: the default is what most deployments run.
const QUIET: &str = r#"
[listen]
address = "127.0.0.1:0"

[auth]
mode = "trusted-header"

[shutdown]
grace = "2s"

[storage]
backend = "memory"

[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["everything"]
"#;

const SEED: &str = r#"
[[seed]]
table = "docs"
rows = [
  { id = 1, kind = "kind-a" },
  { id = 2, kind = "kind-b" },
]
"#;

const APP: Identity = Identity::app("u64:1", "u64:1");

/// The quiet configuration with an `[observability]` section appended.
fn talking(section: &str) -> String {
    format!("{QUIET}\n[observability]\n{section}\n")
}

fn serving(files: &Files, config: &str) -> Serving {
    let config = files.write("head.toml", config);
    let seed = files.write("seed.toml", SEED);
    Serving::start(&[
        "--config",
        &config.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ])
}

/// Two requests, one of which fails. Returns the node's stderr.
///
/// The failure is an unauthenticated call, chosen because it is rejected in
/// the authenticator — *inside* the service the layer wraps — so it is a real
/// round trip that produces a non-`OK` head rather than a connection that
/// never reached the server.
async fn serve_two_requests(serving: Serving) -> String {
    let mut client = connect(&serving).await;
    rows(&mut client, &APP, query("docs"))
        .await
        .expect("a query the node will answer");
    let refused = rows(&mut client, &Identity::nobody(), query("docs"))
        .await
        .expect_err("a request with no identity must be refused");
    assert_eq!(refused.code(), tonic::Code::Unauthenticated);
    // Dropped so the drain is a drain rather than a wait for the grace period.
    drop(client);

    let finished = serving.terminate();
    assert_eq!(
        finished.code,
        Some(0),
        "stderr:\n{}\nstdout:\n{}",
        finished.stderr,
        finished.stdout
    );
    finished.stderr
}

/// One HTTP/1.1 `GET`, returning the whole response including the head.
///
/// Hand-written for the reason the unit test's is: the request is one line,
/// the server closes the connection so the body ends at end-of-stream, and an
/// HTTP client dependency in a test suite that has none would be a build cost
/// for seventy bytes.
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
    // Bounded, and the bound is the point. A listener that is *bound* but
    // never accepted still completes the TCP handshake out of the kernel's
    // backlog, so `connect` and `write` both succeed and only the read hangs —
    // which is exactly what happens if nothing spawns the server. Without this
    // timeout that case is a test that never finishes rather than one that
    // fails, and a hanging test in CI is a six-hour job rather than a red
    // cross. Found by a mutation that removed the spawn: the suite stopped
    // reporting anything at all instead of reporting a failure.
    tokio::time::timeout(
        core::time::Duration::from_secs(20),
        socket.read_to_string(&mut answered),
    )
    .await
    .expect("the metrics endpoint should answer, not hang: nothing is serving the port")
    .expect("a response should be readable");
    answered
}

#[tokio::test]
async fn a_scrape_reports_what_the_node_served() {
    // Through the real binary and a real socket. The unit tests beside
    // `metrics.rs` cover the routing and the exposition text and would all
    // still pass if the listener were never spawned from `serve.rs` — which is
    // the wiring this is here for, and the same gap the summary's integration
    // test was written against.
    let files = Files::new();
    let mut serving = serving(
        &files,
        &talking("summary_interval = \"60s\"\nmetrics_address = \"127.0.0.1:0\""),
    );
    let address = serving
        .metrics_address()
        .expect("the node should announce its metrics port");

    let mut client = connect(&serving).await;
    rows(&mut client, &APP, query("docs"))
        .await
        .expect("a query the node will answer");
    let refused = rows(&mut client, &Identity::nobody(), query("docs"))
        .await
        .expect_err("a request with no identity must be refused");
    assert_eq!(refused.code(), tonic::Code::Unauthenticated);
    drop(client);

    let answered = scrape(&address, "/metrics").await;
    assert!(answered.starts_with("HTTP/1.1 200 OK"), "{answered}");
    // The content type is asserted here as well as in the unit test, because
    // this is the one that would catch the wrong handler being wired up.
    assert!(answered.contains("text/plain; version=0.0.4"), "{answered}");

    let query_method = "/slate.v1.Records/Query";
    assert!(
        answered.contains(&format!(
            "slate_requests_total{{method=\"{query_method}\"}} 2"
        )),
        "both calls should be counted:\n{answered}"
    );
    assert!(
        answered.contains(&format!(
            "slate_request_failures_total{{method=\"{query_method}\"}} 1"
        )),
        "the refusal should be counted as a failure:\n{answered}"
    );
    // `le="+Inf"` equals the call count, which is the invariant that says the
    // histogram and the counter beside it describe the same requests.
    assert!(
        answered.contains(&format!(
            "slate_request_head_seconds_bucket{{method=\"{query_method}\",le=\"+Inf\"}} 2"
        )),
        "{answered}"
    );

    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

#[tokio::test]
async fn a_node_with_no_metrics_address_serves_no_metrics_port() {
    // The default. Asserted because "off unless configured" is the whole
    // security argument for an endpoint with no authentication on it, and an
    // argument nothing checks is an argument that can stop being true.
    let files = Files::new();
    let mut serving = serving(&files, QUIET);
    assert_eq!(serving.metrics_address(), None);
    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

#[tokio::test]
async fn a_request_log_names_the_method_and_how_it_ended() {
    let files = Files::new();
    let stderr = serve_two_requests(serving(&files, &talking("request_log = true"))).await;

    // The method is the proto path, whole, so this is greppable for exactly
    // the string `records.proto` contains.
    assert!(
        stderr.contains("/slate.v1.Records/Query id=- status=0"),
        "the answered query should be logged as a success, with the id field:\n{stderr}"
    );
    // 16 is `UNAUTHENTICATED`. The *number* matters: a layer that logged
    // "failed" without the code would be useless for telling an auth failure
    // from a timeout, which is most of what somebody reads this log for.
    assert!(
        stderr.contains("/slate.v1.Records/Query id=- status=16"),
        "the refused query should be logged with its gRPC code:\n{stderr}"
    );
    // Both lines carry a duration, and it says `head` rather than `total`.
    let logged: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("status="))
        .collect();
    assert_eq!(logged.len(), 2, "one line per request:\n{stderr}");
    for line in &logged {
        assert!(line.contains("head="), "no duration on `{line}`");
        assert!(line.ends_with("ms"), "no unit on `{line}`");
    }
}

#[tokio::test]
async fn the_log_carries_the_id_the_caller_sent() {
    // The point of the id: a caller holding a failure can find the line the
    // server wrote for it. Asserted through a real socket rather than against
    // the filter, because the filter is unit-tested beside itself and what is
    // in doubt here is whether the header survives the transport, the
    // authenticator and the layer to reach the line.
    let files = Files::new();
    let serving = serving(&files, &talking("request_log = true"));
    let mut client = connect(&serving).await;

    let mut request = APP.on(proto::QueryRequest {
        transaction: String::new(),
        query: Some(query("docs")),
        ..Default::default()
    });
    request
        .metadata_mut()
        .insert("slate-request-id", "checkout-7f3a".parse().expect("ascii"));
    let mut stream = client.query(request).await.expect("a query").into_inner();
    while stream.message().await.expect("a message").is_some() {}
    drop(client);

    let stderr = serving.terminate().stderr;
    assert!(
        stderr.contains("/slate.v1.Records/Query id=checkout-7f3a status=0"),
        "the caller's id should be on the line for its call:\n{stderr}"
    );
}

#[tokio::test]
async fn a_caller_sending_no_id_gets_a_line_of_the_same_shape() {
    // `id=-` rather than a missing field. A log where the columns move
    // depending on what the caller sent is a log nothing can parse, and the
    // dash is a value a reader can see rather than a gap they have to infer.
    let files = Files::new();
    let stderr = serve_two_requests(serving(&files, &talking("request_log = true"))).await;
    for line in stderr.lines().filter(|line| line.contains("status=")) {
        assert!(line.contains(" id=- "), "no id field on `{line}`");
    }
}

#[tokio::test]
async fn a_summary_counts_what_the_node_served() {
    let files = Files::new();
    // Long enough that the ticker cannot fire during the test, so what this
    // reads is the *final* summary printed on the way out. Timing the ticker
    // instead would be a sleep in a test and a flake on a loaded machine.
    let stderr = serve_two_requests(serving(&files, &talking("summary_interval = \"1h\""))).await;

    let summary: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("calls="))
        .collect();
    assert_eq!(
        summary.len(),
        1,
        "one line for the one method called:\n{stderr}"
    );
    assert!(
        summary[0].contains("/slate.v1.Records/Query calls=2 failed=1 late=0"),
        "both calls counted and the refusal marked failed:\n{stderr}"
    );
    // `late=0` is load-bearing and not a formality. The successful call here
    // is a *streamed* read, so tonic ends it with a real `grpc-status: 0`
    // trailer — the only place in the suite where the trailer-reading wrapper
    // meets a trailer it did not build itself. Inverting its comparison turns
    // this into `late=1`, which is what says the wrapper is installed on the
    // real path and reads what tonic actually sends.
    // The quantiles reach the log line, which the unit tests cannot say: they
    // call `summary()` directly, so a `summary` that was never wired to what
    // the node prints would pass every one of them. The numbers themselves
    // are two local RPCs and are not asserted — what is asserted is that the
    // fields exist, in order, with a `<=` that says they are an upper bound.
    for field in ["p50_head<=", "p90_head<=", "p99_head<="] {
        assert!(summary[0].contains(field), "no `{field}` on:\n{stderr}");
    }
    let mean = summary[0].find("mean_head=").expect("a mean");
    let p50 = summary[0].find("p50_head<=").expect("a p50");
    let p99 = summary[0].find("p99_head<=").expect("a p99");
    let slowest = summary[0].find("slowest_head=").expect("a slowest");
    assert!(
        mean < p50 && p50 < p99 && p99 < slowest,
        "the fields should read low to high:\n{stderr}"
    );
    // A summary without `request_log` is the production shape: counters, no
    // line per request. Asserted so the two settings cannot silently merge.
    assert!(
        !stderr.contains("status="),
        "`summary_interval` alone should not log per request:\n{stderr}"
    );
}

#[tokio::test]
async fn the_summary_repeats_on_its_interval_while_the_node_runs() {
    // The only test here that involves waiting, and it is here because a
    // mutation proved it was needed: emptying the ticker's loop body left
    // every other test passing. They all read the *final* summary printed on
    // the way out, so a node whose periodic summary never fired looked
    // identical to one whose did — which is the whole feature.
    //
    // Told apart by counting: a one-hour interval yields exactly one summary
    // (the final one), so two or more means the ticker fired. The wait is
    // three intervals for an expectation of two lines, so this does not go red
    // on a machine that scheduled the task late.
    let files = Files::new();
    let serving = serving(&files, &talking("summary_interval = \"1s\""));
    let mut client = connect(&serving).await;
    rows(&mut client, &APP, query("docs"))
        .await
        .expect("a query the node will answer");
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    drop(client);

    let stderr = serving.terminate().stderr;
    let summaries = stderr
        .lines()
        .filter(|line| line.contains("calls="))
        .count();
    assert!(
        summaries >= 2,
        "a 1s summary over 3.2s should have printed more than the final one; saw {summaries}:\n{stderr}"
    );
    // And the first one is a summary of what had happened by then, not an
    // empty shell: `interval`'s first tick fires immediately and is consumed
    // on purpose, so no line should report a node that has served nothing.
    assert!(
        !stderr.contains("calls=0"),
        "a summary should not report a method with no calls:\n{stderr}"
    );
}

#[tokio::test]
async fn a_node_told_nothing_says_nothing() {
    let files = Files::new();
    let stderr = serve_two_requests(serving(&files, QUIET)).await;

    assert!(
        !stderr.contains("status="),
        "the request log is off by default:\n{stderr}"
    );
    assert!(
        !stderr.contains("calls="),
        "the summary is off by default:\n{stderr}"
    );
}

#[tokio::test]
async fn methods_are_counted_apart_over_the_wire() {
    // The unit test for this uses two names handed to `record`. This one
    // proves the *path* the layer reads really differs between two RPCs —
    // a layer keyed on something coarser (the connection, say) would pass
    // that unit test and fail this one.
    let files = Files::new();
    let serving = serving(&files, &talking("summary_interval = \"1h\""));
    let mut client = connect(&serving).await;

    rows(&mut client, &APP, query("docs"))
        .await
        .expect("a query");
    let found = client
        .get(APP.on(proto::GetRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            primary_key: Some(harness::row(vec![u64_value(1)])),
            freshness: None,
            ..Default::default()
        }))
        .await
        .expect("a get")
        .into_inner();
    assert!(found.found);
    drop(client);

    let stderr = serving.terminate().stderr;
    assert!(
        stderr.contains("/slate.v1.Records/Query calls=1 failed=0"),
        "{stderr}"
    );
    assert!(
        stderr.contains("/slate.v1.Records/Get calls=1 failed=0"),
        "{stderr}"
    );
}

#[tokio::test]
async fn a_zero_summary_interval_is_refused_at_startup() {
    // `tokio::time::interval` panics on a zero period, and it is spawned, so
    // the panic would land in a detached task: the node would serve on with no
    // summary and no explanation. A refusal names the field instead.
    let files = Files::new();
    let config = files.write("head.toml", &talking("summary_interval = \"0s\""));
    let finished = harness::run(&["--config", &config.display().to_string()]);

    assert_ne!(finished.code, Some(0), "it started:\n{}", finished.output());
    let said = finished.output();
    assert!(
        said.contains("summary_interval"),
        "the refusal should name the field:\n{said}"
    );
}
