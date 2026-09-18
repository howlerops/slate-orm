# A streamed read that dies part way is counted as a failure, and the cost of noticing was measured rather than assumed

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-serverd/src/observe.rs`, `crates/slate-serverd/tests/observing.rs`, `crates/slate-serverd/Cargo.toml`, `docs/orm-comparison.md`, `site/docs/deployment.html`, `README.md`, `clients/python/src/slate/schema.py`, `clients/go/slate/schema.go`
- **Kind:** fix

## What changed

The observability layer wraps a streaming response's body and reads its
trailers. A `grpc-status` there that is present and not `"0"` is a failure
raised *after* the response head, and it now reaches the counters. The summary
reports it apart:

```
slate-serverd: /slate.v1.Records/Query calls=2 failed=1 late=0 mean_head=1.4ms …
```

`http-body` becomes a named dependency, for the same reason `tower` and `http`
already are. The `Data` and `Error` types are taken from `tonic::body::Body`
rather than from `bytes` and `tonic::Status`, so there is no fourth crate whose
version has to stay in step.

And the counters grew a **cap on distinct method names**, which is not part of
the feature and is here because a mutation on it led straight to a defect. See
the evidence.

Three stale claims went with it, all saying a decimal's scale is not in the
schema fingerprint. `clients/python/src/slate/schema.py` said so on a field
whose own `fingerprint_of`, 160 lines below, hashes it. `README.md` said so
thirty lines above the item recording that it does. And `clients/go/slate/
schema.go` said **both**, in one paragraph — "Deliberately not part of the
fingerprint … and it is part of the fingerprint" — because the change that
added the hashing appended its explanation to the sentence that denied it
instead of replacing it. That is CLAUDE.md's own note about re-reading a
scripted edit, earned a second time, and it is the worst of the three: the
other two are out of date, and this one is a paragraph that cannot be true.

## Why

`failed=` counted only what the response *head* said. gRPC puts a status in a
header when the call fails before the body and in a trailer otherwise — so a
read that answered a thousand rows and then died was counted as a success, and
`failed=0` on a node whose scans were breaking was a true statement of
something nobody asked.

That is the failure most worth counting. A head failure is a request the server
refused: the caller sent something wrong, got nothing, and knows it. A late
failure is the store breaking under a scan — the caller got rows, and whether
it noticed depends on whether it checked the end of the stream. The two point
at completely different things, which is why `late` is a field of its own
rather than folded silently into `failed`.

## Alternatives rejected

**Leave it, as the previous comment did.** That comment gave a reason — "a
per-row cost on the streaming path to correct a count" — and the reason was
asserted rather than measured. It is wrong twice, and both halves are the sort
of thing that only shows up when somebody checks:

- A frame is a **message**, not a row. At `rows_per_message = 256` the check
  runs once per 256 rows, not once per row.
- The check costs **4.7 ns a frame**. The measurement is below.

Against the 41.3 ms drain this repository already measured for a 20,000-row
scan at that batch size, the total is 375 ns — 0.0009%, in a table whose own
row-to-row spread is 3 ms. The cost that justified the gap does not exist.

**Fold `late` into `failed` and print one number.** One field fewer. Rejected
because the two numbers answer different questions and an operator with only
their sum cannot get back to either: `failed=40` is "forty callers sent
something wrong" or "forty scans broke" and there is no way to tell.

**Count it in the service rather than in the layer.** The service knows exactly
when its stream yields an error — `service.rs` has the arm — and would need no
body wrapper at all. Rejected because the counters belong to the daemon and the
service belongs to `slate-server`, which has no idea this layer exists; wiring
them would mean a metrics handle threaded through the library for the benefit
of one of its two callers. The body is the seam the two already share.

**`pin-project` for the wrapper.** `tonic::body::Body` holds a `Pin<Box<…>>`,
so it is `Unpin`, and so is everything else in the struct. A proc-macro
dependency to move one field, for a guarantee the types already give.

## Evidence

**The cost, measured.** A standalone release build outside this workspace,
polling a body of 20,000 data frames plus a trailers frame to completion, with
and without the wrapper, interleaved so a machine that gets busier part way
slows both columns. The inner body is an `UnsyncBoxBody`, which is what
`tonic::body::Body` holds, so the baseline already pays one vtable call a frame
and what is measured is the second one plus the branch. 21 runs:

| | best | median | worst |
| --- | ---: | ---: | ---: |
| bare | 3.24 ns/frame | 3.32 | 4.59 |
| wrapped | 7.95 ns/frame | 8.00 | 10.27 |

**4.7 ns a frame**, with about 1.3 ns of run-to-run spread either way — a
difference well outside the noise, and small enough to be irrelevant: 375 ns
added to a 41.3 ms drain. This isolates the wrapper rather than measuring the
server end to end, which is stated because it matters: it does not include the
one `Box` allocated per streaming response, which is one allocation against a
response that already allocated per message.

**Ten mutations**, each restored, in two passes. The first pass found the
feature sound and the *surroundings* not:

| # | mutation | outcome |
|---|---|---|
| T1 | the trailer comparison is inverted, so a success is a late failure | killed — `a_summary_counts_what_the_node_served`, through the real binary |
| T2 | the body is never wrapped | killed |
| T3 | the late failure is read rather than taken, so it can count twice | killed |
| T4 | `record_late_failure` invents a row for a method never called | **survived** |
| T5 | a late failure bumps `late` but not `failures` | killed |
| T6 | `is_end_stream` is defaulted rather than delegated | **survived** |
| C1 | no cap on distinct method names | killed — `a_caller_cannot_grow_the_counters_without_bound` |
| C2 | calls past the cap are dropped instead of pooled | killed — the same test's total |
| C3 | `is_end_stream` defaulted, again | killed — `the_wrapper_answers_is_end_stream_for_what_it_wraps` |
| C4 | a late failure bumps `late` but not `failures`, again | killed |

**T4 survived and led to the largest thing in this commit, which is not about
trailers at all.** The mutation is genuinely equivalent *as written* — a row
created with `calls=0` is skipped by the summary, so a failure recorded on one
is lost exactly as a failure recorded on no row is. Asking why the original
`get` was right is what surfaced the real problem: the map is keyed on
`request.uri().path()`, the layer wraps the **router**, so a request for a
method that does not exist is still counted under whatever path it asked for —
and the key is chosen by anyone who can open a connection. An unbounded map
fed by a stranger.

That was true before this session and it was cheap: a name and four counters.
The histogram commit made every row 4 KiB, which turns a few megabytes of
nuisance into 256 MB per sixty-five thousand invented paths. `MAX_METHODS = 64`
with everything past it pooled into `(other)` caps it at about 256 KiB, and
pooling rather than dropping means a node under this treatment has a summary
that says it has stopped naming things rather than one that quietly stops
counting. A mutation that "survived, equivalent" was worth more than the eight
that died.

The redesign that followed also deleted T4's whole class: `record` now hands
back the `Arc<Method>` it counted into, and the body wrapper holds *that*
rather than a name to look up later. There is no second lookup, no second lock,
and no question of what happens when the name is missing or the cap redirected
it.

**T6 survived because the test was for an effect that does not exist.** A
defaulted `is_end_stream` answers "cannot tell", which makes hyper poll a body
it could have skipped — no answer changes, no counter changes, nothing
observable through the summary. The fix was to assert the property directly
rather than hunt for a consequence: `the_wrapper_answers_is_end_stream_for_what
_it_wraps` checks the delegation against a body with frames left and one
without, and C3 is the same mutation re-run against it.

**T1 is what makes the hand-built frames honest.** The positive case is tested
with a body assembled by hand, because a *real* mid-stream failure cannot be
produced through this daemon's own surfaces: there is no fault-injection knob
below the store, and tonic's `grpc-timeout` times the service future rather
than the body, so a deadline cannot expire mid-stream either. A hand-built
frame tests the wrapper against this author's idea of what tonic sends, which
is exactly the kind of test that passes while the feature does nothing.

Inverting the comparison closes that gap from the other side. The integration
suite's successful call is a *streamed* read, so tonic ends it with a real
`grpc-status: 0` trailer, and `late=0` in that assertion is only true if the
wrapper is installed on the real path, is polled, sees a trailers frame tonic
built, and reads the status out of it. The mutation turns that line red. What
remains untested is the single step from "reads a real `0`" to "reads a real
`13`", which is one `Status::to_header_map` inside tonic.

`cargo test -p slate-serverd --no-fail-fast` (153 unit plus every integration
binary), `cargo clippy --workspace --all-targets`, `cargo fmt -p slate-serverd`,
`sh .githooks/test-pre-commit.sh`, `python3 site/check/docs.py`,
`ruff check clients/python`, `gofmt -l clients/go`: green.

Clippy's `collapsible_if` caught the four nested `if`s in `poll_frame` — a
warning here and, under CI's `-D warnings`, an error there, which is the gap
CLAUDE.md warns about from the other direction. It is one let-chain now,
ordered cheapest test first so a data frame never reaches `trailers_ref`.

## What this does not do

- **No real mid-stream failure is exercised anywhere.** Named above, and it is
  the honest limit of this change. Closing it properly means a fault-injection
  knob below the store that the daemon can be started with — which is a
  production-facing setting added for a test, and a bigger decision than this.
- **A client that hangs up is not a failure.** The body is dropped without
  reaching its trailers, so nothing is counted, which is right: the server did
  not fail. It does mean `calls` counts a call whose rows nobody read.
- **The duration is still the head's.** A late failure does not re-time the
  call, so a read that streamed for a minute and then died contributes its
  first-byte latency to the quantiles and nothing else.
- **`late` is cumulative, like everything else on the line.** Same limitation
  as the quantiles, same fix, same still-open `/metrics` item.
- **The cap can be poisoned, only degraded.** Sixty-four invented paths before
  the first real request leaves every genuine method pooled in `(other)` until
  a restart. That is a summary made useless by somebody who could already fill
  a log with the same requests, and it is bounded, which is the property that
  was missing. Distinguishing a routed path from an unrouted one would fix it
  properly and needs the layer to know what the router knows, which it does
  not.
- **Nothing reads the trailer's `grpc-message`.** The count says a stream died
  and not why; the reason is in the client's error and in the request log's
  line for the call, which carries the same request id.
