//! The benchmarks in this crate are run by hand, so nothing notices when they
//! stop working.
//!
//! Three of the five did stop working, and stayed broken for as long as nobody
//! ran one. `leadership` used to take `_request` and answer anybody who could
//! reach the port; the fix gave it the same `self.context(&request)` every
//! other handler has, and every benchmark that warmed its channel with a bare
//! `LeadershipRequest` began panicking on `UNAUTHENTICATED` at the first call.
//! `ci.yml` builds this crate and runs none of it, so `cargo clippy
//! --all-targets` kept saying the examples compiled — which was true, and not
//! the question.
//!
//! These two tests are the smallest thing that would have caught *that*, and
//! they run under `cargo test --workspace` like everything else. They are not
//! the smallest thing that would have caught the next one, and saying so cost
//! nothing at the time and turned out to be the important sentence: a fourth
//! benchmark was broken by `EXPLAIN` becoming privileged, both tests passed
//! over it, and only running `head_report` found it. `run.sh --smoke` does
//! that now, in CI, and is what actually guards this crate — these two stay
//! because they are seconds rather than minutes and they name the specific
//! shape rather than reporting an exit code.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use slate_headbench::fixture::{catalog, principal_request, security};
use slate_headbench::harness::{leading, memory, serve};
use slate_server::proto as pb;
use std::path::Path;

/// The harness can talk to the server the harness stood up.
///
/// A round trip and nothing else. It is deliberately not a measurement: what
/// broke was never a number, it was the call.
#[tokio::test]
async fn the_harness_client_can_reach_the_harness_server() {
    let head = slate_headbench::harness::head(
        catalog(),
        security(),
        memory(),
        Vec::new(),
        leading().await,
        slate_server::Limits::default(),
    );
    let serving = serve(head).await;
    let mut client = serving.client().await;
    let answer = client
        .leadership(principal_request(pb::LeadershipRequest {}, 1))
        .await
        .expect("an authenticated leadership call is answered");
    assert!(
        answer.into_inner().standing != 0,
        "the node reported no standing at all"
    );
}

/// No example builds a request without an identity on it.
///
/// A source check rather than a run, because running five benchmarks is
/// minutes and this is the property that actually broke: `tonic::Request::new`
/// makes a request with no metadata, and every RPC on this service now
/// authenticates. `fixture::principal_request` is the sanctioned form and the
/// only one the examples use.
///
/// This cannot see an example that authenticates *wrongly* — a bad principal,
/// a role with no grant — and it is not meant to. It sees the shape that broke
/// three files at once, which is the shape a sixth benchmark would copy from
/// the five beside it.
#[test]
fn no_example_sends_a_request_with_no_identity() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut bare = Vec::new();
    let mut read = 0;
    for entry in std::fs::read_dir(&examples).expect("the examples directory is there") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|kind| kind != "rs") {
            continue;
        }
        read += 1;
        let body = std::fs::read_to_string(&path).expect("an example source");
        for (at, line) in body.lines().enumerate() {
            if line.contains("tonic::Request::new(pb::") {
                bare.push(format!(
                    "{}:{}: {}",
                    path.file_name().expect("a file name").to_string_lossy(),
                    at + 1,
                    line.trim()
                ));
            }
        }
    }
    // The never-fires half. A rename of the directory, or a build that runs
    // this from somewhere else, would leave the loop reading nothing and this
    // test reporting success over a tree it never looked at — which is the
    // failure mode `CLAUDE.md` names and the one this whole file is about.
    assert!(
        read >= 5,
        "read {read} example sources in {}; this check is looking in the wrong place",
        examples.display()
    );
    assert!(
        bare.is_empty(),
        "these build a request with no identity metadata, and every RPC on this \
         service authenticates — use `fixture::principal_request`:\n  {}",
        bare.join("\n  ")
    );
}
