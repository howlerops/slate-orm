# The third client sends a floor the other two do not

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `clients/python/tests/test_round_trips.py`
- **Kind:** finding

## What changed

The Python client reads its freshness floor off the wire now, through a
`Recording` interceptor beside the `Counting` one already there. Two tests, and
the second is the finding:

**A non-monotonic Python session that has written still sends a floor. The Go
and TypeScript equivalents send none.** Measured in all three, this morning.

## Why

I opened this caveat an hour ago, closing the TypeScript half of the same gap:

> **The Python client still has no such assertion.** It has a `Counting`
> interceptor for round trips, so the door and the machinery are both there;
> what is missing is the case.
> — `ledger/2026-09-29-a-control-that-controlled-nothing-in-two-clients.md`

Writing the case is nine lines. What it found is not.

The three clients implement `monotonic_reads` in two different places:

- **Go** — `Session.freshness()` returns `nil` when `!s.monotonic`, so the flag
  is consulted where the floor is *built*. A session that wrote has a
  watermark and sends nothing.
- **TypeScript** — `#freshness()` is the same shape, the same result.
- **Python** — `_freshness()` consults only the watermark. The flag is
  consulted in `_observe_read`, where a *read's* `served_by` is folded in.
  `_observe`, which a write goes through, is ungated. So the watermark
  advances on a commit either way and the floor rides on the next read.

Each is self-consistent and each says what it does. `Session`'s docstring:
"from its own commits and — when `monotonic_reads` is on — from the views that
served its reads". Go's: the session "does not carry its watermark". They are
two different features under one name, and nothing had put them side by side
because the conformance runner compares answers and this is a per-session flag
that changes only the request.

**The consequence is not cosmetic.** In Go and TypeScript, a caller who turns
the flag off, writes, and reads back can miss their own write. In Python they
cannot.

## Alternatives rejected

**Pick a side now and change two clients.** The tempting close, and the wrong
shape for this change. Read-your-writes and monotonic reads are distinct
guarantees; a flag named for the second should arguably not switch off the
first, which argues for Python — and Go's documented contract is explicitly
"does not carry its watermark", which argues the other way and is what a Go
caller has been told. Changing either is a shipped client's contract and wants
its own change, with its own reasoning and its own alternatives, not a
paragraph inside a test's docstring.

**Write the Python test to match Go, and let it fail.** A red test is a way of
recording a decision that has not been made, and it would stay red. Pinning
today's behaviour makes the *change* visible when somebody makes it, which is
the same job without the broken build.

**A conformance case instead.** The natural home for a three-client
disagreement, and it cannot go there: the adapters expose one session per
identity and the flag is chosen when the session is made. Reaching it would
mean a new endpoint whose only purpose is this flag, which is a worse place for
it than the three suites that each already have an interceptor.

**Fold it into the previous entry.** It was found while closing that entry's
own caveat, an hour later, and a divergence between three shipped clients is
not a footnote to a test port.

## Evidence

- A probe, before any test was written: the same write-then-read against a live
  node with `monotonic_reads` both ways, reading `HasField("freshness")` off
  the intercepted `QueryRequest`. **True for both.** The Go and TypeScript
  answers are the tests written earlier today, which assert the floor is
  *absent* for the loose session — so the divergence is three measurements, not
  one measurement and two readings.
- `clients/python`: `python3 -m pytest -q` — **347 passed** (345 before).
- **Three mutations, all caught**
  (`ledger/mutations/20260929T044012-clients-python-src-slate-client-py.json`,
  outcome `clean`). Never sending the floor and never advancing the watermark
  on a write are each caught by both new tests. The third is the important
  one: **gating `_freshness` on `_monotonic_reads`, exactly as Go and
  TypeScript gate it, breaks only the divergence test** — which is the
  demonstration that the test is about the divergence and not about the floor
  in general.

## What this does not do

**It does not resolve the divergence.** Open, with the three behaviours
measured and the argument on both sides written down. Whoever picks a side
changes a shipped client's contract, and the one thing this makes easier is
seeing that they did.

**It does not check the other RPCs.** `Query` only, on the same reasoning the
Go and TypeScript versions give: the floor is built in one helper per client,
so a mutation there breaks every read and surfaces here. Nothing checks that
`Get` and `Related` actually call that helper — in any of the three.

**Nothing compares the three automatically.** Three suites, three
interceptors, three assertions that happen to disagree, and a person is what
noticed. The conformance runner is where a disagreement should surface and it
cannot reach this one; that is stated in the alternatives above and is not
fixed.

**The probe is not in the tree.** It ran from a scratch directory and its
result is transcribed here. What is committed is the Python assertion; the Go
and TypeScript sides of the comparison are their own tests, in their own
suites, and nothing joins the three except this entry.
