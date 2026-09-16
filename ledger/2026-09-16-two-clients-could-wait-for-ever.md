# Two of the three clients passed no deadline at all, so a silent server blocked them for ever

- **Date:** 2026-09-16
- **Author:** Claude Code
- **Touches:** `clients/python/src/slate/client.py`,
  `clients/typescript/src/client.ts`, a test file and a README section in each
  of the three clients
- **Kind:** fix

## What changed

The Python and TypeScript clients now carry a per-call deadline, through a
*view* rather than a setting:

```python
client = Client("127.0.0.1:50051", timeout=5.0)   # seconds
rows = list(client.with_timeout(60.0).query(Query(TRIPS)))
```

```ts
const rows = await client.withTimeout(60_000).session()   // milliseconds
  .query({ table: "trips" }).collect();
```

Each returns a new object over the same connection — in Python the same
freshness scope too, shared by reference; in TypeScript the same identity and
schema declarations. The Go client needed nothing added, and gained the tests
that turn that into a fact rather than a claim.

## Why

Neither client passed a `timeout` or a `CallOptions` on any RPC. A head node
that accepted a connection and then stopped answering — a wedged process, a
half-open connection a middlebox has forgotten, a node that lost its lease and
is stuck — blocked the caller **for ever**.

That is the one failure an error taxonomy cannot help with, and both clients
have a good one: `Conflict`, `Unavailable`, `NotLeader`, `UnknownOutcome`,
`DeadlineExceeded`, each with a note on whether retrying is safe. Every one of
them presumes an error *arrives*. A hang produces none.

It was also invisible to everything that checks these clients. The conformance
runner compares the three SDKs' answers and requires them byte-identical, which
is exactly the wrong instrument: all three answer identically, and one of them
could not be made to give up. `docs/correctness.md` calls this shape out for
other reasons — a check that agrees with itself.

The unit differs on purpose: seconds in Python, milliseconds in TypeScript,
each matching its own language. That is a real trap for someone reading one
client and writing the other, so both name the unit in the parameter and both
READMEs say what the other one does.

## Alternatives rejected

**A default deadline.** The tempting fix, and wrong: every existing caller that
upgraded would find a slow query turning into a failure, with no line in their
own code to blame. The default stays `None`, and each client's README states
the cost of that in the same paragraph as the feature — a silent server still
blocks a caller who does not set one.

**One deadline per connection.** Smaller surface, and it cannot be right: a
gRPC deadline covers a *whole streaming call*, so one number has to be either
too short for a hundred-thousand-row scan or too long for a point get. That is
why it is a view rather than a constructor argument, although the constructor
takes one too as the floor.

**A `timeout=` keyword on every operation.** The most granular option, and it
means editing fifteen signatures in Python and every method in TypeScript, with
the deadline threaded through `Transaction` by hand at each one. A view gets the
same granularity for one method and composes.

**A mutable setting, or a context manager that swaps it for a scope.** Either
lets a short deadline survive the block that wanted it, because a caller forgets
a restore or an exception skips it. A new object cannot be left switched on, and
can be handed to another thread while the original is in use.

**Adding a `context.Context` equivalent to Python and TypeScript.** The Go
shape, and it does not belong in either language: `context` is idiomatic Go and
a parameter neither of the other two has any tradition of threading.

**A request id, which this task also named.** Not built, and the reason is worth
recording: `slate-serverd` logs its startup line and its warnings and **nothing
per request**. An id sent from a client would have nothing on the server to be
correlated against, so it would be a header nobody could use. The useful order
is a per-request log first, and that is a server change.

## Evidence

The interesting test in each client is not the one that shows the deadline
working. It is the one that shows the hang:

```
test_a_call_with_no_deadline_does_not_return   (python)
a call with no deadline does not come back     (typescript)
TestACallWithNoDeadlineDoesNotReturn           (go)
```

Each runs a call with no deadline against a listener that accepts connections
and never speaks, and asserts it has *not* returned two seconds later. Not a
slow server and not a closed port — either produces a different error; this is
the one shape where the caller's own deadline is the only thing that can end the
call, because gRPC completes the TCP connect and then waits for a server preface
that never comes.

Beside each: the same call with a one-second deadline fails as
`DeadlineExceeded` / `deadline-exceeded` / `KindDeadlineExceeded`, in **between
one and six seconds** — bounded both ways, because coming back too fast would
mean something else failed the call and the deadline proved nothing.

Unary and streaming are separate lines in both clients, so both are tested
separately. **Mutations**, four, all caught:

| mutation | caught by |
|---|---|
| Python `_unary` drops `timeout=` | `test_a_silent_server_is_a_deadline_rather_than_a_hang` |
| Python `_stream` drops `timeout=` | `test_the_deadline_reaches_a_streaming_call_too` |
| TypeScript `call` drops `#options()` | "a silent server is a deadline rather than a hang" |
| TypeScript `stream` drops `#options()` | "the deadline reaches a streaming call too" |

The TypeScript pair needed a fix before they were honest. The first run of that
mutation **hung the whole suite** rather than failing it — the call never
returns, so `node --test` sat with no output until the harness killed it at ten
minutes. A hanging test reads as broken infrastructure rather than a broken
assertion, which this repository has already paid for once (see the pagination
entry). Both now carry `{ timeout: GRACE_MS * 8 }`, and the mutation fails in
about a second.

The view semantics are tested too, and the assertions are chosen to catch the
obvious wrong implementation rather than to restate the right one:

- **Python** `test_a_view_shares_the_freshness_scope_it_came_from` writes
  through the view and reads through the original. Copying the watermark
  instead of sharing it would break this and nothing else in the suite, because
  every other test uses one session.
- **TypeScript** "a view does not change the client it came from" asserts on the
  *original*, which is what `withTimeout` implemented as
  `this.#timeoutMs = ms; return this` would break. It deliberately does not
  assert that a 1 ms view fails, because "1 ms is too short for a real call" is
  a race rather than a property.
- **TypeScript** "a view keeps the schema declarations it came from" covers the
  one piece of state on `Client` that is not the connection. Dropping it fails
  *open* — a request with no schema claim is served exactly as before — so
  nothing else would have noticed.

Suites run locally against a real daemon, not only in CI: Python 173 passed, Go
whole suite with `go vet` clean, TypeScript 73 passed.

## What this does not do

**No retrying `transact` in Go or TypeScript.** Python has one, with full-jitter
exponential backoff on a conflict; the other two offer `Begin`/`Commit`/
`Rollback` and leave every caller to write their own. That is a real divergence
and it is in the README's "Not built" — it is a feature rather than the fix this
entry is about, and bundling them would have made both harder to review.

**Nothing changed on the server.** A deadline that expires client-side leaves
the server finishing the work it was asked for; gRPC cancellation is delivered,
but nothing here checks that the server acts on it promptly.

**No default, so the hang is still reachable** by a caller who sets no deadline
— which is every existing caller. The three READMEs say so in the same paragraph
as the feature, rather than leaving it to be discovered.

**The Go half added no code**, only tests. Worth stating plainly: "Go was
already fine" was true, and it was also untested, and those are different
things.
