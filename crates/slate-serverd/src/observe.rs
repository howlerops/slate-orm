//! One line per request, and counters that summarise them.
//!
//! The node logged its startup and its warnings and nothing in between, so the
//! only way to find out that a deployment was serving `Query` ten thousand
//! times a minute at a p99 of two seconds was to instrument the client. That
//! is recorded in three places — the README, the comparison's gap table and
//! its "smaller, and owed" list — which is what an owed item looks like.
//!
//! # Why a `tower` layer rather than a `tonic` interceptor
//!
//! An interceptor sees the request and not the response, so it can log that a
//! call *arrived* and not how it ended or how long it took — which is most of
//! what a request log is for. A layer wraps the whole call.
//!
//! # Why not `tracing`
//!
//! It is the obvious dependency and it is a bigger decision than this item.
//! `tracing` without a subscriber logs nothing, so adding it means also
//! choosing a subscriber, a format and a filter syntax, and every one of those
//! is a thing an operator then has to configure. This writes one line to
//! stderr in the same `slate-serverd: …` shape as every other line the process
//! emits, and an operator who wants structured logs pipes it somewhere. If
//! that becomes the constraint, `tracing` is the answer and this is one file
//! to replace.
//!
//! # What a streamed response means for the duration
//!
//! The duration recorded is to the *response head*, not to the last row. A
//! streaming read returns its head almost immediately and then streams for as
//! long as the client keeps reading, so timing to the end would measure the
//! client's appetite rather than the server's work — and a slow consumer would
//! read as a slow server. `rows_per_message` and the client's own timing are
//! where the rest of that lives. The log line says `head` so this is not
//! mistaken for the whole call.

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What to record, and how loudly.
///
/// No `Default`, deliberately: the default lives in `config::Observability`'s
/// `#[serde(default)]`, where an absent `[observability]` section is read, and
/// a second one here would be a second place for it to drift from. Both
/// settings are off when the section is absent — a node that starts logging
/// differently because it was upgraded is a surprise, and a line per request
/// on a node serving ten thousand a second is a hundred megabytes an hour of
/// stderr nobody asked for. The failure mode of logging by default is a full
/// disk rather than a missing log.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Observing {
    /// Emit a line per request: method, gRPC status, time to the head.
    pub(crate) request_log: bool,
    /// Emit a counter summary this often, or never when `None`.
    ///
    /// Separate from `request_log` on purpose: the summary is cheap at any
    /// request rate and is what a node should have on in production, where the
    /// per-request line is a debugging tool.
    pub(crate) summary: Option<Duration>,
}

/// Counters for one method.
#[derive(Debug, Default)]
struct Method {
    /// Calls that reached a response head, whatever its status.
    calls: AtomicU64,
    /// Of those, the ones whose head carried a non-`OK` gRPC status.
    failures: AtomicU64,
    /// Total microseconds to the response head, for a mean.
    micros: AtomicU64,
    /// The slowest head, in microseconds.
    slowest: AtomicU64,
}

/// Every method's counters.
///
/// A `Mutex<BTreeMap>` rather than a lock-free map: the map is only written
/// when a *new method name* appears, which is at most eighteen times in a
/// process's life, and every request after that takes the lock for the length
/// of a lookup. Sorted so the summary reads the same way twice.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    methods: Mutex<BTreeMap<String, Arc<Method>>>,
}

impl Counters {
    /// Record one finished call.
    fn record(&self, method: &str, elapsed: Duration, failed: bool) {
        let entry = {
            let mut methods = self.methods.lock().unwrap_or_else(|poisoned| {
                // A poisoned lock means a panic while holding it, which can
                // only be an allocation failure here — nothing in the guarded
                // section can panic on its own. Losing counters is not worth
                // taking the process down for.
                poisoned.into_inner()
            });
            Arc::clone(methods.entry(method.to_owned()).or_default())
        };
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        entry.calls.fetch_add(1, Ordering::Relaxed);
        if failed {
            entry.failures.fetch_add(1, Ordering::Relaxed);
        }
        entry.micros.fetch_add(micros, Ordering::Relaxed);
        entry.slowest.fetch_max(micros, Ordering::Relaxed);
    }

    /// One line per method that has been called, or `None` when none has.
    ///
    /// Returns the text rather than printing it so a test can read it without
    /// capturing stderr, which is the difference between a test that asserts
    /// the numbers and a test that asserts something was written.
    pub(crate) fn summary(&self) -> Option<String> {
        let methods = self
            .methods
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if methods.is_empty() {
            return None;
        }
        let mut out = String::new();
        for (name, counters) in methods.iter() {
            let calls = counters.calls.load(Ordering::Relaxed);
            if calls == 0 {
                continue;
            }
            let failures = counters.failures.load(Ordering::Relaxed);
            let total = counters.micros.load(Ordering::Relaxed);
            let slowest = counters.slowest.load(Ordering::Relaxed);
            // `write!` to a String cannot fail; the result is discarded rather
            // than unwrapped so this cannot be the thing that kills a node.
            let _ = writeln!(
                out,
                "slate-serverd: {name} calls={calls} failed={failures} \
                 mean_head={:.1}ms slowest_head={:.1}ms",
                total as f64 / calls as f64 / 1000.0,
                slowest as f64 / 1000.0,
            );
        }
        (!out.is_empty()).then_some(out)
    }
}

/// Wraps a service so every call is timed and counted.
#[derive(Debug, Clone)]
pub(crate) struct Observe<S> {
    inner: S,
    counters: Arc<Counters>,
    request_log: bool,
}

impl<S> Observe<S> {
    pub(crate) const fn new(inner: S, counters: Arc<Counters>, request_log: bool) -> Self {
        Self {
            inner,
            counters,
            request_log,
        }
    }
}

/// The gRPC status a response head carries, if any.
///
/// gRPC puts the status in a *header* on a failure that happens before the
/// body, and in a trailer otherwise — so a missing `grpc-status` here is the
/// ordinary success case rather than a problem, and a present one that is not
/// `"0"` is a failure this layer can see.
///
/// A failure that appears only in a *trailer* — one raised after the head was
/// sent, which for this server means a streamed read that died part way — is
/// invisible here and counts as a success. Said out loud rather than left for
/// somebody to discover from a `failed=` that looks too low. Catching those
/// would mean wrapping the response body and inspecting its trailers, which is
/// a per-row cost on the streaming path to correct a count; the client's own
/// error is where that failure is visible today.
fn head_status(headers: &http::HeaderMap) -> Option<&str> {
    headers.get("grpc-status")?.to_str().ok()
}

impl<S, B> tower::Service<http::Request<B>> for Observe<S>
where
    S: tower::Service<http::Request<B>, Response = http::Response<tonic::body::Body>>,
    S::Future: Send + 'static,
    S::Error: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn core::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        // The gRPC method is the request path: `/slate.v1.Records/Query`. Kept
        // whole rather than split, so a log line can be grepped for the exact
        // string a proto file contains.
        let method = request.uri().path().to_owned();
        let started = Instant::now();
        let counters = Arc::clone(&self.counters);
        let request_log = self.request_log;
        let call = self.inner.call(request);
        Box::pin(async move {
            let outcome = call.await;
            let elapsed = started.elapsed();
            let status = match &outcome {
                Ok(response) => head_status(response.headers()).unwrap_or("0").to_owned(),
                // A transport-level error never became a gRPC response at all.
                Err(_) => "transport".to_owned(),
            };
            let failed = status != "0";
            counters.record(&method, elapsed, failed);
            if request_log {
                eprintln!(
                    "slate-serverd: {method} status={status} head={:.1}ms",
                    elapsed.as_secs_f64() * 1000.0
                );
            }
            outcome
        })
    }
}

/// A `tower` layer producing [`Observe`].
#[derive(Debug, Clone)]
pub(crate) struct ObserveLayer {
    counters: Arc<Counters>,
    request_log: bool,
}

impl ObserveLayer {
    pub(crate) const fn new(counters: Arc<Counters>, request_log: bool) -> Self {
        Self {
            counters,
            request_log,
        }
    }
}

impl<S> tower::Layer<S> for ObserveLayer {
    type Service = Observe<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Observe::new(inner, Arc::clone(&self.counters), self.request_log)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_reports_calls_failures_and_timing() {
        let counters = Counters::default();
        counters.record("/slate.v1.Records/Query", Duration::from_millis(2), false);
        counters.record("/slate.v1.Records/Query", Duration::from_millis(4), true);
        let summary = counters.summary().expect("a method was called");
        assert!(summary.contains("calls=2"), "{summary}");
        assert!(summary.contains("failed=1"), "{summary}");
        // 2 ms and 4 ms is a 3 ms mean and a 4 ms slowest. Asserted rather
        // than eyeballed: a mean that divided by the wrong count would still
        // look plausible.
        assert!(summary.contains("mean_head=3.0ms"), "{summary}");
        assert!(summary.contains("slowest_head=4.0ms"), "{summary}");
    }

    #[test]
    fn nothing_called_is_no_summary_rather_than_an_empty_one() {
        // A node that served nothing should print nothing, not a header with
        // no rows under it — an empty summary in a log reads as a bug.
        assert!(Counters::default().summary().is_none());
    }

    #[test]
    fn methods_are_counted_apart() {
        let counters = Counters::default();
        counters.record("/slate.v1.Records/Query", Duration::from_millis(1), false);
        counters.record("/slate.v1.Records/Insert", Duration::from_millis(1), false);
        let summary = counters.summary().expect("two methods were called");
        assert_eq!(summary.lines().count(), 2, "{summary}");
        assert!(summary.contains("Query calls=1"), "{summary}");
        assert!(summary.contains("Insert calls=1"), "{summary}");
    }

    #[test]
    fn an_ok_head_is_not_a_failure_and_a_missing_one_is_not_either() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(head_status(&headers), None, "no grpc-status is success");
        headers.insert("grpc-status", http::HeaderValue::from_static("0"));
        assert_eq!(head_status(&headers), Some("0"));
        headers.insert("grpc-status", http::HeaderValue::from_static("7"));
        assert_eq!(head_status(&headers), Some("7"));
    }
}
