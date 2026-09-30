# The ceilings, against a live node — and what `request_timeout` does not bound

- **Date:** 2026-09-28
- **Author:** an agent session, closing the residual of
  `ledger/2026-09-20-the-ceilings-and-the-paths-they-do-not-reach.md`
- **Touches:** `crates/slate-serverd/tests/ceilings.rs`
- **Kind:** fix

## What changed

Five tests against a real `slate-serverd` process, which is what the residual
asked for: *"I did not check that setting them in TOML actually takes effect
end to end. That is a different kind of test — a live server — and it is not
here."*

Four of them assert what was expected. The fifth asserts the opposite, because
the experiment came back the other way.

## Why

**A `request_timeout` does not bound a request the handler answers at once.**
That is a property of tonic, not of this code. `GrpcTimeout::poll` polls the
inner future first and returns its `Poll::Ready` before the sleep is ever
polled:

```text
if let ready @ Poll::Ready(_) = this.inner.poll(cx) { return ready; }
if let Some(sleep) = this.sleep.as_pin_mut() { ready!(sleep.poll(cx)); … }
```

So a handler that completes without pending cannot be cancelled, whatever the
timeout says. I set out to write the obvious test — a tiny timeout, a query, a
`CANCELLED` — and it failed. Then it failed at every value the configuration
accepts:

| `request_timeout` | runs | query cancelled |
|-------------------|-----:|----------------:|
| `1ms`             |    5 |               0 |
| `0ms`             |    5 |               0 |

`1ns` is refused at startup: the smallest unit the parser takes is `ms`.

~~So there is no value a client can set that makes a two-row in-memory query
overrun, and the question "does the timeout ever fire?" cannot be answered from
this side at all.~~

**Wrong, and withdrawn the same evening. CI run 443 cancelled that exact query
at `0ms` on the first try** — `Cancelled: Timeout expired`, same test, same
configuration, a different machine. Ten runs on one container is not a
property; it is ten samples of a race. The mechanism above is right and the
conclusion drawn from it was too strong: a handler that completes on its
*first* poll cannot be cancelled, and one that pends once — on a read that is
not ready yet, which is the scheduler's business — meets an already-elapsed
sleep on the next poll and is.

What stands is weaker and more useful than either version: a small
`request_timeout` is neither a latency ceiling nor a harmless setting. It is a
coin flip whose bias depends on the machine. The test that asserted the
five-out-of-five reading is deleted rather than loosened, and
`crates/slate-serverd/tests/ceilings.rs` now asserts only the deterministic
half; `ledger/2026-09-28-a-measurement-that-reversed-under-ci.md` is the
account.

This matters because `[limits] request_timeout` reads like a ceiling on request
latency and is a bound on requests that *wait*. Both are useful; they are not
the same setting, and only one of them is what the name suggests.

The concurrency half came back worse. `max_concurrent_requests` maps to tonic's
`concurrency_limit_per_connection`, a `tower` limit over the HTTP service whose
permit is held by the *response future*. `Query`, `Join` and `Aggregate` are all
server-streaming, so that future resolves once the headers are ready — before a
single row has been read. A held-open stream holds no permit. There is no way
from a client to keep one long enough to see the limit refuse anything, which
is why nothing here claims to have seen it.

## Alternatives rejected

**Make the query slow enough to overrun 1ms.** A few thousand seeded rows and a
regex would do it, and it would turn a deterministic test into a race between a
scan and a timer on whatever machine CI gives us. This repository has been
bitten by a timing assertion with a margin under a percentage point
(`2026-09-14-a-timer-that-can-be-wrong.md` and the wasm timing work); a test
that fires on a fast runner and not a slow one is worse than the admission.

**Add a fault-injection knob so a handler can be made to block.** It is the only
thing that would let a client observe either ceiling binding, and
`ledger/2026-09-18-the-failure-that-arrives-after-the-answer-has-started.md`
already declined the same trade for the same reason: a production-facing
setting added for a test, and a bigger decision than a test earns.

**Lower the parser's floor to nanoseconds so the timeout could be forced.**
Changing a configuration surface to make a test possible, and the value would
be one no deployment should ever set. The floor is stated in the refusal and
the refusal is now pinned by a test.

**Report the timeout finding and skip the tests.** The finding is the valuable
half, and four of the five tests still pin real behaviour — a zero limit refused
by name, a sub-millisecond timeout refused by name, a node that serves with each
setting present. Without them the finding would be a paragraph nothing defends.

## Evidence

Five tests, `cargo test -p slate-serverd --test ceilings`: 5 passed.

Two mutations, both caught by a *named* test:
`ledger/mutations/20260928T204430-crates-slate-serverd-src-main-rs.json` —
deleting the zero-concurrency refusal fails
`a_concurrency_limit_of_zero_is_refused_by_name`, and never reading
`request_timeout` from the file fails
`a_sub_millisecond_timeout_is_refused_by_name`.

The timeout measurement above is ten runs, five at each value, all from this
container. No run cancelled.

## What this does not do

**Nothing here has seen either ceiling bind.** The tests prove the values reach
tonic's builder and that the refusals fire; they do not prove a request was ever
refused for exceeding one, and the two paragraphs above are why that cannot be
shown from a client. That is the residual, narrowed from "untested end to end"
to "untestable from this side without a knob nobody should ship".

**The streaming observation is read, not measured.** That
`concurrency_limit_per_connection`'s permit is released when the response future
resolves is tower's documented shape and tonic's use of it, and I did not
instrument the server to watch a permit's lifetime. It is consistent with the
timeout result — both come from the same place, that a streaming RPC's handler
returns before its rows do — and consistency is not a measurement.

**No per-request ceiling was examined**, only the daemon's two. `ExecutionLimits`
has its own tests in the kernel and `ledger/2026-09-20-the-ceilings-and-the-paths-they-do-not-reach.md`
covers those paths.
