# A node-wide cap on open response streams, and "open" turned out to mean something narrower than I assumed

- **Date:** 2026-09-29
- **Author:** Claude Code, task K5
- **Touches:** `crates/slate-serverd/src/streams.rs` (new),
  `crates/slate-serverd/src/{config,main,serve}.rs`,
  `crates/slate-serverd/tests/ceilings.rs`, `docs/security-review.md`
- **Kind:** feature

## What changed

`[limits] max_open_streams`: a node-wide cap on response streams, whose permit
**rides on the response body** rather than on the handler's future. A caller
over it is refused with `RESOURCE_EXHAUSTED` immediately. Unset means
unbounded; `0` is refused at startup. Two tests in `ceilings.rs` — one that
watches the cap bind and release, one that watches zero be refused.

## Why

`ledger/2026-09-29-ten-streams-under-a-limit-of-one.md` opened ten concurrent
reads against a node configured for `max_concurrent_requests = 1`, watched all
ten be served, and wrote down what it had not built:

> **Nothing bounds concurrent streams instead.** This measures the absence and
> adds no mechanism. A cap on open response streams is a different layer —
> tonic's `max_concurrent_streams` on the HTTP/2 connection is the obvious
> candidate and is per-connection, which is the scope mistake this setting was
> just fixed for. Whoever adds it should read that entry first.

Read, and heeded: this is not tonic's setting. One `Semaphore` in an `Arc`,
shared by every clone of the layer, so a caller opening a second socket does
not get a second allowance — the one-word difference that made
`concurrency_limit_per_connection` read as correct for weeks.

**The finding is what "open" means.** The first version of the test opened two
streams against a cap of one, did not drain the first, and expected the second
to be refused. It was served. The permit had already come back, because a body
is finished when the **server** has written it, not when the client has read
it: two rows fit in one frame, the whole response was flushed before the second
request arrived, and `Held` was dropped.

So the cap bounds streams the node is **still producing**. That is a narrower
claim than "streams the caller has open" and it is the useful one — a client
holding an idle handle costs a socket, and a client holding a scan costs the
kernel work behind it — but it is not what I set out to build, and the test now
says so in the mechanism it uses: 20,000 seeded rows, roughly half a megabyte,
against HTTP/2's 64 KiB windows. A server that has filled the window stops
writing until the reader consumes, and *that* is a stream that is open in the
sense this cap means.

**Refused, not queued.** The admission limiter queues, and queueing is right
there — a request waiting to start is one that will finish. A stream is held
for as long as its reader likes, so a queue in front of one has no deadline,
and a client reading `n + 1` streams round-robin would wait on a permit only it
could release. A refusal turns that deadlock into an error a caller can retry.

## Alternatives rejected

**Tonic's `max_concurrent_streams`.** Named for exactly this and is HTTP/2's
per-connection setting, so a second socket doubles the allowance. That is the
defect `max_concurrent_requests` was fixed for two entries ago, and adopting it
would have reintroduced it under a name that reads even more like the right
thing.

**Holding the permit in `poll_ready` and waiting there.** Where a tower limiter
usually waits, and wrong twice: it would queue, which the module argues against
at length, and `poll_ready` answers about the *service* rather than about one
request — a service reporting itself not-ready makes hyper stop reading the
connection rather than answer the caller.

**Widening `max_concurrent_requests` to hold its permit through the body.** One
setting instead of two, and it would silently change what an existing
configuration means: a deployment that set it to bound admission would find it
bounding streams, which is a different and much smaller number. The two caps
answer different questions and a node may want both, which is why the test sets
`max_concurrent_requests = 8` beside `max_open_streams = 1`.

**Counting streams rather than gating them** — a metric and no refusal. Cheaper
and answers the wrong question: the entry that asked for this was about a node
a caller can pin, and an observation of being pinned is not a defence.

**Refusing with `UNAVAILABLE`.** The other plausible code, and wrong: it means
the node is down, every client here retries it, and the node is up. The caller
is over a quota, so `RESOURCE_EXHAUSTED` — which is what the execution ceilings
already use.

**Returning HTTP 429.** A gRPC failure is an HTTP 200 whose `grpc-status` says
otherwise. A 429 reaches the client as a transport error with no status code,
and every client here turns that into a different exception than the one it has
for a refused request.

## Evidence

- **The cap binds, and the permit comes back.**
  `a_second_stream_is_refused_when_one_may_be_open`: under
  `max_open_streams = 1` the first stream opens and carries all 20,000 rows;
  the second is refused **within the 20-second timeout** rather than queued,
  with `Code::ResourceExhausted` and a message naming "open response streams";
  and after the first is drained a third opens and carries 20,000 rows. That
  third is not decoration — a layer that refused everything after the first
  request passes every assertion before it.
- **The whole suite:** `cargo test -p slate-serverd --test ceilings` — 8
  passed, 0 failed, 3.36s.
- **Mutation**, record
  `ledger/mutations/20260929T154206-crates-slate-serverd-src-streams-rs.json`:
  two cases, two caught, both by
  `a_second_stream_is_refused_when_one_may_be_open`.
  - dropping the permit when the handler returns instead of moving it into the
    body → caught. This is the mutation that turns the new setting back into
    the old one, and it is the reason the 20,000 rows are there.
  - `Arc::clone(&self.permits)` → a fresh `Semaphore` per `layer` call → caught.
    This is the per-connection scope mistake, reintroduced deliberately.
  Both are changes: the first moves a `drop`, the second constructs a new
  semaphore where one was shared.
- **Two rows were not enough, measured rather than reasoned.** With the
  original seed the second stream returned
  `Response { … message: Streaming }` — served, not refused — which is the
  observation the "what open means" section is built on.
- `cargo clippy -p slate-serverd --all-targets`: clean. `cargo fmt -p
  slate-serverd`: applied.

## What this does not do

**It bounds a count, not bytes or time.** One stream reading a hundred million
rows takes one permit and may hold it for an hour. A byte or duration budget is
a different mechanism and `slate-kernel`'s per-request ceilings are what
currently bound the work behind a stream.

**It does not bound idle handles.** A client that opens a stream, reads it to
the end and keeps the handle costs a socket and no permit. If the resource a
deployment is worried about is file descriptors, this is the wrong setting and
the right one is at the listener.

**The 20,000 rows are calibrated against a default window.** HTTP/2's initial
window is 64 KiB and neither tonic's nor hyper's default is pinned here, so a
future version that raises it could let the whole response through and turn the
test's refusal into a pass-by-accident. The margin is eight windows, which is
generous and is not a guarantee; what would be one is a handler that blocks on
command, and that is the knob
`ledger/2026-09-28-a-request-timeout-does-not-bound-a-fast-request.md` rejected
shipping.

**Nothing measures what the layer costs.** It adds an `Arc` clone, an atomic
decrement and a body wrapper per request. `observe.rs`'s equivalent wrapper was
measured at 4.7 ns a frame and this one does strictly less per frame, so the
cost is very likely smaller — "very likely" being an inference from a
neighbouring measurement rather than a measurement of this.

**No client knows about it.** All three turn `RESOURCE_EXHAUSTED` into their
ordinary refused-request exception, which is correct and means none of them
retries with backoff or says anything specific. A caller hitting this sees the
same shape of error as a caller who broke a `CHECK`.
