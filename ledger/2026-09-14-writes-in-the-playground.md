# Writes, and the index answering for a row that did not exist a moment ago

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/playground.rs`
- **Kind:** feature

## What changed

`insert`, `update`, `delete` and `reset` on the `Playground` binding, each
taking one string per column and parsing it against that column's declared
type.

## Why

The playground could read and not write, which left out the half that makes
this a record layer rather than a query engine. Index maintenance is the
project's central claim — atomic, inside the same transaction as the row — and
a read-only demo can only assert it.

## How it is demonstrated, and why the control is the whole test

Insert a book for author 2, then run a query that is **index-only**, and see
the count go up.

The obvious version of this test — insert a row, check it comes back — proves
nothing. A table scan finds a row whether or not any index was maintained, so
that test passes against a store with no secondary index at all. Requiring
`Index Only Scan` in the plan means the answer came from the index entry, and
the index entry exists only because the write created it.

The same shape covers the other three directions: a delete must remove the
entry (a dangling one would still be counted by an index-only scan), an update
that changes the indexed column must *move* the entry rather than leave two,
and a refused insert must leave none behind.

## Alternatives rejected

**Exposing the index contents directly.** A `list_index_entries` on the binding
would make the demonstration trivial and would be a hole in the abstraction
that exists only to make a demo easier — nothing else in the system can read
index entries outside a plan, and adding a way to would mean the playground was
testing a surface no other caller has.

**Typed JSON values instead of strings per column.** An HTML form has strings,
and the parsing is worth showing rather than hiding: a `Str` written into a
`U64` column does not fail at the storage layer, because the kernel's value
order is type-first — it would sort among the strings and silently never match
a numeric predicate. Parsing at the edge, and naming the column in the
refusal, is the honest boundary.

**Leaving `reset` out.** The store lives in the tab, so a reader who deletes
half the fixture and reloads gets their own wreckage back — nothing restores
it. `reset` is not polish; without it the panel breaks permanently on first
curious use.

**A shared body for `insert`/`update` or two copies.** Shared: they differ by
one call, and everything before it — resolving the table, parsing a string per
column — is identical. Two copies is how the two drift on the next change.

## Evidence

Fifteen tests in `crates/slate-wasm/tests/playground.rs`, six of them new:
insert reaches the index without reading the row, delete leaves no entry,
update moves the entry rather than duplicating it, a duplicate primary key is
refused and writes nothing, a wrong-typed value is refused *by column name*,
and `reset` restores all 4,824 rows.

Mutation: widening the covering query's projection so the plan stops being
index-only makes the insert test fail on its premise rather than pass quietly —
which is the control doing its job.

## What this does not do

Writes go through the ordinary authorised path as the `app` role, so a reader
cannot see a write *refused* by RBAC or reshaped by a row policy. The explorer
demo is where that is visible, with three personas.

Nothing exercises a conflicting concurrent write, because there is one tab and
one writer. Retry and fencing are the head node's concern and are tested in
`slate-kernel` and `slate-server`.

No bulk path: `insert_many` exists in the kernel and one row at a time is what
a form produces.

The panel does not expose any of this yet — this commit is the binding and its
tests. The controls come next.
