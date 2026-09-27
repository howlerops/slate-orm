# A computed value over an outer join's unmatched row was reasoned about in a doc comment and demonstrated nowhere

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-15-a-chains-computed-value-and-a-type-tag-in-a-label.md`
- **Touches:** `crates/slate-kernel/tests/join.rs`,
  `crates/slate-kernel/tests/chain.rs`
- **Kind:** test

## What changed

Two tests, one per shape:

- `a_joins_computed_value_over_an_unmatched_outer_row_is_computed_from_nulls`
  — a right outer join of `authors` to `books`, computing
  `books.published + 1` and `authors.died`, over a fixture whose unmatched
  rows are a book naming an author that does not exist and a book naming
  none.
- `a_chains_computed_value_over_an_unmatched_step_reads_nulls` — a three-table
  chain, outer at both steps, computing `upper(authors.name)` and
  `publishers.house`, over rows where the last step matched, where it did not,
  and where *neither* step did.

## Why

`computed_values` opens with the rule:

> A missing side reads as null throughout, so a computed value over an outer
> join's unmatched row is computed from nulls rather than skipped.

and gives the argument for it: the alternative — a null slot regardless of the
expression — would make `coalesce(right.x, 0)` answer differently here than
anywhere else, which is exactly the case somebody writes `coalesce` *for*.
Nothing demonstrated it. Every join and chain fixture in the kernel suite
computes over inner joins.

**Two computed values in each test rather than one**, because one cannot tell
the two wrong answers apart. An expression over the *present* side must stay a
real value — a rule that nulled the whole computed tail whenever any slot was
empty would pass an assertion about the absent side and fail this. An
expression over the *absent* side must be null — an engine that skipped
computing over outer rows entirely would fail both.

## Alternatives rejected

**A property test against a fold.** The oracle suites already run every join
type against a brute-force fold, and adding computed values to
`join_oracle.rs` would cover far more shapes. Rejected because the fold would
have to implement `Scalar::evaluate` over a padded row — restating the rule
under test, which is the oracle's one failure mode. What is wanted here is a
written-out expectation of what the rule *is*, and five rows of it are
readable.

**One test covering both shapes.** A chain of two tables is a join, so the
chain test could have subsumed the other. Rejected because they go through
different code: a join flattens two rows and a chain accumulates n, and the
chain's version is the one where a row can be missing *two* tables at once —
which is the case the second assertion is really about.

**Assert only the absent side.** What the caveat literally asks for, and the
cheaper half. It would have been satisfied by an engine that computed nothing
at all over an outer row, which is the behaviour the doc comment explicitly
rejects.

## Evidence

- `cargo test -p slate-kernel --test join a_joins_computed` — 1 passed.
  `cargo test -p slate-kernel --test chain a_chains_computed` — 1 passed.
- One mutation via `scripts/mutate.py`, recorded as
  `ledger/mutations/20260927T031117-crates-slate-kernel-src-join-rs.json`:
  making `computed_values` return a null per expression as soon as any value in
  the flattened row is null — the plausible-looking implementation of "an outer
  row computes nothing" — is caught by **both** new tests and by nothing else
  in either suite. Before them it was a free change.
- The join test's expected values are written out per title
  (`A Wizard of Earthsea` → `1969`, `2018`; `Orphaned` → `2001`, null) rather
  than summarised, so a wrong answer names the row.

## What this does not do

**The client accessors on an unmatched side are still untested**, which is the
caveat `ledger/2026-09-15-the-values-that-arrived-and-vanished.md` carries and
this does not close. That one is about an *input's* own computed values —
`InputComputed` in Go, `inputComputed` in TypeScript — returning nil where the
input produced no row, which needs a live server and all three clients. The
kernel half is what is demonstrated here.

**Neither test covers a full outer join.** A right outer was enough to get an
unmatched row on the side the fixture has orphans on, and a full outer would
add unmatched rows on both sides at once — a fourth shape, and the one where
*every* table in a two-table join is absent from a row, which cannot happen
because a row with neither side is not a row.

**The expression under test is arithmetic and a column.** Nothing here computes
over an absent side with a function that has opinions about null —
`coalesce` is the case the doc comment names as the motivation, and it is not
among the `Scalar` variants exercised. What is pinned is that the row reaching
the expression contains nulls rather than being skipped, which is the property
every such function then depends on.
