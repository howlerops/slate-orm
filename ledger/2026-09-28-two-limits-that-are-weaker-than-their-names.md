# Two limits that are weaker than their names, said so in four places

## What changed

`[limits] request_timeout` and `[limits] max_concurrent_requests` now carry doc
comments that describe what they do rather than what their names suggest:

- `request_timeout` becomes tonic's `Server::timeout`, whose `GrpcTimeout`
  future polls the handler *before* it polls the sleep. A handler that returns
  without ever pending is never cancelled, whatever the timeout says. It bounds
  a request that **waits**, not request latency. The old comment read "How long
  one request may run before it is cancelled."
- `max_concurrent_requests` becomes `concurrency_limit_per_connection`, so a
  caller who opens a second socket gets a second allowance. The old comment
  read "across all connections", and the comment in `serve.rs` went further:
  "these bound the node as a whole … not the number per connection", which is
  the opposite of the method being called on the line beneath it.

Corrected in `crates/slate-serverd/src/config.rs` (both settings), in
`crates/slate-serverd/src/serve.rs` (the builder comment and the `Serving`
field doc), and in `docs/security-review.md`, whose remedy paragraph for the
"one authenticated caller can pin the node" finding read as though these two
settings answered it. `site/docs/limits.html` already says it, from
`ledger/2026-09-28-the-limits-page-omitted-the-sharpest-limit.md`.

Separately, four caveats are closed: the progress markers "The 468 are
untriaged", "364 remain", "283 remain" and "130 remain", each written into the
batch that was working through them and each retired by the batch after it.
`docs/caveat-status.json` reports 0 untriaged, so all four are false. They are
exempt closures under the existing `stamped` kind, whose reason text named only
the *unread* count and now names both.

## Why

The measurement came first and the prose was left behind it.
`crates/slate-serverd/tests/ceilings.rs` was written to prove the two settings
take effect end to end, and could not: `request_timeout = "0ms"` on a query the
memory backend answers at once returned both rows, five times out of five, and
`1ms` did the same. **On CI the same test cancelled on the first try**, which
makes those ten runs samples of a race rather than a property — see
`ledger/2026-09-28-a-measurement-that-reversed-under-ci.md`. The mechanism
described below holds; the "never cancelled" it was read as saying does not. The test was rewritten to pin the behaviour that is there
— `a_zero_request_timeout_does_not_cancel_a_query_answered_at_once` — and the
limits page was corrected. The doc comments a reader actually meets when they
open the config struct were not, and those are the ones that decide what
somebody types into a TOML file.

The `serve.rs` comment is the worse of the two, because it is not merely stale:
it argues. "A caller's leverage here is the number of requests they can have in
flight, not the number per connection" is a *reason*, stated confidently,
immediately above `concurrency_limit_per_connection`. A comment that explains
why the code is right is the most valuable line in a file when it is true and
the most expensive when it is not, because it stops the next reader checking.

## Alternatives rejected

**Rename the settings.** `request_wait_timeout` and
`max_concurrent_requests_per_connection` would need no prose at all, which is
the better shape. Rejected because the keys are in a shipped TOML schema: every
deployment that sets one would fail to start on the next binary, and serde
would report it as an unknown field rather than as a rename. A deprecation
cycle — accept both, warn on the old — is the real version of this and is
larger than a doc fix; it is worth doing when there is a second reason to touch
the config surface. The names are now wrong *and* labelled, which is strictly
better than wrong and unlabelled, and strictly worse than right.

**Add the node-wide semaphore instead.** That is what
`max_concurrent_requests` sounds like, and the caveat asking for it in
`ledger/2026-09-13-per-request-ceilings.md` is deliberately left **open** here.
Rejected for this change because it is a behaviour change to the serving path
with no test that can observe it: the previous session established that
demonstrating a concurrency limit binding from a client needs a handler that
blocks on command, and a node that ships one has a worse problem than the
limit. Writing the semaphore without being able to watch it refuse would be
the third guess in a row about this code.

**Make `request_timeout` do what it says** by wrapping each handler in
`tokio::time::timeout` rather than relying on tonic's layer. Rejected as out of
scope for a documentation correction, and not obviously right: a latency bound
that fires on a request already holding a transaction needs an answer for what
happens to the transaction, and tonic's semantics — cancel what is waiting —
is the conservative one. Recorded here rather than as a caveat because it is a
design question, not a gap.

**Delete the four progress markers from their entries** instead of closing
them. Rejected: an entry is append-only and dated, which is the whole reason
the verdicts live in `docs/caveat-status.json` and not in the entries. The
count was true when it was written.

## Evidence

- `python3 scripts/mutate.py`, record
  `ledger/mutations/20260928T212654-scripts-check-closed-caveats-py.json`:
  three mutations to the new roster rows, all caught by named checks —
  dropping a row hits "every closed verdict names a witness or an exemption"
  and "no reason outlives the exemption it explains"; renaming the kind to one
  `EXEMPT` does not define hits "every exemption used is one EXEMPT explains".
  Baseline and restore both reported 1 suite, none failing.
- `sh scripts/check.sh`: 71 passed, all of them.
- `cargo test -p slate-serverd --no-fail-fast`: 308 passed across ten binaries,
  0 failed — including the five in `tests/ceilings.rs` that pin the measured
  behaviour this entry describes.
- `python3 scripts/check_closed_caveats.py`: 369 closed verdicts, 345
  witnessed in the tree, 24 exempt with a reason each.
- `python3 scripts/caveats.py`: 1559 caveats, 116 open, 59 narrowed, 369
  closed, 882 deliberate, 0 untriaged. The four progress markers moved from
  `open` to `closed`, and this entry's own three caveats are triaged with it,
  which is why 118 open became 116 rather than 114.

## What this does not do

**It does not measure `max_concurrent_requests` binding.** The per-connection
claim rests on the name of the tonic method called and on tonic's own
documentation for it, which is a reading rather than a demonstration. It is a
reading that is hard to get wrong — the method is named for the thing — but the
distinction is the one this repository keeps insisting on, and four greps
earlier today produced four false alarms for exactly the reason that a word
appearing is not a claim being true.

**It does not re-read the rest of the open caveats.** 116 remain open after
this. Five were found false by accident last night and four more were tested by
grep today and survived the test, which is evidence about those four and not
about the population. Nothing here counts how many of the remainder were
overtaken by work on their own day, and the only way to find out is to read
each one against the tree — which is the work this entry stops short of, not a
decision that the work is unnecessary.

**It does not touch the `1ms` floor.** `config::optional_duration` refuses
anything smaller, so `request_timeout = "1ns"` will not start. That is
unrelated to the polling order and is not written down anywhere a config author
would find it except the error message itself.
