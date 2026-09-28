# The other forty-eight open caveats, read: three were answered the same afternoon they were written, and nobody told the tracker

- **Date:** 2026-09-28
- **Author:** Claude Code, finishing task J3
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

The remaining forty-eight of the ninety-nine pre-today `open` caveats were read
against the tree, which finishes the census `ledger/2026-09-28-fifty-one-of-
ninety-nine-read-at-random.md` left at fifty-one. **Forty-five hold. Three
moved** — one to `closed`, two to `narrowed` with a residual each.

The three are the interesting part, because all three were answered by commits
made **within eighty minutes of the entry that recorded them**, by the same
session, and no verdict was updated:

| caveat | the commit that answered it | gap |
|---|---|---|
| "the predicate paths were probed at one shape" | `9578e1b` *Run the predicate shape an earlier entry named as unrun* | 65 min |
| "three paths remain unprobed" (two of three) | `5561a77`, `4beac75` | 35 and 57 min |
| "it does not build a factory" (two of three clauses) | `a82915b` *Generate seed rows from the table, which already knows how* | 17 min |

The first of those commits quotes the caveat in the test's own comment. The
verdict said `open` for eight days anyway.

## Why

Task J3 asked whether the open caveats are still true, and the honest answer
until now was "1 in 51, measured on a random draw, bound 0.05%–10.5%". Reading
the other forty-eight closes the census at **4 of 99, 95% interval 1.1%–10.0%**
(Clopper-Pearson, computed rather than looked up). The point estimate barely
moves; what the second half buys is not a tighter bound but a *cause*.

All four stale caveats in the whole census have the same shape, and it is not
the shape the backlog's length suggests. None is a caveat that rotted slowly as
the tree drifted around it. Every one was **overtaken by the next thing the
same session did**, usually within the hour, by a commit whose message names
the gap. The tracker's failure mode is not decay. It is that a session writes
down what it did not do, then does it, and closes the loop in the code and not
in the record.

That reframes what a tracker is for here. Re-reading the backlog on a schedule
is aimed at drift and finds almost nothing, which is what the first sample
already said. The productive moment is the one where a session is about to
write "this closes X" in a commit message.

## Alternatives rejected

**A guard that flags an open caveat whose entry is cited from a source file
that changed after the entry landed.** This looked exactly right — `9578e1b`'s
test cites `ledger/2026-09-20-four-say-absent-one-says-present.md` by name in
its own comment, which is how this session found that one. Measured before
building, as the deliberate-verdict guard earlier today should have been and
was:

- 216 distinct ledger entries are cited from somewhere outside `ledger/`.
- 65 of them carry an `open` or `narrowed` caveat, covering **82 of the 193**
  open-or-narrowed caveats — 42%.
- Restricting to citations whose *file* changed after the entry landed removes
  almost nothing: **65 of 193**, 34%.

A flag that raises a third of the backlog to find a 4% population is not a
signal, it is a slower way to read the backlog. And the decisive number is
recall, not precision: `2026-09-21-the-rust-seeding-entry-point-was-never-
missing.md` is cited from **no source file at all** — `crates/slate-orm/src/
factory.rs`, the thing that answered two thirds of its caveat seventeen minutes
later, never mentions it. The guard would have missed one of the three it was
designed for. Rejected on both halves, and this is the second guard measured
and withdrawn today rather than built and trusted.

**Reading the forty-eight and stopping at the first finding.** Three were found
and the third was in the last third of the list; stopping early would have
produced "2 of 30" and a different, wronger story about the rate.

**Closing "three paths remain unprobed" outright.** Two of its three are
probed. The third — `purge_deleted` on a table whose rows a policy hides — has
eight purge tests in `crates/slate-kernel/tests/soft_delete.rs` around it and
not one constructs a row filter. Recording it `closed` because most of it is
done is exactly the "the work happened and answered the easier half" failure
that `check_closed_caveats.py`'s own docstring was written about.

**Closing the factory caveat outright.** The factory generates a plausible row
and sequences a key, which is two of its three clauses. The third —
assembling a graph of related records — is not absent by oversight:
`Factory::cycle` documents why the factory will not read the store to find
parents itself, since that would make generation `async` and tie it to a
transaction. `narrowed` with that as the residual says what is true; `closed`
would have thrown away a design decision worth keeping.

## Evidence

- The draw is the one the previous two entries recorded and is reproducible
  against a *historical* file, which matters because the population has moved:
  `git show 13cc4cb:docs/caveat-status.json`, `open` verdicts whose entry does
  not start `2026-09-28`, 99 of them. `random.seed(20260928)` then
  `random.sample(pop, 12)` reproduces draw A exactly; the same seed then
  `random.shuffle` reproduces draw B, with **overlap 1 and union 51**, which is
  the number the previous entry reported and is how this reproduction was
  verified rather than assumed. This entry read `order[40:]` — the other 48.
- **45 held.** Twenty-two were settled by an absence a `git grep` over every
  place the thing could live returns nothing for: no `prepare` script in any
  `package.json`, no purge bench, no teardown test, no node-wide `Semaphore`,
  no `returning_projection` in the proto, no `AS OF` in `slate-sql`, no offline
  purge tool, no kernel-side counters, no expect-a-failure in the conformance
  harness, no rename test in the Python client, no freshness assertion in the
  TypeScript suite, no scale on any client's `Scalar`, and ten more. The rest
  were settled by reading the code or the corpus.
- **3 moved**, each verified by reading the test or module that answers it, and
  each dated against the entry with `git log -S`:
  `a_predicate_that_selects_only_the_retired_row_touches_nothing` (`9578e1b`,
  16:40 against the entry's 15:35),
  `a_cascade_edge_leaves_an_already_retired_child_and_its_timestamp_alone`
  (`5561a77`, 16:10) and `the_bulk_writes_restore_a_retired_row_too`
  (`4beac75`, 16:32), and `crates/slate-orm/src/factory.rs` (`a82915b`, 00:37
  against the entry's 00:20).
- The rejected guard's numbers are above and were computed over the tree, not
  estimated: 216 / 65 / 82 / 65, against 193 open-or-narrowed.
- Rates, exact binomial, computed here: this half **3/48 = 6.3%
  [1.3%, 17.2%]**; the earlier half **1/51 = 2.0% [0.05%, 10.5%]**; the census
  **4/99 = 4.0% [1.1%, 10.0%]**. The two halves are consistent with each other,
  which is the one thing worth saying about the difference between 6.3% and
  2.0% — it is noise at these counts, not evidence that the second half was
  read harder.
- `python3 scripts/caveats.py`: 1599 caveats, 124 open, 69 narrowed, 378
  closed, 895 deliberate, 0 untriaged. `sh scripts/check.sh`: 72 passed.

## What this does not do

**It fixes none of the forty-five.** They are true, which means the gaps they
name are still there. A census measures the record, not the system, and this
one says the record is accurate — not that the system is finished.

**The census is of `open` only.** 895 `deliberate` verdicts remain, sampled
once today at 15 and found 1 false. They outnumber the open ones seven to one
and a `deliberate` that has stopped being defensible is invisible; that is now
by a wide margin the largest unexamined population in the tracker, and no cheap
route into it has survived measurement.

**Nothing acts on the cause this entry identifies.** "The stale ones are the
ones the same session overtook" is a finding with an obvious remedy — check the
entry you are citing when you close a gap — and the obvious mechanical version
of it was measured above and does not work. What would work is a convention in
`ledger/README.md`, and a convention is not a check.

**A caveat settled by an absence is settled less thoroughly than one settled by
reading a function.** Twenty-two of the forty-five are the former, and this is
the same residual the previous entry recorded for the same reason: a `git grep`
that returns nothing proves the needle is absent, not that the thing is.
