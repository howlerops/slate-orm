# A retrying transact in Go and TypeScript

- **Date:** 2026-09-18
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `clients/go/slate/{transact.go,transact_test.go}` (new), `clients/typescript/src/client.ts`, `clients/typescript/test/transact.test.ts` (new), both client READMEs, `README.md`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

Go gets `slate.Transact`; TypeScript gets `session.transact`. Both run a body
in a transaction, commit on success, roll back on any error, and retry a
conflict with full-jitter exponential backoff. Three tests each.

Also: three rows retired from the gap table in `docs/orm-comparison.md` —
many-to-many on the wire, nested eager loading on the wire, and keyset paging
over a join — all built by N1 and N3, all still listed as missing with proto
evidence that is now false.

## Why

Python has had `transact` since the SDK existed. Go and TypeScript exposed
`retryable` on the error and offered nothing that used it, so every caller
wrote the backoff by hand or — far more likely — did not retry at all. The
README called this "the divergence the conformance runner cannot see, because
it compares answers rather than ergonomics", which is exactly right: three
clients that agree on every answer and disagree on whether a conflicting write
survives are not the same SDK.

## What is retried, and what is not

`Conflict` and nothing else, which is `RecordStore::transact`'s own rule: a
unique violation, an access denial or a fenced writer fails identically
forever, and retrying them turns a clear error into a hang.

**`Unavailable` and `NotLeader` are not retried, although both report
`retryable`.** They are retryable *somewhere* — against a different node — and
these functions have only the node they were given. Retrying here spends the
caller's attempts on a node that will keep saying no. Python's docstring
already made this argument; the other two now make it in their own words.

Defaults match Python's exactly — five attempts, 5 ms doubling to 500 ms, full
jitter. Three clients disagreeing about how hard they try is its own bug.

## Alternatives rejected

**A method on `*Session` in Go.** The obvious shape, and impossible: Go has no
generic methods, so it would return `any` or `error` alone — which is the
version every caller wraps to get their value back out. A generic free function
is the only shape that lets the body return something. Named in the doc comment
so the next person does not try.

**A `using` block in TypeScript.** Explicit resource management would give
`await using tx = session.begin()` with automatic rollback, and cannot retry:
retrying means running the body again and a block cannot re-run itself. Python
makes the same split for the same reason — `transaction()` is the context
manager, `transact()` takes a function.

**Retrying everything `retryable` says is retryable.** Rejected above. It would
also make the flag useless: a caller who wants a different policy needs the
flag to mean "could work again", not "this function will do it for you".

**Returning a wrapper naming the attempt count on the last failure.** Rejected:
a caller matching on `KindConflict` should not have to unwrap a count to do it.
The last attempt returns the conflict itself.

**Backoff that ignores the caller's context in Go.** The `select` on
`ctx.Done()` returns the *conflict* rather than `ctx.Err()`, because "this kept
conflicting" is more useful than "time ran out" and the deadline is visible on
the context anyway.

## Evidence

Go's suite, 131 TypeScript tests, 83 conformance cases still agreeing.

**Both retries are mutation-tested, because a retry that never retries passes
every happy-path test.** Forcing the first failure to propagate — `if (true ||
…)` in each — fails `TestTransactRetriesAConflictAndLosesNoUpdate` and
`transact retries a conflict and loses no update`, and nothing else.

**The conflict is forced rather than hoped for.** Both tests put two writers on
a barrier so each has read the counter before either commits, then assert the
counter reached 2. Without the barrier the two would almost always run serially,
the retry would never fire, and the test would pass against an implementation
that did not retry at all — which is the failure mode this shape exists to
avoid. The tests also assert that *some* attempt happened twice, so a barrier
that stopped working would fail loudly rather than silently weakening the test.

**A permanent failure is asserted to run once.** A duplicate key is inserted and
the body counts its own calls: `tries != 1` fails. Without it, a `Transact` that
retried everything would pass the other two tests and hang in production.

## What this does not do

**No retry budget across calls.** Each `Transact` gets its own five attempts, so
a caller in a loop can retry indefinitely in aggregate. That is the same shape
Python has and the same shape most clients have; a shared budget needs a
policy object with state, which is a bigger thing than this.

**Go's `Transact` cannot be used as a method value**, because it is a function
taking the session. `session.Transact(...)` does not exist and will read as
missing to somebody scanning the type.

**Nothing measures it.** A retrying write costs at least one wasted round trip
per conflict; no number here says what that is under contention, and
`examples/batchbench` measures uncontended writes only.
