# Grouping a chain, and withdrawing the costing item

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-kernel/src/{chain,join,read,record}.rs`,
  `crates/slate-kernel/tests/grouped_chain_oracle.rs` (new), `README.md`
- **Kind:** feature + a withdrawn claim

## What changed

`group_by_chain` on both the transaction and the snapshot: the n-way
generalisation of `group_by_join`. `ChainRow::flatten` and `JoinSchema::len`
support it. Six tests.

And one README item **withdrawn** rather than built — see below.

## Why

Two of the three remaining kernel gaps. Grouping a chain was the one the wire
refuses by name, so a caller reaching for it gets a reason rather than a wrong
answer — but a reason is not the feature.

## The withdrawn item

The README carried:

> A grouped join is not *costed*: the join is planned as if its rows were being
> returned, so a plan cheaper to group than to stream is not preferred.

Reading the cost model before implementing anything, the per-joined-row term is
added to **both** candidates:

```text
hash_cost = read(left) + read(right)    + rows * JOIN_ROW_COST
loop_cost = read(left) + probes * probe + rows * JOIN_ROW_COST
```

A term common to both sides of a comparison cannot decide it. "Costing a
grouped join as grouped" would mean discounting that term — which changes no
plan, ever. The item was based on a plausible reading of the code that does not
survive looking at the arithmetic.

Grouping *does* change the plan, and already did: `narrowed_join` reads only the
columns the grouping and the condition need, which can make a side index-only.
That half was built and tested some time ago.

Rather than delete the item quietly,
`the_per_row_term_is_symmetric_so_grouping_cannot_flip_the_algorithm` asserts
the symmetry the reasoning depends on. The day someone makes the per-row term
asymmetric, that test fails and the withdrawal is revisited.

## Alternatives rejected

**Narrowing each step's projection in `grouped_chain`, as the two-table version
does.** Tried and abandoned as unsound in the general case: a step's condition
may name *any* earlier table, so the columns a chain depends on are not the
columns the grouping asks for. Narrowing to the grouping's set reads away a
column a later step's `having` still needs. Doing it properly means the
transitive closure of every step's references, which is a real piece of work and
is not here — so a grouped chain reads wider than a grouped join, and `COUNT(*)`
over one does not get the index-only treatment. Said plainly in the doc comment
and in the README.

**Reusing `JoinedRow::flatten` by treating a chain as nested pairs.** Would make
the ordinal space depend on the nesting rather than on `JoinSchema::over`, so a
group key naming the third table would mean something different depending on how
the chain was built.

**A row-like view over `ChainRow` instead of flattening.** The accumulators take
a `Row`; a second row-like type threaded through them is a second place for the
null rules to drift. Same argument the two-table version already made.

## Evidence

941 workspace tests pass; fmt and clippy clean. Six new tests: a property test
against a hand fold over the ungrouped chain, a cross-check that a two-table
chain agrees with `group_by_join` (two implementations, one answer), an RLS
probe, two direct `flatten` tests, and the symmetry pin.

Four mutations, all killed — two only after the tests were strengthened:

- Dropping the nulls for an absent table, and not pushing rows into the
  grouper, fail immediately.
- **Not padding a short row, and iterating the row's length instead of the
  schema's, both survived every property test.** Both guards are only reachable
  for rows a finished chain does not produce — but both shapes are
  constructible through the public API, since `ChainRow::start` makes a short
  row and a projected step makes a narrow one. Two direct tests on `flatten`
  kill them. Same lesson as the TypeScript `COUNT(*)` guard earlier today: a
  guard that only a hand-built input reaches needs a hand-built input to test
  it.

## What this does not do

A grouped chain reads wider than it needs to, as above. It is correct and it is
not index-only.

There is no `EXPLAIN` for a grouped chain, or for a grouped join — the plan is
built and thrown away. Nothing depends on it today; a caller wanting to know
whether their grouped join went index-only has no way to ask.

`group_by_chain` is not on the wire. The server refuses a chain-shaped
`AggregateQuery` with "the kernel groups a two-table join and does not group a
chain", which is now false. That refusal is a lie as of this commit and the next
protocol change should carry the feature through — the message is in
`convert.rs` and its test is `grouping_a_chain_is_refused_with_the_reason`.
