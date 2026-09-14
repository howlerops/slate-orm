# Conjunctions, joins and grouped reads, through the binding

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/playground.rs`
- **Kind:** feature

## What changed

Three widenings of the playground binding, all query-shaped:

- **`filters`**, a list of conditions ANDed together, beside the single
  `filter` the panel shipped with.
- **`join`**, pairing `authors` with `books` on `authors.id =
  books.author_id`, with conditions on either side.
- **Grouping** over that join — `count`, `min`, `max`, `sum`, `avg` — returning
  groups and the grouped plan.

## Why

The binding could express a filtered scan of one table, which is the least
interesting thing the kernel does. Joins are cost-chosen between hash and
nested loop, grouping narrows each input's projection so a `count(*)` per
author can avoid reading book rows entirely, and a conjunction is where the
planner's split between scan bounds and residual predicate becomes visible.
None of that was reachable.

## Alternatives rejected

**Letting the caller choose join keys.** The fixture has exactly one sensible
join, and offering a key picker would mostly produce empty results from
mismatched columns — a panel that makes nonsense easy to express teaches
nothing. The join is fixed; what varies is the filters and the grouping.

**Separate `join` and `groupedJoin` entry points.** They return different plans
for the same underlying read, and two entry points let the panel show one
beside the other's rows. One function, and the presence of `groupBy` decides —
the same reason `ExplainAggregate` exists as its own RPC rather than being
inferred.

**Folding conjuncts with `a.and(b).and(c)`.** `Expr::all` is the kernel's own
constructor and the planner reads conjuncts out of it looking for scan bounds.
A hand-folded tree of nested `And`s is the same predicate and gives the planner
more to undo.

**Dropping the single `filter` field once `filters` existed.** It is what a
one-condition panel sends and what the binding shipped with a few commits ago.
Both are accepted and both are ANDed in; a test asserts they agree.

**Testing the aggregates against the kernel.** Asking for a count and then
asking a different way is a self-consistency check that passes for a grouping
that is wrong in a self-consistent way. `a_grouped_join_agrees_with_counting_
the_fixture_by_hand` folds the fixture's own rows in Rust and requires the
grouped read to match — the same shape as the kernel's `aggregate_oracle`, for
the same reason.

## Evidence

Twenty-three tests, eight new. The join pairs every book with the right author
and reports two inputs; the grouped join's count matches a hand fold over the
fixture; `min`/`max` over the book side return 1968 and 1990 for Le Guin, which
are the fixture's own earliest and latest; grouping demonstrably narrows what
the books input decodes relative to the ungrouped join; a conjunction returns
only rows satisfying both and the plan names a residual; one filter and a list
of one agree; and unknown aggregates and contradictory conjunctions are refused
or empty rather than erroring.

## What this does not do

`authors` and `books` only — no chain of three, though the kernel has one.

The join is inner. Left, right and full outer exist in the kernel and would
mostly demonstrate the `—` placeholder the row renderer already carries for a
missing side.

No `HAVING` and no group ordering, both of which the kernel supports and the
wire protocol carries.

**A `books` ordinal is shifted into the joined row's space by hand**
(`authors.columns().len() + n`) when building an aggregate. That arithmetic is
exactly what `ColumnRef` exists to remove in the three SDKs, and doing it
manually here is a real wart — it is why the `min`/`max` test checks against
values computed from the fixture rather than against whatever the kernel
returned.

The panel exposes none of this yet. This commit is the binding and its tests;
the controls come next.
