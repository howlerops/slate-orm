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

/// How many sub-buckets each octave of the latency histogram is cut into.
///
/// Three bits is eight sub-buckets, so a bucket is at most an eighth wider
/// than its own floor and a reported quantile is within +12.5% of the true
/// one, never below it. Two bits (25%) cannot tell 80 ms from 100 ms, which is
/// the distinction somebody reading a p99 is usually making; four (6.25%)
/// doubles the array to report a precision the sample counts on a quiet node
/// do not support.
const SUB_BITS: u32 = 3;

/// Sub-buckets per octave.
const SUB: u64 = 1 << SUB_BITS;

/// Enough buckets for every `u64` microsecond value, so nothing is clamped.
///
/// The largest index [`bucket_of`] can return, plus one. Clamping the top
/// instead would save 8 bytes a method and put a silent ceiling on what the
/// histogram can say — and the number it would put a ceiling on is exactly the
/// pathological latency somebody turned the summary on to find.
const BUCKETS: usize = 496;

/// Which bucket a duration falls in.
///
/// Log-linear: below [`SUB`] microseconds each value is its own bucket and the
/// answer is exact; above it, the index is the octave and the top [`SUB_BITS`]
/// bits below it. Monotone and gap-free across the boundary, which
/// `the_buckets_are_monotone_and_gapless` checks rather than asserts.
fn bucket_of(micros: u64) -> usize {
    if micros < SUB {
        // Exact, and the arm that makes the boundary work: index 7 is the
        // value 7, and index 8 is the first value of the first octave.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "micros < SUB = 8, so this fits"
        )]
        return micros as usize;
    }
    let octave = u64::from(63 - micros.leading_zeros());
    let shift = octave - u64::from(SUB_BITS);
    let sub = (micros >> shift) & (SUB - 1);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "octave <= 63, so the index is at most 495"
    )]
    let index = ((octave - u64::from(SUB_BITS) + 1) * SUB + sub) as usize;
    index
}

/// The largest duration a bucket holds.
///
/// A quantile is reported as this rather than as the bucket's floor, so the
/// number is an *upper* bound on the true one. A latency that is reported too
/// low is the failure mode worth avoiding: it is the one that reads as "this
/// is fine".
fn bucket_ceiling(index: usize) -> u64 {
    let index = index as u64;
    if index < SUB {
        return index;
    }
    let octave = index / SUB + u64::from(SUB_BITS) - 1;
    let sub = index % SUB;
    let shift = octave - u64::from(SUB_BITS);
    let floor = (SUB + sub) << shift;
    // `saturating` rather than a wider type: the top bucket's ceiling is 2^64,
    // which is not a `u64`, and it is a bucket no request head will ever
    // reach. Saturating there is a better answer than a panic in a logging
    // path.
    floor.saturating_add(1 << shift).saturating_sub(1)
}

/// Counters for one method.
#[derive(Debug)]
struct Method {
    /// Calls that reached a response head, whatever its status.
    calls: AtomicU64,
    /// Of those, the ones whose head carried a non-`OK` gRPC status.
    failures: AtomicU64,
    /// Total microseconds to the response head, for a mean.
    micros: AtomicU64,
    /// The slowest head, in microseconds.
    slowest: AtomicU64,
    /// Every head's duration, bucketed, for the quantiles.
    ///
    /// A `Box<[AtomicU64]>` rather than a `Mutex<Vec<u64>>` of every sample:
    /// keeping the samples would give exact quantiles and unbounded memory on
    /// a node that serves for a week, and would put a lock on the path every
    /// request already takes. Bucketed counts are a fixed 4 KiB a method and
    /// two relaxed atomic operations.
    heads: Box<[AtomicU64]>,
}

impl Default for Method {
    fn default() -> Self {
        Self {
            calls: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            micros: AtomicU64::new(0),
            slowest: AtomicU64::new(0),
            heads: (0..BUCKETS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

impl Method {
    /// The `numerator`/`denominator` quantile of the recorded heads, in
    /// microseconds, or `None` when nothing has been recorded.
    ///
    /// Nearest-rank: the value of the `ceil(q * n)`-th sample in order, which
    /// is the definition that needs no interpolation between two buckets whose
    /// contents it cannot see. Clamped to the exact `slowest`, because a
    /// bucket's ceiling can sit above every sample in it and a p99 printed
    /// above the observed maximum reads as a bug in the summary rather than as
    /// the rounding it is.
    fn quantile(&self, numerator: u64, denominator: u64) -> Option<u64> {
        let total = self.calls.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        // Saturating: `total * numerator` is a request count times 99, which
        // needs 2^57 requests to overflow, but a logging path that can panic
        // on a busy node is not worth the two characters saved.
        let rank = total.saturating_mul(numerator).div_ceil(denominator).max(1);
        let mut seen = 0u64;
        for (index, bucket) in self.heads.iter().enumerate() {
            seen = seen.saturating_add(bucket.load(Ordering::Relaxed));
            if seen >= rank {
                return Some(bucket_ceiling(index).min(self.slowest.load(Ordering::Relaxed)));
            }
        }
        // Only reachable if a concurrent `record` landed between the `calls`
        // read and the sweep, so the buckets hold fewer samples than `total`
        // claimed. The slowest is the honest answer to "the largest sample" in
        // that case, and it is never wrong by more than one request's worth.
        Some(self.slowest.load(Ordering::Relaxed))
    }
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
        // `get` rather than an index: `bucket_of` cannot exceed `BUCKETS`, and
        // a panic here would take down a node over a log line if it ever did.
        if let Some(bucket) = entry.heads.get(bucket_of(micros)) {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
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
            // Every quantile is `Some` here, because `calls` is non-zero;
            // `unwrap_or(slowest)` rather than an `expect` so a logging path
            // has no way to panic at all.
            let at = |numerator| counters.quantile(numerator, 100).unwrap_or(slowest);
            let _ = writeln!(
                out,
                "slate-serverd: {name} calls={calls} failed={failures} \
                 mean_head={:.1}ms p50_head<={:.1}ms p90_head<={:.1}ms \
                 p99_head<={:.1}ms slowest_head={:.1}ms",
                total as f64 / calls as f64 / 1000.0,
                at(50) as f64 / 1000.0,
                at(90) as f64 / 1000.0,
                at(99) as f64 / 1000.0,
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

/// How much of a caller's request id is kept.
///
/// A UUID is 36 characters and every client here sends one; 64 leaves room for
/// a caller's own scheme without letting one line of log become a page of it.
const REQUEST_ID_LIMIT: usize = 64;

/// The caller's id for this request, fit to go in a log line.
///
/// **This is attacker-controlled text on its way into a log**, which is the
/// whole of why it is not simply printed. Anyone who can reach this server can
/// choose the value, and the log is a line-oriented file somebody greps and
/// something else may parse. A value containing a newline would let a caller
/// write log lines of their own — an entry claiming a different method, a
/// different status, a different id — and forged lines in an audit trail are
/// worse than no audit trail, because they are believed.
///
/// So the filter is an allowlist and not a blocklist: `[A-Za-z0-9._:-]`, which
/// covers a UUID, a hex span id, a ULID and a `service/1234` style label, and
/// admits no whitespace, no control character and no quote. Anything else is
/// dropped rather than escaped, because an escape is a second encoding for a
/// reader to get wrong and there is no value in round-tripping a label nobody
/// but its sender chose.
///
/// `None` when the header is absent, unreadable as ASCII, or has nothing left
/// after filtering — all three mean the same thing to a log line, which is
/// that this call carries no usable id.
fn request_id(headers: &http::HeaderMap) -> Option<String> {
    let raw = headers.get(slate_server::REQUEST_ID_KEY)?.to_str().ok()?;
    let kept: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
        .take(REQUEST_ID_LIMIT)
        .collect();
    (!kept.is_empty()).then_some(kept)
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
        let request_log = self.request_log;
        // The gRPC method is the request path: `/slate.v1.Records/Query`. Kept
        // whole rather than split, so a log line can be grepped for the exact
        // string a proto file contains.
        let method = request.uri().path().to_owned();
        // Taken before the call, because the request is moved into it. Only
        // when `request_log` is on: this allocates and filters a caller's
        // string, and doing that per request to then throw it away is the kind
        // of cost that is invisible until somebody profiles.
        let id = request_log.then(|| request_id(request.headers())).flatten();
        let started = Instant::now();
        let counters = Arc::clone(&self.counters);
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
                // `id=-` rather than omitting the field, so every line has the
                // same shape and `awk`ing a column does not silently read the
                // next one on the calls that carried no id.
                eprintln!(
                    "slate-serverd: {method} id={} status={status} head={:.1}ms",
                    id.as_deref().unwrap_or("-"),
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

    /// The bucket boundaries are checked rather than trusted: a log-linear
    /// index that skips a value or goes backwards at an octave boundary is the
    /// classic defect in this shape, and it is invisible in a quantile because
    /// the answer still looks like a latency.
    #[test]
    fn the_buckets_are_monotone_and_gapless() {
        let mut previous = bucket_of(0);
        assert_eq!(previous, 0);
        // Exhaustive over the first four octaves and then over every value
        // within eight either side of every power of two, which is where a
        // boundary can be. Exhaustive to 2^63 is not a test, it is a job.
        let mut probes: Vec<u64> = (0..256).collect();
        for power in 8..64u32 {
            let at = 1u64 << power;
            probes.extend((at - 8)..=(at + 8));
        }
        probes.sort_unstable();
        probes.dedup();
        for micros in probes {
            let index = bucket_of(micros);
            assert!(index < BUCKETS, "{micros} fell outside the array: {index}");
            // Monotone. Gapless is the pair of ceiling checks below, which
            // say exactly "this is the one bucket whose range holds this
            // value" — a stronger statement than any step rule, and one the
            // sparse probes above 256 cannot break.
            assert!(index >= previous, "{micros} went backwards to {index}");
            // The bucket a value falls in must be able to hold it.
            assert!(
                bucket_ceiling(index) >= micros,
                "bucket {index} holds {micros} but its ceiling is {}",
                bucket_ceiling(index)
            );
            // And the bucket below it must not.
            if index > 0 {
                assert!(
                    bucket_ceiling(index - 1) < micros,
                    "{micros} belongs one bucket lower"
                );
            }
            previous = previous.max(index);
        }
        assert_eq!(
            bucket_of(u64::MAX),
            BUCKETS - 1,
            "the array is exactly big enough and no bigger"
        );
    }

    /// The width claim in `SUB_BITS`'s own doc comment, checked.
    ///
    /// Written because "within +12.5%" is the sentence an operator reads a p99
    /// through, and a number in a comment that nothing checks is a number that
    /// drifts when somebody changes `SUB_BITS`.
    #[test]
    fn a_bucket_is_never_more_than_an_eighth_wider_than_its_floor() {
        // From `SUB` up: below it every bucket holds exactly one value, so
        // the error there is zero and the ratio below is not what says so.
        for index in (SUB as usize)..BUCKETS {
            let floor = bucket_ceiling(index - 1) + 1;
            let ceiling = bucket_ceiling(index);
            if ceiling == u64::MAX - 1 {
                continue; // The saturating top bucket, which has no true width.
            }
            let width = ceiling - floor + 1;
            assert!(
                width * SUB <= floor,
                "bucket {index} spans {floor}..={ceiling}, wider than an eighth of its floor"
            );
        }
    }

    /// One field's milliseconds, read back out of a summary line.
    ///
    /// The tests below go through `record` and `summary` rather than calling
    /// `Method::quantile` and filling buckets by hand. That is not fussiness:
    /// the first version of them wrote buckets directly, and a mutation
    /// removing the bucket increment from `record` altogether **survived** —
    /// every quantile fell through to the `slowest` fallback, which on those
    /// samples was a plausible number. A test that builds the state it then
    /// measures is testing arithmetic, not a feature.
    fn field(summary: &str, name: &str) -> f64 {
        let rest = summary
            .split_once(name)
            .unwrap_or_else(|| panic!("no `{name}` in:\n{summary}"))
            .1;
        let millis = rest
            .split_once("ms")
            .unwrap_or_else(|| panic!("`{name}` has no unit in:\n{summary}"))
            .0;
        millis
            .parse()
            .unwrap_or_else(|_| panic!("`{name}` is not a number: {millis:?}"))
    }

    #[test]
    fn a_quantile_is_the_nearest_rank_and_never_under_the_truth() {
        let counters = Counters::default();
        // A hundred samples at 1..=100 ms, so every quantile has a known
        // answer and the bucketing is the only thing that can move it.
        for millis in 1..=100u64 {
            counters.record("/x", Duration::from_millis(millis), false);
        }
        let summary = counters.summary().expect("a method was called");
        for (name, truth) in [
            ("p50_head<=", 50.0f64),
            ("p90_head<=", 90.0),
            ("p99_head<=", 99.0),
        ] {
            let reported = field(&summary, name);
            assert!(
                reported >= truth,
                "{name} reported {reported}ms, under the true {truth}ms:\n{summary}"
            );
            assert!(
                reported <= truth * 1.125,
                "{name} reported {reported}ms, more than an eighth over {truth}ms:\n{summary}"
            );
        }
    }

    /// The rank is the `ceil(q * n)`-th sample and not the one after it.
    ///
    /// Written because a mutation flipping `>=` to `>` in the sweep survived
    /// the even spread above: one sample either way is well inside the 12.5%
    /// the bucketing already costs, so the spread cannot see the difference.
    /// A distribution with a cliff in it can. Ninety-nine samples at 1 ms and
    /// one at a second: the true p99 is the 99th sample, which is 1 ms, and an
    /// answer one sample later is a *thousand* times bigger.
    #[test]
    fn a_quantile_falls_on_the_rank_and_not_one_past_it() {
        let counters = Counters::default();
        for _ in 0..99 {
            counters.record("/x", Duration::from_millis(1), false);
        }
        counters.record("/x", Duration::from_secs(1), false);
        let summary = counters.summary().expect("a method was called");
        assert!(
            field(&summary, "p99_head<=") <= 1.2,
            "the p99 of these is the 99th sample, which is 1ms:\n{summary}"
        );
        // And the outlier is not lost — it is what `slowest` is for, and a p99
        // that reported it would be the mutation this test exists for.
        assert!(
            (field(&summary, "slowest_head=") - 1000.0).abs() < 0.1,
            "{summary}"
        );
    }

    #[test]
    fn a_quantile_never_exceeds_the_observed_slowest() {
        // The clamp, and the reason for it: 4 ms lands in a bucket whose
        // ceiling is 4.095 ms, so an unclamped p99 of these two samples would
        // print above a `slowest_head` computed exactly — which reads as a bug
        // in the summary rather than as the rounding it is.
        let counters = Counters::default();
        counters.record("/x", Duration::from_millis(2), false);
        counters.record("/x", Duration::from_millis(4), false);
        let summary = counters.summary().expect("a method was called");
        assert!(summary.contains("p99_head<=4.0ms"), "{summary}");
        assert!(summary.contains("slowest_head=4.0ms"), "{summary}");
    }

    #[test]
    fn a_method_with_no_calls_has_no_quantile() {
        assert_eq!(Method::default().quantile(50, 100), None);
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

    /// A header map carrying one request id, built from raw bytes.
    ///
    /// `from_bytes` rather than `from_static`, because half of what is being
    /// tested is what happens to values a well-behaved client would never
    /// send, and `HeaderValue` refuses some of those at construction — which
    /// is itself part of the answer.
    fn with_id(raw: &[u8]) -> Option<http::HeaderMap> {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            slate_server::REQUEST_ID_KEY,
            http::HeaderValue::from_bytes(raw).ok()?,
        );
        Some(headers)
    }

    #[test]
    fn an_ordinary_id_survives_whole() {
        let headers = with_id(b"3f2504e0-4f89-11d3-9a0c-0305e82c3301").expect("a valid header");
        assert_eq!(
            request_id(&headers).as_deref(),
            Some("3f2504e0-4f89-11d3-9a0c-0305e82c3301")
        );
        // And the other shapes the allowlist exists to admit.
        for shape in ["01JB2Q", "checkout:1234", "web.api_7", "a-b.c:d_e"] {
            let headers = with_id(shape.as_bytes()).expect("a valid header");
            assert_eq!(request_id(&headers).as_deref(), Some(shape), "{shape}");
        }
    }

    #[test]
    fn a_caller_cannot_forge_fields_in_a_log_line() {
        // The reason this filter exists, and the shape of the threat was
        // measured rather than assumed. `HeaderValue::from_bytes` refuses
        // CR, LF, FF, VT, NUL and DEL outright, so a caller cannot break the
        // *line* — that much the transport already gives us, and it is
        // asserted here so that a change to it is a failing test rather than a
        // silent loss of the guarantee.
        for line_break in [b"a\nb".as_slice(), b"a\rb", b"a\x0cb", b"a\x0bb", b"a\x00b"] {
            assert!(
                with_id(line_break).is_none(),
                "the transport should refuse {line_break:?}"
            );
        }

        // What it does *not* refuse is the interesting half, and it was a
        // surprise: space, `=`, `"` and `'` all construct fine. So a caller
        // can forge *fields* even though they cannot forge lines — an id of
        // `x status=0 head=0.0ms` would give a reader, and anything splitting
        // this line on whitespace, two `status=` to choose between. That is
        // the attack this filter actually stops, and it is the one that would
        // have survived a filter written against newlines alone.
        let headers = with_id(b"x status=0 head=0.0ms").expect("the transport allows this");
        assert_eq!(
            request_id(&headers).as_deref(),
            Some("xstatus0head0.0ms"),
            "the separators must not survive"
        );
        for forged in ["\"", "'", " ", "="] {
            assert!(
                !request_id(&headers).expect("an id").contains(forged),
                "{forged:?} survived the filter"
            );
        }

        // A high byte constructs as a header and is not ASCII, so `to_str`
        // rejects it before the filter is reached. Same outcome by a different
        // route, and asserted so that route is known to be covered.
        let headers = with_id(b"caf\xc3\xa9").expect("the transport allows this");
        assert_eq!(request_id(&headers), None, "a non-ASCII value has no id");
    }

    #[test]
    fn a_long_id_is_cut_rather_than_refused() {
        // Cut and kept, not dropped: a caller with a verbose scheme still gets
        // a correlatable prefix, and one line of log cannot become a page of
        // it. Refusing outright would lose the correlation entirely over a
        // formatting opinion.
        let headers = with_id(&b"a".repeat(500)).expect("a valid header");
        let id = request_id(&headers).expect("a long id is still an id");
        assert_eq!(id.len(), REQUEST_ID_LIMIT);
    }

    #[test]
    fn an_absent_or_unusable_id_is_none_rather_than_empty() {
        // Three different causes, one meaning: this call carries no id. The
        // log line prints `-` for all of them.
        assert_eq!(request_id(&http::HeaderMap::new()), None, "absent");
        let headers = with_id(b"!!!").expect("a valid header");
        assert_eq!(request_id(&headers), None, "nothing survives the filter");
        let headers = with_id(b"").expect("an empty header value is legal");
        assert_eq!(request_id(&headers), None, "empty");
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
