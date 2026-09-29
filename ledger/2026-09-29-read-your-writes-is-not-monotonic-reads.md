# Read-your-writes is not monotonic reads

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `clients/go/slate/{client.go,related_test.go}`, `clients/typescript/{src/client.ts,test/related.test.ts}`, `clients/python/tests/test_round_trips.py`
- **Kind:** fix

## What changed

Go and TypeScript adopted the Python client's semantics. The flag now decides
one thing instead of two:

- **A read no longer advances a non-monotonic session's watermark.** That is
  what `monotonic_reads` is named for and it is unchanged.
- **A write still advances it, always.** So a session that has written carries
  a freshness floor on its next read whether or not the flag is set, and reads
  its own writes.

One condition moved in each client, from the helper that *builds* the floor to
the one that folds in a *read's* `served_by`. The doc comments on
`SessionWithoutMonotonicReads` and `sessionWithoutMonotonicReads` no longer say
"does not carry its watermark", because it does.

## Why

`ledger/2026-09-29-the-third-client-sends-a-floor-the-other-two-do-not.md`
measured the divergence this morning and deliberately did not resolve it:

> Which side is right is a real decision and is not made here. … Changing
> either is a shipped client's contract, so it wants its own change with its
> own reasoning.

This is that change. The reasoning is three things.

**The flag is named for one guarantee and was switching off two.** Monotonic
reads means a later read never sees less than an earlier one. Read-your-writes
means a session sees its own commits. They are distinct, and a caller reaching
for the first has not asked to give up the second.

**The failure mode is silent.** A Go or TypeScript caller who turned the flag
on, wrote a row, and read it back could get the pre-write view from a lagging
replica, with no error anywhere. `ColumnDef.Scale`'s comment in this same
client makes the general form of the argument — the worst failures are the ones
that produce a plausible answer — and this is one.

**It costs the documented caller nothing.** Go's own doc names the use cases:
"a dashboard, a cache warmer". Both are read-only, so their watermark stays
unset and they still send no floor. The behaviour only changes for a session
that *writes*, which is exactly the caller for whom the old answer was a
surprise rather than a saving. That is what made this a resolution rather than
a trade.

## Alternatives rejected

**Change Python to match the other two.** The majority, and two-to-one is not
an argument. It would have taken the silent failure from two clients to three
and made the flag mean "no consistency of any kind" in all of them.

**Split the flag in two.** `monotonic_reads` and `read_your_writes`, separately
settable. Honest, and three clients' worth of surface for a combination nobody
has asked for: a caller who wants to miss their own writes has not appeared,
and inventing the knob is the thing to do when one does.

**Rename the Go and TypeScript constructors instead of changing them.**
`SessionWithoutAnyFreshness` would make the old behaviour accurate. It leaves
every existing caller on the surprising semantics and renames the surprise.

**Leave it, since each client documents what it does.** True and insufficient:
the three are meant to be one product, `clients/python`'s docstring and Go's
described different guarantees under one name, and a caller reading one and
using the other gets the gap. The conformance runner exists for precisely this
class and cannot reach a per-session flag.

## Evidence

- `clients/go`: `go test ./...` — **ok**, 141s. `clients/typescript`:
  **202 pass, 0 fail** (201 before). `clients/python`: **347 passed**,
  unchanged, and its assertion is unchanged — the point of having pinned the
  behaviour before the decision.
- `examples/explorer ./run.sh --conformance`: **140 cases, the three SDKs agree
  on all of them.**
- **Five mutations, and the fourth was a missing test.**
  - `ledger/mutations/20260929T051653-clients-go-slate-client-go.json`
    (`problems`, 2): restoring the old gate on `freshness()` is caught;
    **removing the new gate on `observeServedBy` survived**. Nothing tested the
    half of the flag that still does something — a read must not advance a
    loose session's watermark — because the existing case did *one* read, and
    with one read there is nothing for the first to have folded in.
  - The case written for it does two reads and asserts the second carries no
    floor. `ledger/mutations/20260929T051750-clients-go-slate-client-go.json`
    (`clean`, 1) catches the mutation now.
  - `ledger/mutations/20260929T051755-clients-typescript-src-client-ts.json`
    (`clean`, 2): both mutations caught, against the same pair of cases. The
    TypeScript version was written with the two-read shape from the start,
    because the Go survivor had just shown why.
- Three tests changed sides, which is the change being visible: the Go and
  TypeScript controls now assert the floor is *present* for a loose session
  that wrote, and the Python test that pinned the divergence pins the agreement
  with its assertion untouched.

## What this does not do

**No conformance case compares the three on this.** The runner reaches
endpoints, not per-session flags, and an endpoint existing only to expose one
is a worse home than the three suites that each have an interceptor. Stated in
the previous entry and unchanged: three tests agree because a person made them,
and a person is what would notice if they stopped.

**It does not touch `Freshness.any()` or an explicitly passed token.** A caller
who names a freshness level still gets exactly that, and a transaction's reads
still carry no floor because they go to the writer. Only the *implied* floor
moved.

**Nothing measures what the restored floor costs.** A session that writes and
then reads now pins that read to a view at least as new as its commit, where
before, with the flag off, it did not. On a lagging replica that is a slower
read or a hop to the writer. It is the cost of the guarantee and it is not
quantified here — `examples/deployed` is where a replica lags, and it does not
exercise this.

**The `monotonic` field is now consulted in exactly one place per client, and
nothing guards that.** The bug was one condition in the wrong function; the fix
is one condition in the right one. A future path that folds a read's
`served_by` in by another route would reintroduce it, and no rule would say so.
