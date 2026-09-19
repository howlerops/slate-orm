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
use http_body::Body as _;
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
// No longer `Copy` or `Clone`: `metrics` holds a bound listener, which is a
// resource and not a setting. That is the right constraint rather than an
// inconvenience — two copies of this struct would be two owners of one socket,
// and the compiler saying so is cheaper than finding out at runtime.
#[derive(Debug)]
pub(crate) struct Observing {
    /// Emit a line per request: method, gRPC status, time to the head.
    pub(crate) request_log: bool,
    /// Emit a counter summary this often, or never when `None`.
    ///
    /// Separate from `request_log` on purpose: the summary is cheap at any
    /// request rate and is what a node should have on in production, where the
    /// per-request line is a debugging tool.
    pub(crate) summary: Option<Duration>,
    /// A bound listener for `/metrics`, or `None` when none was asked for.
    ///
    /// The *listener* rather than the address, for the reason the gRPC one is
    /// bound before the banner prints: a scraper that reads the banner and
    /// connects immediately finds a socket already accepting, and a failure to
    /// bind is a startup failure rather than a dashboard that is empty for a
    /// reason nobody can see.
    pub(crate) metrics: Option<tokio::net::TcpListener>,
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
///
/// `pub(crate)` only because `Counters::record` hands one back and `metrics.rs`
/// records a call to check the endpoint serves the right counters. Nothing
/// outside `observe` reads a field.
#[derive(Debug)]
pub(crate) struct Method {
    /// Calls that reached a response head, whatever its status.
    calls: AtomicU64,
    /// Of those, the ones that failed — at the head or in a trailer.
    failures: AtomicU64,
    /// The subset of `failures` that arrived *after* the response head.
    ///
    /// Counted apart because the two are different operational problems and a
    /// single number cannot tell them apart. A head failure is a call the
    /// server refused: the caller got nothing and knows it. A late failure is
    /// a read that started answering and then died — the caller got rows, and
    /// whether it noticed depends on whether it checked the end of the stream.
    /// A node whose `failed` is all `late` is failing in the middle of scans,
    /// which points at storage rather than at the requests.
    late: AtomicU64,
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
            late: AtomicU64::new(0),
            micros: AtomicU64::new(0),
            slowest: AtomicU64::new(0),
            heads: (0..BUCKETS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

impl Method {
    /// Record a failure that arrived after this call's response head.
    ///
    /// Counted in `failures` as well as `late`: `late` is a *subset*, so a
    /// reader adding the two would double-count and one comparing them would
    /// find `late` larger than `failed` on a node whose only failures were
    /// late ones.
    fn record_late_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        self.late.fetch_add(1, Ordering::Relaxed);
    }

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

/// How many distinct method names the counters will hold.
///
/// **The key is the request path, and a caller chooses it.** The layer wraps
/// the router, so a request for a method that does not exist still reaches
/// here and is counted under whatever path it asked for — which means an
/// unbounded map fed by anyone who can open a connection. That was true before
/// the histogram and cheap (a name and four counters); with 4 KiB of buckets
/// behind every entry it is 80 times worse, and a memory bound that only held
/// because each entry was small is not a bound.
///
/// Nineteen RPCs exist, so 64 is headroom rather than a limit anybody reaches,
/// and it caps the counters at about 256 KiB. Past it, calls go to
/// [`OVERFLOW`] rather than being dropped: a node under this treatment has a
/// summary that has stopped naming things, and a line that says so is better
/// than a count that quietly stops moving.
const MAX_METHODS: usize = 64;

/// Where calls past [`MAX_METHODS`] distinct paths are counted.
///
/// Parenthesised because a real gRPC path begins with `/`, so this cannot
/// collide with one however a caller spells it.
const OVERFLOW: &str = "(other)";

/// Every method's counters.
///
/// A `Mutex<BTreeMap>` rather than a lock-free map: the map is only written
/// when a *new method name* appears, which is at most [`MAX_METHODS`] times in
/// a process's life, and every request after that takes the lock for the
/// length of a lookup. Sorted so the summary reads the same way twice.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    methods: Mutex<BTreeMap<String, Arc<Method>>>,
    /// Rows a standalone write touched, keyed on (statement, table).
    ///
    /// Separate from `methods` because it is a different question with a
    /// different key. "How many purges ran" is already a method counter; "how
    /// many rows did they erase" is the one an operator watching a retention
    /// sweep actually needs, and a sweep that runs nightly and erases nothing
    /// looks identical to a working one in every counter that existed before.
    ///
    /// Both halves of the key are bounded — the statement by an enum, the
    /// table by the catalog — so unlike the method map this needs no overflow
    /// bucket. `MAX_METHODS` exists because a method name arrives from the
    /// wire; neither of these does.
    rows: Mutex<BTreeMap<(&'static str, String), Arc<AtomicU64>>>,
}

/// The head node reports its writes straight into the scrape counters.
///
/// A trait implementation rather than a closure passed in, because the head
/// holds it for the process's life and `Arc<dyn Trait>` is what that wants;
/// and on `Counters` rather than a wrapper, because the wrapper would hold
/// exactly one field and forward one method.
impl slate_server::WriteObserver for Counters {
    fn wrote(&self, kind: &'static str, table: &str, affected: u64) {
        Self::wrote(self, kind, table, affected);
    }
}

impl Counters {
    /// Record that a write touched `affected` rows.
    ///
    /// Counted even when it is zero: a purge that found nothing is a fact
    /// worth having, and a series that only appears once it is non-zero is a
    /// series a dashboard cannot tell from a node that never ran one.
    pub(crate) fn wrote(&self, kind: &'static str, table: &str, affected: u64) {
        let entry = {
            let mut rows = self
                .rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = (kind, table.to_owned());
            match rows.get(&key) {
                Some(entry) => Arc::clone(entry),
                None => Arc::clone(rows.entry(key).or_default()),
            }
        };
        entry.fetch_add(affected, Ordering::Relaxed);
    }

    /// Record one finished call, and hand back the row it landed in.
    ///
    /// The row is returned rather than looked up again later because a
    /// streamed call may still fail in its trailers, and the alternative —
    /// finding the row by name a second time — has to answer "what if it is
    /// not there", "what if the cap sent this call to [`OVERFLOW`]" and "what
    /// if a name arrived twice". Holding the `Arc` makes all three unaskable.
    pub(crate) fn record(&self, method: &str, elapsed: Duration, failed: bool) -> Arc<Method> {
        let entry = {
            let mut methods = self.methods.lock().unwrap_or_else(|poisoned| {
                // A poisoned lock means a panic while holding it, which can
                // only be an allocation failure here — nothing in the guarded
                // section can panic on its own. Losing counters is not worth
                // taking the process down for.
                poisoned.into_inner()
            });
            match methods.get(method) {
                Some(entry) => Arc::clone(entry),
                None => {
                    let key = if methods.len() < MAX_METHODS {
                        method
                    } else {
                        OVERFLOW
                    };
                    Arc::clone(methods.entry(key.to_owned()).or_default())
                }
            }
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
        entry
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
            let late = counters.late.load(Ordering::Relaxed);
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
                 late={late} mean_head={:.1}ms p50_head<={:.1}ms p90_head<={:.1}ms \
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

    /// The same counters in the Prometheus text exposition format.
    ///
    /// Always a `String`, never `None`: a scrape of a node that has served
    /// nothing is a successful scrape of zero series, and answering 404 or an
    /// error there would make "this node is idle" indistinguishable from "this
    /// node is broken" to the one tool whose job is telling them apart.
    ///
    /// # Why this exists beside `summary`
    ///
    /// Everything on the summary line is cumulative over the process's life,
    /// which the ledger recorded as its limitation twice. A mean over a week
    /// is not a mean over now, and a p99 over a week is whatever the worst
    /// hour was. The fix is not a better number here — it is a *second* read,
    /// subtracted from the first, which needs something to read.
    ///
    /// That is also why the histogram is exported as buckets rather than as
    /// the three quantiles the summary prints. A Prometheus summary's
    /// quantiles cannot be subtracted or added: `p99` over five minutes is not
    /// derivable from two cumulative `p99`s, and `p99` across three nodes is
    /// not derivable from theirs. Bucket counts are, which is the whole point
    /// of the endpoint and the reason a nine-line summary was not just wrapped
    /// in HTTP.
    pub(crate) fn prometheus(&self) -> String {
        // Snapshotted under one lock rather than read family by family: the
        // exposition format groups every series of a family together, so the
        // alternative is four passes over the map and four chances for a
        // method to appear in one family and not the next. A scrape holding
        // the lock for the length of a clone of at most `MAX_METHODS` `Arc`s
        // is cheaper than a reader having to reason about that.
        let rows: Vec<(String, Arc<Method>)> = {
            let methods = self
                .methods
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            methods
                .iter()
                .filter(|(_, counters)| counters.calls.load(Ordering::Relaxed) != 0)
                .map(|(name, counters)| (escape_label(name), Arc::clone(counters)))
                .collect()
        };

        // Not even the `# HELP` lines. A family header with no series under
        // it creates no series, so it is exactly as informative as silence and
        // longer — and an empty body is what says "this node is up and has
        // served nothing", which is a thing a scraper needs to be able to
        // read.
        if rows.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        let mut family = |help: &str, kind: &str, name: &str, read: &dyn Fn(&Method) -> u64| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} {kind}");
            for (method, counters) in &rows {
                let _ = writeln!(out, "{name}{{method=\"{method}\"}} {}", read(counters));
            }
        };
        family(
            "Requests that reached a response head, by method.",
            "counter",
            "slate_requests_total",
            &|method| method.calls.load(Ordering::Relaxed),
        );
        family(
            "Requests that failed, at the response head or in a trailer.",
            "counter",
            "slate_request_failures_total",
            &|method| method.failures.load(Ordering::Relaxed),
        );
        family(
            "Failures raised after the head; a subset of the failures total.",
            "counter",
            "slate_request_late_failures_total",
            &|method| method.late.load(Ordering::Relaxed),
        );

        // Its own loop rather than a `family` call: this is keyed on a pair,
        // not on a method, so it shares neither the row set nor the label.
        let written: Vec<((&'static str, String), u64)> = {
            let rows = self
                .rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rows.iter()
                .map(|((kind, table), n)| ((*kind, escape_label(table)), n.load(Ordering::Relaxed)))
                .collect()
        };
        if !written.is_empty() {
            let _ = writeln!(
                out,
                "# HELP slate_rows_written_total Rows a committed write touched, by statement."
            );
            let _ = writeln!(out, "# TYPE slate_rows_written_total counter");
            for ((kind, table), n) in &written {
                let _ = writeln!(
                    out,
                    "slate_rows_written_total{{statement=\"{kind}\",table=\"{table}\"}} {n}"
                );
            }
        }

        let _ = writeln!(out, "# HELP {HEAD} Seconds to the response head.");
        let _ = writeln!(out, "# TYPE {HEAD} histogram");
        for (method, counters) in &rows {
            let mut running = 0u64;
            let mut at = 0usize;
            for edge in EXPORTED {
                // The internal buckets below this boundary, summed. `edge` is
                // a bucket *index*, not a duration, which is what makes this
                // exact: every exported `le` is some internal bucket's exact
                // ceiling, so no sample is ever counted on the wrong side of a
                // boundary. Choosing round numbers like 5 ms instead would
                // have meant interpolating inside a bucket and labelling the
                // guess as if it were a measurement.
                while at <= *edge {
                    running += counters
                        .heads
                        .get(at)
                        .map_or(0, |bucket| bucket.load(Ordering::Relaxed));
                    at += 1;
                }
                let _ = writeln!(
                    out,
                    "{HEAD}_bucket{{method=\"{method}\",le=\"{:.6}\"}} {running}",
                    bucket_ceiling(*edge) as f64 / 1e6,
                );
            }
            let calls = counters.calls.load(Ordering::Relaxed);
            let _ = writeln!(
                out,
                "{HEAD}_bucket{{method=\"{method}\",le=\"+Inf\"}} {calls}"
            );
            let _ = writeln!(
                out,
                "{HEAD}_sum{{method=\"{method}\"}} {:.6}",
                counters.micros.load(Ordering::Relaxed) as f64 / 1e6,
            );
            let _ = writeln!(out, "{HEAD}_count{{method=\"{method}\"}} {calls}");
        }
        out
    }
}

/// The histogram family's name, used in five places in one loop.
const HEAD: &str = "slate_request_head_seconds";

/// Which internal buckets are exported as Prometheus boundaries.
///
/// The internal histogram has 496 buckets. Exporting all of them would be 496
/// series per method and 31,744 for a node at the [`MAX_METHODS`] cap —
/// cardinality that makes a scrape a denial of service against the thing
/// scraping it. These are the last bucket of each octave, so every exported
/// boundary is exactly `2^n - 1` microseconds and exactly some bucket's
/// ceiling; the counts are sums of whole buckets, never interpolations.
///
/// The range is 15 µs to 67.1 s. The bottom sits inside the 10–23 µs that this
/// node's own measurements put its fastest requests at, so the fastest calls
/// separate into the first two boundaries instead of piling into one; the top
/// is past any timeout anyone would configure, so `+Inf` stays empty and the
/// last real boundary is informative rather than a wall.
///
/// Twenty-three boundaries is 26 histogram series a method — the boundaries,
/// `+Inf`, `_sum` and `_count` — plus the three counters, so 29 in all and
/// 1,856 at the cap.
const EXPORTED: &[usize] = &[
    15, 23, 31, 39, 47, 55, 63, 71, 79, 87, 95, 103, 111, 119, 127, 135, 143, 151, 159, 167, 175,
    183, 191,
];

/// A method name, safe to put inside a Prometheus label value.
///
/// The name is `request.uri().path()`, which a caller chooses — the same
/// untrusted string the request log sanitises, and the same argument applies
/// with a different alphabet. A raw `"` would end the label and let a caller
/// forge one of their own; a `\` would escape the quote that ends it; a
/// newline would end the *series*. The exposition format names exactly these
/// three, so this escapes exactly these three.
///
/// Escaped rather than rejected because a label that cannot be forged is
/// enough — dropping the series would lose the count of the very requests
/// somebody was trying to hide.
fn escape_label(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
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
/// invisible to this function. [`Trailing`] is what catches those, and the
/// same reading applies there: a trailer's `grpc-status` that is present and
/// not `"0"` is a failure.
fn head_status(headers: &http::HeaderMap) -> Option<&str> {
    headers.get("grpc-status")?.to_str().ok()
}

/// A response body that notices a gRPC failure in the trailers.
///
/// Until this existed, `failed=` counted only what [`head_status`] could see,
/// so a streamed read that answered a thousand rows and then died read as a
/// success. That is the one failure a node most wants counted — it means the
/// store broke under a scan, not that a caller sent something wrong — and it
/// was the only one missing.
///
/// The comment this replaces called wrapping the body "a per-row cost on the
/// streaming path", and that was asserted rather than measured. It is wrong
/// twice. A frame is a *message*, not a row, so at `rows_per_message = 256`
/// this is consulted once per 256 rows; and the cost of consulting it is
/// **4.7 ns a frame** (median of 21 runs; 3.3 ns bare against 8.0 ns wrapped,
/// with a run-to-run spread of about 1.3 ns either way). Against the measured
/// 41.3 ms drain of a 20,000-row scan at that batch size — 80 frames — that is
/// 375 ns, or 0.0009%, in a table whose own row-to-row spread is 3 ms. The
/// measurement is written up in the ledger entry.
struct Trailing {
    inner: tonic::body::Body,
    /// Where to report a late failure, taken when it is reported.
    ///
    /// An `Option` so a body cannot count the same call twice: gRPC sends one
    /// trailers frame, but nothing in the `Body` contract says a wrapper will
    /// be polled exactly once after it, and a counter that can double under a
    /// polling pattern is a counter nobody can trust.
    late: Option<Late>,
}

/// What a [`Trailing`] needs to report a late failure: the row itself.
struct Late {
    row: Arc<Method>,
}

impl http_body::Body for Trailing {
    type Data = <tonic::body::Body as http_body::Body>::Data;
    type Error = <tonic::body::Body as http_body::Body>::Error;

    fn poll_frame(
        mut self: core::pin::Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        // `Pin::new` rather than a projection crate: `tonic::body::Body` holds
        // a `Pin<Box<..>>`, so it is `Unpin`, and so is everything else here.
        // A `pin-project` dependency to move one field would be a build-time
        // cost for a guarantee the types already give.
        let polled = core::pin::Pin::new(&mut self.inner).poll_frame(context);
        // One chain rather than four nested `if`s, which clippy refuses under
        // CI's `-D warnings`. The order is the cheap test first: most frames
        // carry data and never reach `trailers_ref`, and `take` runs only for
        // the one frame that is a failing trailer.
        if let Poll::Ready(Some(Ok(frame))) = &polled
            && let Some(trailers) = frame.trailers_ref()
            && head_status(trailers).is_some_and(|status| status != "0")
            && let Some(late) = self.late.take()
        {
            late.row.record_late_failure();
        }
        polled
    }

    /// Delegated, not defaulted.
    ///
    /// The default says "cannot tell", which makes hyper poll a body it could
    /// have skipped — and `tonic::body::Body::new` reads exactly this to
    /// decide whether to keep a body at all. Answering for the wrapper rather
    /// than for what it wraps would change how every response is framed, for
    /// no reason connected to counting a failure.
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
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
            let row = counters.record(&method, elapsed, failed);
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
            // Only a call that succeeded at the head can still fail, and only
            // one with a body left to send can say so. A head failure carries
            // an empty body and is already counted, so wrapping it would pay
            // for a box to watch a stream with nothing in it.
            match outcome {
                Ok(response) if !failed && !response.body().is_end_stream() => {
                    let (parts, body) = response.into_parts();
                    let wrapped = Trailing {
                        inner: body,
                        late: Some(Late { row }),
                    };
                    Ok(http::Response::from_parts(
                        parts,
                        tonic::body::Body::new(wrapped),
                    ))
                }
                outcome => outcome,
            }
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
    fn rows_written_are_counted_by_statement_and_table() {
        // The gap this closes: a nightly purge that erases nothing looks
        // exactly like a working one in every counter that existed before,
        // because those count calls and not effect.
        let counters = Counters::default();
        counters.wrote("purge_deleted", "shipments", 9);
        counters.wrote("purge_deleted", "shipments", 3);
        counters.wrote("insert", "books", 1);
        // A call has to have been recorded too, or the exposition is empty by
        // design — see the early return in `prometheus`.
        counters.record(
            "/slate.v1.Records/PurgeDeleted",
            Duration::from_millis(1),
            false,
        );

        let text = counters.prometheus();
        assert!(
            text.contains(
                "slate_rows_written_total{statement=\"purge_deleted\",table=\"shipments\"} 12"
            ),
            "{text}"
        );
        assert!(
            text.contains("slate_rows_written_total{statement=\"insert\",table=\"books\"} 1"),
            "{text}"
        );
        assert_eq!(
            text.matches("# TYPE slate_rows_written_total").count(),
            1,
            "one family header for every series: {text}"
        );
    }

    #[test]
    fn a_purge_that_found_nothing_is_still_a_series() {
        // Zero is the answer an operator most needs: a sweep running and
        // erasing nothing is the failure mode, and a series that only appears
        // once it is non-zero cannot be told from a node that never swept.
        let counters = Counters::default();
        counters.wrote("purge_deleted", "shipments", 0);
        counters.record(
            "/slate.v1.Records/PurgeDeleted",
            Duration::from_millis(1),
            false,
        );
        assert!(
            counters.prometheus().contains(
                "slate_rows_written_total{statement=\"purge_deleted\",table=\"shipments\"} 0"
            ),
            "{}",
            counters.prometheus()
        );
    }

    #[test]
    fn a_node_that_wrote_nothing_exposes_no_write_family() {
        // The same reasoning the method families already use: a family header
        // with no series under it is as informative as silence and longer.
        let counters = Counters::default();
        counters.record("/slate.v1.Records/Query", Duration::from_millis(1), false);
        let text = counters.prometheus();
        assert!(!text.contains("slate_rows_written_total"), "{text}");
    }

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

    /// A body of exactly the frames handed to it, in order.
    ///
    /// Built by hand because the alternative — a real streamed read that dies
    /// part way — cannot be produced through this daemon's own surfaces: there
    /// is no fault-injection knob below the store, and tonic's `grpc-timeout`
    /// times the *service future* rather than the body, so a deadline cannot
    /// expire mid-stream either. What this does not test is therefore named in
    /// the ledger entry, and the framing assumption it rests on — that tonic
    /// puts a streamed call's status in one trailers frame — is checked
    /// against a real response by `a_summary_counts_what_the_node_served`,
    /// which reads `late=0` off a successful streamed query and so goes red if
    /// this comparison is inverted.
    struct Frames(std::collections::VecDeque<http_body::Frame<BodyData>>);

    type BodyData = <tonic::body::Body as http_body::Body>::Data;

    impl http_body::Body for Frames {
        type Data = BodyData;
        type Error = <tonic::body::Body as http_body::Body>::Error;

        fn poll_frame(
            mut self: core::pin::Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(self.0.pop_front().map(Ok))
        }

        fn is_end_stream(&self) -> bool {
            self.0.is_empty()
        }
    }

    /// One data frame, then a trailers frame carrying `status`.
    ///
    /// The data frame is not decoration: a status in the *head* is a different
    /// code path, and a body with nothing before the trailer would not
    /// distinguish them.
    fn streamed(status: &str) -> Frames {
        let mut trailers = http::HeaderMap::new();
        trailers.insert(
            "grpc-status",
            http::HeaderValue::from_str(status).expect("a header value"),
        );
        Frames(
            [
                http_body::Frame::data(BodyData::from_static(b"a row")),
                http_body::Frame::trailers(trailers),
            ]
            .into(),
        )
    }

    /// Poll a body until it ends, discarding the frames.
    async fn drain(mut body: Trailing) {
        core::future::poll_fn(|context| {
            loop {
                match core::pin::Pin::new(&mut body).poll_frame(context) {
                    Poll::Ready(Some(_)) => continue,
                    Poll::Ready(None) => return Poll::Ready(()),
                    Poll::Pending => return Poll::Pending,
                }
            }
        })
        .await;
    }

    /// A counted call whose body is `frames`, as the layer would build it.
    fn watched(counters: &Counters, frames: Frames) -> Trailing {
        let row = counters.record("/x", Duration::from_millis(1), false);
        Trailing {
            inner: tonic::body::Body::new(frames),
            late: Some(Late { row }),
        }
    }

    #[tokio::test]
    async fn the_wrapper_answers_is_end_stream_for_what_it_wraps() {
        // Delegated rather than defaulted, and asserted directly because the
        // difference is invisible in an answer: the default says "cannot tell",
        // which makes hyper poll a body it could have skipped and makes
        // `tonic::body::Body::new` keep one it could have dropped. Nothing
        // about the counters changes, so only this says the delegation is
        // there.
        let live = Trailing {
            inner: tonic::body::Body::new(streamed("0")),
            late: None,
        };
        assert!(
            !live.is_end_stream(),
            "a body with frames left has not ended"
        );

        let empty = Trailing {
            inner: tonic::body::Body::new(Frames(Default::default())),
            late: None,
        };
        assert!(empty.is_end_stream(), "a body with no frames has ended");
    }

    #[tokio::test]
    async fn a_failure_in_the_trailers_is_counted_after_the_head_said_nothing() {
        let counters = Counters::default();
        let body = watched(&counters, streamed("13"));
        // The head has been recorded and the body not yet read, so the call is
        // a success as far as anything could tell — which is the state the
        // whole wrapper exists to correct, and is what `failed=` said for ever
        // before it existed.
        let before = counters.summary().expect("the head was recorded");
        assert!(before.contains("failed=0 late=0"), "{before}");

        drain(body).await;

        let after = counters.summary().expect("the call was recorded");
        assert!(
            after.contains("calls=1 failed=1 late=1"),
            "the trailer's failure should have been counted:\n{after}"
        );
    }

    #[tokio::test]
    async fn an_ordinary_streamed_success_is_not_a_late_failure() {
        // Every successful streamed response carries a `grpc-status: 0`
        // trailer, so this is the common case and a wrapper that counted it
        // would report every read as a failure.
        let counters = Counters::default();
        drain(watched(&counters, streamed("0"))).await;
        let summary = counters.summary().expect("the call was recorded");
        assert!(summary.contains("failed=0 late=0"), "{summary}");
    }

    #[tokio::test]
    async fn a_late_failure_is_counted_once_however_often_the_body_is_polled() {
        // Two trailers frames is not something gRPC sends. It is what a body
        // adapter, a retry, or a `poll_frame` called again after the end could
        // produce, and a counter that doubles under any of those is a counter
        // an operator cannot reason about.
        let counters = Counters::default();
        let mut frames = streamed("13");
        let mut second = streamed("13");
        second.0.pop_front();
        frames.0.extend(second.0);
        drain(watched(&counters, frames)).await;
        let summary = counters.summary().expect("the call was recorded");
        assert!(summary.contains("failed=1 late=1"), "{summary}");
    }

    #[test]
    fn a_caller_cannot_grow_the_counters_without_bound() {
        // The key is the request path, and a caller picks it. The layer wraps
        // the router, so a request for a method that does not exist is counted
        // under whatever it asked for — and with a 4 KiB histogram behind every
        // row, an unbounded map is 256 MB per sixty-five thousand made-up
        // paths rather than a few megabytes.
        let counters = Counters::default();
        for at in 0..MAX_METHODS * 10 {
            counters.record(&format!("/made/up/{at}"), Duration::from_millis(1), false);
        }
        let rows = counters.methods.lock().expect("an uncontended lock").len();
        assert!(
            rows <= MAX_METHODS + 1,
            "{rows} rows past a cap of {MAX_METHODS} plus the overflow"
        );

        // Nothing is dropped: everything past the cap is in one row, and the
        // summary says so rather than quietly undercounting.
        let summary = counters.summary().expect("calls were recorded");
        assert!(
            summary.contains(OVERFLOW),
            "{}",
            &summary[..200.min(summary.len())]
        );
        let counted: u64 = summary
            .lines()
            .filter_map(|line| {
                line.split_once("calls=")?
                    .1
                    .split_once(' ')?
                    .0
                    .parse::<u64>()
                    .ok()
            })
            .sum();
        assert_eq!(
            counted,
            MAX_METHODS as u64 * 10,
            "every call is counted somewhere"
        );
    }

    #[test]
    fn the_overflow_row_cannot_be_spelled_by_a_caller() {
        // A path always begins with `/`, so the parenthesised name is out of
        // reach — otherwise a caller could merge their own calls into the
        // overflow row, or worse, have real methods land in one they control.
        assert!(!OVERFLOW.starts_with('/'), "{OVERFLOW}");
    }

    #[test]
    fn every_exported_boundary_is_an_exact_bucket_ceiling() {
        // The whole claim the histogram export rests on. If a boundary fell
        // *inside* an internal bucket, the count at that boundary would have
        // to interpolate — and a number labelled `le="0.001023"` that is
        // actually a guess about how a bucket's contents are distributed is
        // worse than no number, because nothing downstream can tell.
        //
        // Checked by round-tripping: the boundary's own bucket must be the
        // bucket it came from, and the *next* microsecond must not be.
        for &index in EXPORTED {
            let ceiling = bucket_ceiling(index);
            assert_eq!(
                bucket_of(ceiling),
                index,
                "boundary {ceiling}us should be the ceiling of bucket {index}"
            );
            assert_eq!(
                bucket_of(ceiling + 1),
                index + 1,
                "one microsecond past {ceiling}us should be the next bucket"
            );
            // 2^n - 1, which is what makes the exported labels readable.
            assert_eq!(
                (ceiling + 1).count_ones(),
                1,
                "{ceiling} is not a power of two minus one"
            );
        }
    }

    #[test]
    fn the_exported_boundaries_rise_and_span_the_range_that_matters() {
        let edges: Vec<u64> = EXPORTED.iter().map(|&at| bucket_ceiling(at)).collect();
        assert!(
            edges.windows(2).all(|pair| match pair {
                [lower, higher] => lower < higher,
                _ => true,
            }),
            "{edges:?}"
        );
        // Inside the 10-23us this node's own measurements put its fastest
        // requests at, so the fastest calls separate across the first two
        // boundaries rather than piling into one; past any timeout anybody
        // configures, so `+Inf` stays empty.
        assert_eq!(edges.first().copied(), Some(15), "{edges:?}");
        assert!(
            edges.last().copied().unwrap_or_default() > 60_000_000,
            "{edges:?}"
        );
    }

    #[test]
    fn a_scrape_of_a_node_that_served_nothing_is_empty_and_not_an_error() {
        // Zero series, not an error and not a 404: a monitoring system has to
        // be able to tell an idle node from a broken one, and that is the
        // whole job of the thing doing the scraping.
        let counters = Counters::default();
        assert_eq!(counters.prometheus(), "");
        assert!(counters.summary().is_none());
    }

    #[test]
    fn the_histogram_counts_every_call_and_rises_to_the_total() {
        let counters = Counters::default();
        // One under the first boundary, one over the last, and one in the
        // middle — so the assertions below are about the shape and not about
        // three samples landing in one place.
        for micros in [1u64, 2_000, 90_000_000] {
            counters.record("/x", Duration::from_micros(micros), false);
        }
        let text = counters.prometheus();

        // Series lines only. Matching on `contains` found the `# HELP` line
        // first and read the last word of the prose as a count, which is a
        // test that passes on a comment — caught by writing it wrong once.
        let counted = |needle: &str| -> u64 {
            text.lines()
                .filter(|line| !line.starts_with('#'))
                .find(|line| line.contains(needle))
                .and_then(|line| line.rsplit(' ').next())
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| panic!("no series line matching {needle} in\n{text}"))
        };
        // The `+Inf` bucket is `calls`, which is what makes the histogram
        // agree with the counter family beside it.
        assert_eq!(counted("le=\"+Inf\""), 3, "{text}");
        assert_eq!(counted("slate_request_head_seconds_count"), 3, "{text}");
        assert_eq!(counted("slate_requests_total"), 3, "{text}");
        // 15us holds the 1us sample and neither of the others.
        assert_eq!(counted("le=\"0.000015\""), 1, "{text}");

        // The *labels* are the boundaries' ceilings, not their bucket indices.
        // Checked at the top of the range and not only the bottom, because
        // bucket 15's ceiling happens to be 15 — so an implementation printing
        // the index instead of the ceiling is indistinguishable there, and a
        // mutation doing exactly that survived a version of this test that
        // only looked at the first boundary. Bucket 191's ceiling is
        // 67,108,863 µs, which no index could be mistaken for.
        assert!(
            text.contains("le=\"67.108863\""),
            "the last boundary should be its bucket's ceiling:\n{text}"
        );
        assert!(
            !text.contains("le=\"0.000191\""),
            "a boundary label should never be a bucket index:\n{text}"
        );

        // Cumulative, which is what `le` means and is the property a scraper
        // subtracting two reads depends on.
        let buckets: Vec<u64> = text
            .lines()
            .filter(|line| line.contains("_bucket{"))
            .filter_map(|line| line.rsplit(' ').next()?.parse().ok())
            .collect();
        assert!(
            buckets.windows(2).all(|pair| match pair {
                [lower, higher] => lower <= higher,
                _ => true,
            }),
            "{text}"
        );
    }

    #[test]
    fn a_caller_cannot_forge_a_series_in_a_scrape() {
        // The method name is the request path, which a caller picks, and the
        // layer wraps the router — so a request for a method that does not
        // exist is still counted under whatever it asked for. Unescaped, a
        // name holding a quote closes the label and everything after it is
        // read as more labels; one holding a newline ends the series and the
        // rest is read as another one entirely.
        let counters = Counters::default();
        let forged = "/x\" } 99\nslate_requests_total{method=\"/fake";
        counters.record(forged, Duration::from_millis(1), false);
        let text = counters.prometheus();

        // One series in the family, not two, and its count is the real one.
        let real: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("slate_requests_total{"))
            .collect();
        let only = match real.as_slice() {
            [one] => *one,
            other => panic!("expected one series, got {}:\n{text}", other.len()),
        };
        assert!(only.ends_with(" 1"), "{only}");
        // The newline is escaped, so the forged text cannot be on a line of
        // its own however it is spelled.
        assert!(!text.contains("\n slate_requests_total"), "{text}");
        assert!(
            text.contains("\\n"),
            "the newline should be escaped:\n{text}"
        );
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
