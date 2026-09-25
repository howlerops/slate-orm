# I wrote "no client can send one" twice in one afternoon. Both were wrong: the wire has carried `Expr.disjunction` all along and all three clients have a builder. A live test now says so.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `clients/python/tests/test_disjunction.py`, and corrections in two entries from earlier today
- **Kind:** withdrawing a wrong claim, and executing the thing it was wrong about

## What changed

A new Python test sends a disjunction through a real head node — in a
`WHERE` and in a `HAVING` — and asserts the answer is the **union of the
arms**, with the arms required to differ so a server that ANDed them would
fail rather than coincide. It also asserts the conjunction *is* smaller,
which is what makes the union assertion a test of the connective rather than
of the fixture.

Both caveats are struck through where they stand, with the correction beside
them, as `ledger/README.md` asks.

## Why

Because I wrote a claim about the protocol from having just edited the SQL
front end, and nobody — including me — had executed it. The section below
says exactly how that happened, because the mechanism matters more than the
fact: it was not carelessness about one detail, it was generalising from the
layer in front of me to the whole system.

## What I got wrong, and how

Two entries earlier today ended with variations on *"`any_of` is a
`slate-sql` spec field, and the gRPC `Query` has no equivalent — the three
SDKs build predicates from typed builders that have no disjunction either."*

Every clause of that is false:

- `Expr.disjunction` is **field 9** of the wire's `Expr` message, and has
  been since the protocol carried expressions at all.
- `convert.rs` maps `Expr::Or` to it and back — both directions, two lines
  apart.
- Python has `any_of` and `|`. Go has `Disjunction`. TypeScript has `or`.
  `AggregateQuery.having` takes an `Expr`, so an ORed `HAVING` was reachable
  too.

The mechanism is worth naming because it is not carelessness about one fact.
**I had spent the afternoon inside the SQL front end**, where the spec shape
genuinely had no disjunction and adding one was the work. When I wrote the
caveat I generalised from the layer I was editing to the whole system,
without going to look. The sentence reads as a fact and was an inference.

What made it survivable is that it was written *down*, in a place a tracker
reads, so it came back as an open item and the first thing I did with it was
check. What made it possible is that nothing executed the claim either way —
the same shape as the stale grammar comment fixed this morning, one layer up.

## Alternatives rejected

**Quietly delete the caveats.** They were wrong when written, which is
exactly the case `ledger/README.md` reserves the strikethrough for. Deleting
would leave two entries that had never been wrong about anything, which is
not what happened.

**Correct the text in place without a test.** The error was writing a claim
about behaviour from reading. Fixing it by writing a different claim about
behaviour from reading would repeat it.

**Test it in all three clients now.** Python only, deliberately: the point of
this file is to falsify a claim I made, and one live path does that. Go and
TypeScript have the builders and no live test, which is a real and much
narrower caveat, recorded below.

## Evidence

`clients/python` suite: **331 passed.** The two new tests exercise a real
`slate-serverd` through the ordinary client.

The `WHERE` test derives its thresholds from the rows — median size, and the
first row's kind — rather than writing them down. The first version used
`size > 100`, matched nothing, and **failed on its fixture rather than on its
subject**, which is the kind of test that gets weakened until it passes. The
median splits the table whatever the seed holds.

No mutation run: the change is a test, and the code it exercises is the
wire conversion, which `tests/wire.rs` already covers as a round trip.

## What this does not do

~~**Go and TypeScript have no live test of their disjunction builders.** They
have `Disjunction` and `or`, and the conformance runner has no case that uses
either. That is the accurate version of the claim I withdrew, and it is
smaller by a lot: a missing test, not a missing capability.~~

**Withdrawn** — the second sentence is wrong, and it is the *third* time in
one day I stated something about a layer I had not opened. The conformance
case `a negated disjunction` has sent `{"op": "or", ...}` to all three
adapters since 2026-09-13 (`3482995`), and every adapter routes it to its own
builder: `slate.Or` in Go, `or` in TypeScript, `any_of` in Python. Mutating
the Go adapter's `or` arm to `slate.And` is caught by that case. So the
builders were exercised and agreed three ways; what was missing was a live
test in Go's and TypeScript's *own* suites and an un-negated case in the
runner. Both now exist —
`ledger/2026-09-25-every-client-sends-a-disjunction.md`.

**It does not audit my other caveats for the same class.** I wrote roughly
thirty today. One more of them has since been read and found wrong — the
first caveat above, struck through — which is two checked out of thirty and
two wrong. At least two were inferences stated as facts, and the only one
I have checked is the one a later task happened to touch. The same reading is
owed to the rest, and a reader should treat any caveat of mine that describes
a *layer I was not editing* with more suspicion than one that describes the
diff in front of it.

**The union assertion is over one fixture.** `docs` has a few dozen rows. The
property holds for any data where the arms differ, which the test checks, but
it is not a property test and does not generate.
