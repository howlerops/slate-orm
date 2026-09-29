# Ten streams under a limit of one, and a count that matched any count

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `crates/slate-serverd/tests/{ceilings.rs,observing.rs}`
- **Kind:** fix

## What changed

`open_streams_are_not_bounded_by_the_concurrency_limit`: ten `Query` streams
opened against a node running `max_concurrent_requests = 1`, none drained until
all ten exist, and the node's own `/metrics` asked afterwards how many it
admitted. **Ten.** Then each stream is drained and carries both seeded rows.

That is the measurement two entries had recorded as a reading. And it is a
**negative** result stated as one: a caller who opens ten thousand slow streams
is bounded by nothing in this setting, so a deployment wanting that bounded
cannot get it here.

Two of the four mutations are recorded survivors, with reasons, because
survival *is* the finding. And one of the two that were caught found a defect
in the assertion I had just written.

## Why

Three caveats, from the two entries that shipped the limit and the timeout:

> **It bounds admission, not open streams.** A caller who opens ten thousand
> slow streams is bounded by nothing here.
> — `ledger/2026-09-29-a-fifth-ceiling-and-a-limit-whose-name-was-a-lie.md`

> **The streaming observation is read, not measured.** That tower's permit is
> released when the response future resolves is read off its shape and tonic's
> use of it, not instrumented.
> — `ledger/2026-09-28-a-request-timeout-does-not-bound-a-fast-request.md`

> **Nothing has seen the concurrency limit bind.**
> — the fifth-ceiling entry again

A resource control whose behaviour is known by reading the library's source is
a resource control nobody has seen work. `a_concurrency_limit_in_the_file_still_serves`
asserted the node starts and answers, which is the value surviving the trip
from TOML and nothing about the limit. The reading — permit released when the
response future resolves, and a streaming handler's future resolves before any
row — predicts something falsifiable: many streams open at once under a limit
of one. That is what this runs.

## Alternatives rejected

**A test-only knob that makes a handler slow.** The direct way to watch the
limit *bind*: a hidden delay, a `#[cfg(test)]` sleep, a config field nobody
should ship. It is what the fifth-ceiling entry said would be needed and why
it left the caveat open. Rejected again, for the same reason and one more: a
knob that exists only in tests makes the tested stack a different stack, and
the thing under test here is precisely how the real layers compose.

**Measure the time two overlapping requests take.** A limit of one serialises
them, so the pair takes twice as long as one — which is a duration, on a
container where a timing claim from this repository already reversed between
two machines. The count is the honest instrument, and a count is what
`/metrics` gives.

**Open the ten streams concurrently.** More faithful to the worry and no
stronger: if a permit were held for a stream's life, `tower` *queues* rather
than refusing, so the second sequential open would simply never return — and
the test's timeout is the discriminator either way. Sequential is fewer moving
parts and the same experiment.

**Assert the ten from the client's ten handles.** Ten `Streaming` values in a
`Vec` says the client believes it opened ten. The server's counter says the
server admitted ten, which is the claim. The difference is not pedantic: it is
the one thing the client cannot get wrong about on its own, and without it the
run had no assertion that depended on the number at all.

## Evidence

- `cargo test -p slate-serverd --test ceilings`: **6 passed, 0 failed.**
- The node, started with `max_concurrent_requests = 1`, reported
  `slate_requests_total{method="/slate.v1.Records/Query"} 10` after ten streams
  were opened and before any was drained.
- **Four mutations**
  (`ledger/mutations/20260929T031441-crates-slate-serverd-tests-ceilings-rs.json`).
  Two caught: the metrics assertion pinned to a literal `1` instead of the
  constant, and the streams dropped as they are opened. Two **expected
  survivors**, each with its reason recorded in the run:
  - reducing `OPEN_AT_ONCE` from ten to one. The constant sizes the
    demonstration and cannot be load-bearing — the finding is that the node
    does not distinguish one open stream from ten, so every assertion follows
    the constant rather than pinning it.
  - starting the node with no concurrency limit at all. Also the finding: a
    node with no limit admits ten streams and so does one limited to a single
    request. Catching it would mean the limit *had* begun bounding open
    streams, which is the change this test would fail on deliberately.
- **A defect in my own assertion, found by the first of the caught pair.**
  `scraped.contains("… } 1")` is satisfied by `… } 10`: a count is a prefix of
  a bigger count, so the mutation that replaced `{OPEN_AT_ONCE}` with a literal
  `1` survived the first run. It compares the whole line now. The same shape
  was in `observing.rs` — `contains("… } 2")` and `contains("… } 1")`, latent
  because the counts there are 2 and 1 today — and is fixed alongside.
  (`ledger/mutations/20260929T031333-crates-slate-serverd-tests-ceilings-rs.json`
  is the run where it survived, outcome `problems`.)
- `cargo clippy --workspace --all-targets`: clean.
- `python3 scripts/caveats.py`: 1647 caveats, 110 open, 73 narrowed, 411
  closed, 920 deliberate, 0 untriaged.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**It still has not seen the limit bind.** Narrowed, not closed: every read RPC
is server-streaming and provably releases its permit early, so the remaining
question is a *unary* call held long enough to keep one. A write is unary, and
nothing here makes one slow without the knob rejected above.

**Ten is not ten thousand.** The claim that a caller can exhaust a node by
opening streams is supported by ten and not demonstrated at a scale anyone
would call an attack. What ten establishes is that the setting does not bound
the count at all, which is the part a deployment needs to know before choosing
it as a defence.

**Nothing bounds concurrent streams instead.** This measures the absence and
adds no mechanism. A cap on open response streams is a different layer —
tonic's `max_concurrent_streams` on the HTTP/2 connection is the obvious
candidate and is per-connection, which is the scope mistake this setting was
just fixed for. Whoever adds it should read that entry first.

**The metrics port is now part of this test's configuration.** It is there so
the count has a witness, and it means the test exercises a node with
observability on where the one above it does not. A failure that turns out to
be about the metrics layer would read as a failure about concurrency.

**`observing.rs`'s fix is latent-only.** Its counts are 2 and 1, so no current
run could have been fooled; the change is to stop the next one being. Nothing
tests the anchoring itself in that file — the mutation that demonstrates it
lives in `ceilings.rs`, where the count is ten.
