# Joins and aggregates in the TypeScript client, and an equivalent mutant

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `clients/typescript/src/{join,client,index}.ts`,
  `clients/typescript/test/{join,wire}.test.ts`, both READMEs
- **Kind:** feature

## What changed

`join`, `aggregate`, `aggregateJoin` and `explainJoin` in the TypeScript
client, with the same `Column` model and the same grouping shape as the Go one.
Sixteen new tests, three of them with no server at all.

## Why

The third of three: the demo application runs one feature set through every
SDK, and a client missing joins would have set the floor for all of them. With
this, all three clients cover the same surface and the demo does not have to
pick a lowest common denominator.

## Alternatives rejected

**Following the Go client's first, wrong column model.** Porting it would have
been faster and would have carried the cumulative-offset mistake into a second
client — the version that needs table widths and therefore a catalog. The Go
ledger entry from earlier today has the detail; this client was written against
the corrected model from the start.

**Overloading `gt` for columns and group refs.** TypeScript could express it
with a union, and then a raw column in a `having` typechecks and is refused by
the server at runtime. A separate `group*` family makes it a compile error.

**Reusing `RowStream` for joins.** The element type is different — one array
per input, with `undefined` for an unmatched side — and flattening to reuse the
class would destroy the distinction between "no match" and "matched, all
nulls".

## Evidence

37 TypeScript tests pass; `tsc` clean under `strict`,
`noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`. Five mutations,
four killed outright: pinning the `own` side of a join condition to input 0
(ten failures), forcing every join to inner (two), dropping the forced
algorithm, and dropping the group sort.

The fifth is worth recording because it is **not** a missing test. Removing the
`a.function !== "count"` guard — so `COUNT(*)` would send a column — changed
nothing, because `count()` never sets one and `column` is optional. It is an
equivalent mutant *through the public constructors*, and a live one through a
hand-built `Aggregate`, which the type permits.

Rather than declare it equivalent and move on, `test/wire.test.ts` builds
`{ function: "count", column: at(0, 3) }` by hand and asserts the column does
not reach the wire. That kills it. Those tests assert on what is *not* sent,
which is the one thing a server-backed test cannot check: a server that ignores
a field it should never have received looks exactly like a client that never
sent it.

The forced-algorithm test asserts the two algorithms produce *different* plans
rather than that every algorithm agrees — the Go entry explains why agreement
is the wrong assertion there.

## What this does not do

No computed values, vectors or `SchemaCheck`, matching the Go client; the
Python one still has all three, and both READMEs now say which.

Still nothing compares the three clients' answers against each other. They are
each tested against the same server, which catches one client being wrong and
not three being wrong the same way.
