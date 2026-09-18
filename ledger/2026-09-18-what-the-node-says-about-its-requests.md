# `slate-serverd` can now say what it served, and how it went

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working the second six of `docs/orm-comparison.md`
- **Touches:** `crates/slate-serverd` (`observe.rs`, `serve.rs`, `main.rs`,
  `config.rs`), `site/docs/deployment.html`, `docs/orm-comparison.md`,
  `README.md`
- **Kind:** feature

## What changed

A `tower` layer wraps the gRPC service and times every call to its response
head, counting calls, failures, total and slowest per method. Two settings turn
that into output: `[observability] request_log = true` writes a line per call,
and `summary_interval = "60s"` writes per-method counters on that cadence and
once more on the way out. Both off by default. Four arguments that had been
travelling together through both start paths — `grace`, `concurrency`,
`request_timeout` and now `observing` — became a `serve::Serving` struct,
because the fourth pushed `announce_and_serve` to eight positional arguments.

## Why

The node logged its startup and its warnings and nothing in between. Finding
out that a deployment was serving `Query` ten thousand times a minute at a p99
of two seconds meant instrumenting the *client*, which is the wrong end. The
absence was recorded in three places — the README's list, the comparison's gap
table and its "smaller, and owed" list — and it blocked a fourth item: the
README's request-id bullet said an id sent from a client would have nothing to
be correlated against, so a log had to come first.

## Alternatives rejected

**A `tonic` interceptor**, which is the obvious mechanism and the documented
one. It sees the request and not the response, so it can log that a call
*arrived* and not how it ended or how long it took — which is most of what a
request log is for. A layer wraps the whole call. The cost of the layer is a
boxed future per request and a `tower` dependency; the cost of the interceptor
is a log nobody can use.

**`tracing`.** It is what a Rust service normally reaches for, and it is a
bigger decision than this item. `tracing` without a subscriber logs nothing, so
adopting it means also choosing a subscriber, a format and a filter syntax, and
each is a thing an operator then has to configure before they see a line. This
writes to stderr in the `slate-serverd: …` shape every other line the process
already uses, and an operator who wants structure pipes it somewhere. If that
becomes the constraint, `tracing` is the answer and `observe.rs` is one file to
replace — which is the point of keeping it one file.

**A `/metrics` endpoint.** The right shape for a scraped deployment and the
wrong size for this: it means a second listener or a second service on the
first, a decision about the exposition format, and a decision about whether it
is authenticated (it would have to be — it leaks traffic shape). A summary on a
cadence needs none of that and answers the same first question.

**A histogram rather than a mean and a slowest.** A mean hides exactly the tail
somebody is looking for, and this is the honest weakness of what shipped. HDR
or a t-digest is ~200 lines and a per-request update on the hot path; the
version here is four atomics. The mean plus the slowest is enough to *notice* a
problem and not enough to *characterise* one, which is written into the
comparison rather than left for somebody to discover.

**Timing to the last row rather than the head.** A streaming read returns its
head almost immediately and then streams for as long as the client keeps
reading, so a slow consumer would read as a slow server. Rejected, and the log
line says `head=` so the number is not mistaken for the whole call.

**Silencing clippy's `too_many_arguments` with an `allow`.** Cheaper by four
lines. The lint was right: eight positional arguments, four of them `Option`s
and `Duration`s, is a signature a caller can silently transpose.

## Evidence

Six process-level tests in `crates/slate-serverd/tests/observing.rs`, all
against the real binary's stderr rather than against `Counters` directly — the
four unit tests beside `observe.rs` cover the arithmetic and would all still
pass if the layer were never applied to the server at all.

Nine mutations, eight killed by a named test:

| mutation | outcome |
| --- | --- |
| `let failed = false` — never count a failure | KILLED `a_summary_counts_what_the_node_served` |
| `if true` — log regardless of the setting | KILLED `a_node_told_nothing_says_nothing` |
| `if false` — never log | KILLED `a_request_log_names_the_method_and_how_it_ended` |
| method key is a constant, not the path | KILLED `methods_are_counted_apart_over_the_wire` |
| no summary printed on shutdown | KILLED `a_summary_counts_what_the_node_served` |
| the zero-interval refusal removed | KILLED `a_zero_summary_interval_is_refused_at_startup` |
| the layer is never applied to the builder | KILLED (3 tests) |
| the ticker's loop body emptied | **SURVIVED**, then KILLED |
| the first tick is not consumed | **SURVIVED**, and left as is |

The eighth is the one worth recording. Every test read the *final* summary
printed on the way out, so a node whose periodic summary never fired looked
identical to one whose did — the feature, untested.
`the_summary_repeats_on_its_interval_while_the_node_runs` tells them apart by
counting: a one-hour interval yields exactly one summary, so two or more over
3.2 seconds at a one-second interval means the ticker fired.

The ninth survives and is an equivalent mutation, not a missing test:
`summary()` returns `None` for a node that has served nothing, so `interval`'s
immediate first tick prints nothing whether or not it is consumed. The line
stays for the narrow case where a request lands in that window, and the comment
above it — which had claimed it prevented an empty summary, which was false —
now says so.

One real defect found while reviewing rather than from a test:
`summary_interval = "0s"` parsed, and `tokio::time::interval` **panics** on a
zero period. It is spawned, so the panic would have landed in a detached task
and the node would have served on with no summary and no explanation. Refused
at startup now, in the same shape as the other zero checks beside it.

Disk hit ENOSPC mid-run (the linker error `CLAUDE.md` describes); the ledger's
dedup snippet freed 4.94 GB and the run finished. `cargo test -p slate-serverd
--no-fail-fast`: 204 passed, 0 failed. `RUSTFLAGS=-Dwarnings cargo clippy
--workspace --all-targets` clean, `cargo fmt --all -- --check` clean.

## What this does not do

- **No histogram, so no percentiles.** Mean and slowest only. Said above and in
  the comparison.
- **A failure raised in a trailer counts as a success.** gRPC puts the status
  in a header when the call fails before the body and in a trailer otherwise,
  and this reads the head. For this server that means a streamed read that dies
  part way is invisible to `failed=`. Catching it means wrapping the response
  body to inspect its trailers — a per-row cost on the streaming path to
  correct a count.
- **No request id.** The blocker on the README's bullet is gone; the id is
  still not built. Nothing in the protocol or in any of the three clients
  carries one, so a log line still cannot be tied to a caller's call.
- **No per-tenant or per-principal breakdown.** Counters are keyed on the gRPC
  method alone. Keying on identity would make the map unbounded in a caller's
  control, which is a different design than "at most eighteen entries".
- **The summary goes to stderr and nowhere else.** No file, no rotation, no
  endpoint. An operator who wants any of those pipes the process.
