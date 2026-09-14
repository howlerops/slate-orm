# A grouped chain reads only what it needs, and the closure I feared was one pass

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-kernel/src/read.rs`,
  `crates/slate-kernel/tests/grouped_chain_oracle.rs`, `README.md`
- **Kind:** performance

## What changed

`narrowed_chain`: every step of a grouped chain now projects only the columns
something downstream takes out of its row. Two tests, one counting reads and one
guarding the case that makes a chain different from a join.

## Why

Yesterday's grouped chain ran the caller's chain unnarrowed, so every step read
every column and `COUNT(*)` over a chain could never be served from an index.
The two-table version had narrowed since it was written; the chain shipped
without it, with a doc comment saying so.

## The thing I got wrong yesterday

That comment called the fix "the transitive closure of every step's
references", and framed it as substantial work. It is one pass.

Every reference a chain makes is written in the **joined space**, which is
absolute: a step's `having` names column 7 of the chain, not "the column two to
the left of whatever that other step needed". Needing a column therefore never
creates a need for a different column, and there is no closure to compute — just
a gather from four places (the grouping, each step's `having`, each step's join
keys on both sides).

I recorded it as harder than it was, and the record was load-bearing: it is why
the feature shipped without it.

## Alternatives rejected

**Narrowing to the grouping's columns, as the two-table version does.** This is
the trap the deferral was avoiding, and it is real: a step's `having` may name
any table read before it, so a projection chosen from the grouping alone reads
away a column a later step needs. The symptom is a null, not an error.
`narrowing_keeps_a_column_a_later_step_names` is that case, and it fails if the
gather skips the steps.

**Adding filter columns to the projection.** Not needed, and it took reading
`narrowed` to be sure: the projection says what a row *carries*, and the planner
computes what to *decode* from the secured predicate. A filter on an unprojected
column still works. That is late materialisation doing its job.

**Keeping the chain's `limit` and `offset`.** Dropped, as the two-table version
drops them, for the reason recorded there: aggregating a windowed subset of an
unordered result is not a meaningful request, and leaving it in once made two
join algorithms disagree about the same grouped join.

## Evidence

8 tests in the chain oracle, all passing. Four mutations, all killed: dropping
the grouping's columns, dropping each step's `having` columns, and dropping
either side of a join key each fail between one and four named tests.

The narrowing is *measured*, not asserted. `a_grouped_chain_reads_only_what_something_downstream_needs`
forces the second table onto an index holding only the join key and counts
reads: a grouping that needs nothing from the book row reads strictly fewer
rows than one aggregating `year`, which only the row holds. A correctness test
cannot see this — the answers are identical either way.

## What this does not do

The gather is per *table*, not per *row shape*: a chain that joins the same
table twice at different positions gets the union of both positions' needs at
each. Correct, and wider than necessary in a case the fixture does not have.

Nothing narrows the *first* table's projection differently from the rest; it
goes through the same gather, which is right, but it means a chain whose first
table is only ever used as a join key still reads that key — as it must.
