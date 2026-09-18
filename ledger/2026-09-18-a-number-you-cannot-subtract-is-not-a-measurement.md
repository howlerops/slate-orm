# A scrape endpoint, because a cumulative quantile cannot be subtracted

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-serverd/src/{metrics.rs,observe.rs,serve.rs,main.rs,config.rs}`, `crates/slate-serverd/Cargo.toml`, `crates/slate-serverd/tests/{observing.rs,refusals.rs,harness/mod.rs}`, `site/docs/deployment.html`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`[observability] metrics_address = "127.0.0.1:9090"` starts an HTTP listener
serving `GET /metrics` in the Prometheus text exposition format. Off unless
set. The bound address is announced on standard output as `METRICS <address>`,
beside the existing `LISTENING` line, so a scraper — or a test using port 0 —
can find it.

It exports three counters per method and the head latency as a **histogram**.

## Why

Everything on the summary line is cumulative over the process's life. Two
ledger entries have now recorded that as the limitation, and both said the same
fix: a second read, subtracted from the first. There was nothing to read.

There is no better number to print instead. A mean over a week is not a mean
over now no matter how it is formatted, and the p50/p90/p99 added earlier are
the same shape — better than a mean and still an average over every hour the
process has been up, including the one where the object store was slow.

## Alternatives rejected

**A `Metrics` gRPC method.** No new dependency, no second port, nothing extra
to shut down. Rejected because nothing that scrapes metrics speaks gRPC:
Prometheus, Grafana Agent, Vector and every hosted equivalent fetch a URL, so
this would mean an adapter process per deployment. An endpoint no monitoring
system can reach is not monitoring.

**A path on the gRPC port.** Rejected because the two ports want different
firewall rules — one carries the data, the other carries the shape of the
traffic — and one address cannot have two.

**Export a Prometheus *summary* (quantiles) rather than a histogram.** It is
what the text line already computes, so it is nearly free, and it is wrong for
the one job this endpoint exists to do. A summary's quantiles cannot be
subtracted or added: `p99` over the last five minutes is not derivable from two
cumulative `p99`s, and `p99` across three nodes is not derivable from theirs.
Bucket counts are both. Exporting a summary would have shipped the endpoint and
left the limitation exactly where it was.

**Round `le` boundaries — 5 ms, 10 ms, 25 ms.** What every Prometheus example
uses, and unavailable honestly. The internal histogram is eight buckets to the
octave, so `0.005` falls *inside* a bucket and the count at it would have to
interpolate — a guess about how a bucket's contents are distributed, published
with a label that makes it look like a measurement. The exported boundaries are
the last bucket of each octave instead, so each is exactly `2^n - 1`
microseconds and each count is a sum of whole buckets. `le="0.001023"` is ugly
and true.

**Export all 496 internal buckets.** Exact, and 31,744 series for a node at the
`MAX_METHODS` cap — a scrape that is a denial of service against the thing
scraping it. Twenty-three boundaries is 29 series a method, 1,856 at the cap.

**A bearer token on the endpoint.** The body is every method this node serves
with call counts and latencies, which is traffic analysis handed over on
request. Rejected because a shared secret in the same configuration file,
compared as a string over an unencrypted port, is a thing that *looks* like
authentication and stops nobody who is on that network. The honest answer is
"off by default, loopback in every example, a startup warning when the address
is not loopback, and put a firewall in front", which is what shipped and what
the docs page says in those words.

**Hand-write the HTTP.** A scrape is one `GET`, and this was sketched that way
first. It is a parser on a listening socket, which is the category of code this
repository fuzzes rather than writes on a hunch — and hyper 1 is already in the
tree under tonic, so "no new dependency" was never actually on offer, only "no
new *declared* dependency".

## Evidence

**A defect this shipped with, and fixed.** The first version bound the metrics
port while parsing the configuration, which put the bind *before* `--check`
returns. So validating a configuration held the port: two `--check` runs at
once refused each other, and a check run against a live node's own file — the
single most likely way anybody runs `--check` — failed with "address already in
use" on a file that was perfectly valid. Reproduced by holding the port from
Python and running `--check`, which exited 2. The parse and the warning happen
before the check now and the bind happens after, and
`checking_a_configuration_does_not_need_its_metrics_port_free` holds a port and
requires the check to pass.

The bind still happens *before* the lease campaign, which is deliberate and the
opposite trade: a metrics address that cannot bind should stop this node before
it fences another node's writer. Failing after the campaign would mean a leader
that exits and a cluster that has to notice.

**Eight mutations. Six died on the first pass; two are the finding.**

| # | mutation | first pass | after |
|---|---|---|---|
| P1 | the method label is not escaped | killed — `a_caller_cannot_forge_a_series_in_a_scrape` | |
| P2 | the `le` boundaries are bucket indices, not ceilings | **survived** | killed |
| P3 | the histogram reports per-bucket counts, not cumulative | killed | |
| P4 | the metrics listener is never spawned from `serve.rs` | **hung** | killed |
| P5 | the `METRICS` banner is never printed | killed — `a_scrape_reports_what_the_node_served` | |
| P6 | every path answers the metrics body | killed | |
| P7 | the content type loses its `version=0.0.4` | killed | |
| P8 | the listener is bound and then dropped instead of served | killed | |

**P2 survived on a coincidence.** The test checked the rendered label at the
first boundary, `le="0.000015"` — and bucket 15's ceiling is also 15, so an
implementation printing the bucket *index* instead of its ceiling produces a
byte-identical line there and nowhere else. The assertion now also reads the
top of the range, where the index is 191 and the ceiling is 67,108,863 µs, and
requires that `le="0.000191"` appears nowhere.

**P4 did not survive; it hung, and the harness called that a survival.** A
listener that is *bound* but never accepted still completes the TCP handshake
out of the kernel's backlog, so `connect` and `write` both succeed and only the
read blocks — forever, because the port is bound for the life of the process.
The scrape helper had no timeout, so removing the spawn turned the suite from
"reports a failure" into "reports nothing", and a script grepping for `FAILED`
read that as clean. Two things came out of it: the helper now bounds the read
at twenty seconds and says in its failure message that nothing is serving the
port, and the mutation runner distinguishes exit code 124 from a pass. A
hanging test in CI is a six-hour job rather than a red cross, which is worse
than the mutation it was meant to catch.

P1 is the one worth naming. The method label is `request.uri().path()` and the
layer wraps the router, so a request for a method that does not exist is still
counted under whatever path it asked for — the same untrusted string the
request log already sanitises, arriving in a format with a different alphabet.
Unescaped, a name holding `"` closes the label and everything after it is read
as more labels; one holding a newline ends the series and the rest is read as
another series entirely. The test sends
`/x" } 99\nslate_requests_total{method="/fake` and requires exactly one series
in the family.

P7 looks pedantic and is not: a body served as bare `text/plain` may be read as
OpenMetrics, which requires a trailing `# EOF` this does not write, and the
scrape then fails on a format error rather than on anything being wrong.

**Two tests I got wrong first**, both caught by the tests themselves rather
than in review. A scrape of a node that had served nothing emitted the `# HELP`
and `# TYPE` headers with no series under them — valid, and exactly as
informative as silence — so the empty case now returns an empty body. And the
helper reading a count out of the exposition matched `# HELP
slate_requests_total …` before the series line and parsed the last word of the
prose; it filters comment lines now, and the reason is in the test.

**Two numbers in my own comment were wrong** and were corrected against the
arithmetic rather than left: the first boundary is *inside* the 10–23 µs band
the head node measures, not below it, and the cardinality at the cap is 1,856
series, not the 1,600 first written.

`cargo test -p slate-serverd --no-fail-fast`: 163 unit tests including five new
in `metrics.rs` and five in `observe.rs`, 10 in `observing.rs` including two new
through the real binary, and 42 in `refusals.rs` including three new on the
configuration. `cargo clippy --workspace --all-targets`, `cargo fmt -p
slate-serverd`, `sh .githooks/test-pre-commit.sh`, `python3 site/check/docs.py`:
green.

## What this does not do

- **No authentication.** Stated above, in `metrics.rs`, and on the docs page.
  It is a decision, not an oversight, and the warning at startup is where a
  deployment gets told.
- **Nothing exports the kernel.** These are the head node's request counters.
  Rows read, bytes fetched from the object store, compaction, lease renewals —
  none of it is here, and the kernel has no counters to export yet.
- **No `tracing`.** A span per request with a subscriber is the other half of
  the observability row and is untouched.
- **Still the response *head*.** Every latency in the histogram is time to the
  head, so a streamed read's rows are not in it — the same limitation the
  summary line has, now exported.
- **No test runs two nodes and compares two scrapes.** Subtracting one read
  from another is the whole argument for the endpoint, and what is tested is
  that the buckets are cumulative and monotone — the property that makes the
  subtraction meaningful — rather than a scraper actually doing it. Doing the
  real thing means a Prometheus in the test suite.
- **The shutdown does not wait for a scrape in flight.** The listener is told
  to stop after the drain and the last summary, so a scraper polling through a
  shutdown gets the final numbers; but a scraper holding a connection open
  cannot delay the exit, and a scrape that arrives during the last few
  milliseconds is refused rather than queued.
