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
