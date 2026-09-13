# Joins and aggregates in the Go client, and a column model I had wrong

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `clients/go/slate/{join,client}.go`, `clients/go/slate/join_test.go`,
  `clients/go/README.md`
- **Kind:** feature

## What changed

`JoinQuery`, `Grouping`, `Join`, `Aggregate`, `AggregateJoin` and `ExplainJoin`
in the Go client, with thirteen tests. The client now covers everything the
Python one does except computed values, vectors and schema checks.

## Why

Needed on its own — a client that cannot join is a client for a different
database — and needed *now* because the demo application asked for next runs
the same feature set through all three SDKs, and would otherwise have had to
degrade to whichever surface the thinnest client had.

## The thing I got wrong

I first modelled a join column as a **cumulative offset into the flattened
joined row**: input 1's column 2 would be `width(input 0) + 2`. That needed
each table's width, so `JoinBuilder.Add` took one, and the client therefore
needed to know schema it has no business knowing.

It is also simply not what the wire carries. A `ColumnRef` holds an *input
index* and that input's *own* ordinal. The server caught it immediately — "a
join equality's own side names input 0, and is evaluated over input 1" — which
is the refusal doing exactly its job.

The corrected `Column` is `At(input, ordinal)`, there is no arithmetic to do,
and the builder no longer wants widths. A worse version of this change ships
the width-taking API and is wrong in a way that only shows up on tables of
unequal width.

## Alternatives rejected

**One `Aggregate` method taking either a table or a join.** Go has no sum type
that reads well here, and an `any` parameter would move the error from the
compiler to the server. Two methods, `Aggregate` and `AggregateJoin`.

**Overloading `Gt` for both columns and group refs.** Would need `any` or an
interface, and a raw column in a `HAVING` is a kind mismatch the server refuses
— so the wrong one should not compile. A separate `Group*` family costs six
short functions and makes the mistake unreachable.

**Concatenating a joined row into one flat slice.** Shorter for the caller and
loses information: an outer join's unmatched side becomes indistinguishable
from a matched side whose columns are null. One slice per input, `nil` for no
match.

**Counting inputs in the client for `AggregateJoin`.** Same argument as the
Python client: the count is the kernel's, and a copy here is free to drift.

## Evidence

29 Go tests pass, `go vet` and `gofmt` clean. Six mutations, all killed — but
two only after the tests were strengthened:

- Pinning the `own` side of a join condition to input 0, dropping the join
  type, sending a column with `COUNT(*)`, and dropping the group sort all fail
  immediately.
- **Dropping the forced-algorithm flag survived.** `TestEveryJoinAlgorithmAgrees`
  runs the four algorithms and checks they return the same rows — which is
  exactly what happens when the flag is ignored and all four use the planner's
  choice. Agreement was the assertion, and agreement is what the bug produces.
  `TestAForcedAlgorithmReachesThePlanner` asserts on `ExplainJoin`'s reported
  algorithm instead, where forcing is actually visible.
- The chain-refusal test passed nothing at first: I asserted on the error from
  the *call*, and opening a server stream does not wait for the server to
  accept the request. It drains the stream now. A version of this test that
  only checks the call passes against a client that never sends the request.

## What this does not do

No computed values in a query or join input, no vector similarity, no
`SchemaCheck` — so this client cannot detect a schema it disagrees with, where
the Python one can. Stated in the README rather than left to be discovered.

Nothing here compares the Go client's answers against the Python client's. Both
are tested against the same server, which catches a client that is wrong and
not two that are wrong identically.
