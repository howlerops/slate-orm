# Acting on the measurement, and finding out why a mutation survives

- **Date:** 2026-09-20
- **Author:** Claude, on the "what this does not do" of the entry that measured
- **Touches:** `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** performance

## What changed

`write_many` builds its security filters once per batch instead of twice per
row. **~7% off a 5,000-row `update_many`**, reproduced across runs.

Plus a test for a decision that was documented and asserted nowhere, and a
comment explaining why a mutation of the new code survives — which is a fact
about the code, not a missing case.

## Why

`ledger/2026-09-20-the-number-that-was-not-there.md` found the grant hoist
invisible in the noise and closed with:

> **It does not reopen the filter build.** Identifying `permits_row_with`'s
> per-row `Expr` construction as the dominant cost is the interesting part of
> this result and nothing here acts on it.

This acts on it. `permits_row_with` and `check_row` each *build* an expression —
the tenant restriction conjoined with the OR of every applicable policy — and
the loop called them twice per row for a value that cannot change between rows
of one statement.

**Why holding a filter across rows is safe, and why the answer is not obvious.**
A policy is a function of the context and may read a clock; this session
demonstrated exactly that in
`an_undo_window_can_be_a_policy_rather_than_a_role`, where the predicate means
something different as the window closes. A filter held across two *statements*
could therefore apply yesterday's window to today's write. Held across the rows
of one `write_many` it cannot: they are one statement, submitted at one instant,
and evaluating them against one another's clocks would be the anomaly rather
than the fix. That the hoist is safe is a consequence of the batch being a
statement, not of policies being static — they are not.

## Alternatives rejected

**Cache the filter on the transaction, keyed by (context, table, action).** It
would help the single-row paths too, which this does not. Rejected on the
paragraph above: a transaction spans statements, so a cached filter would
outlive the instant it was built for, and the failure would be a stale policy
silently applied — invisible, and wrong in the direction that matters.

**Make `Policy` return a static `Expr` so filters could be cached freely.** It
would make the whole question go away and it would delete the clock-reading
policy this session just demonstrated, which is the only shape that expresses a
time-bounded undo window. Trading a real capability for a 7% batch write is
the wrong way round.

**Leave it: 7% is not much.** True, and it is a 7% that costs three lines and
has a measurement behind it rather than an argument. The repository has just
spent an entry establishing that the *other* two changes on this path were
noise; declining the one that is not would be odd.

**Also hoist `check_foreign_keys`' reads.** Tempting — it is in the same loop —
and it is not the same kind of thing: those are storage reads whose results
genuinely differ per row, and `visible_parents` already batches them. Nothing to
hoist.

## Evidence

`update_many` over 5,000 rows, non-superuser, 65 grants, release build, seven
runs per sample, microseconds. Two samples of the new shape and three of the
old, because one of each would not show whether the gap is stable:

| shape | median | runs (sorted) |
| --- | --- | --- |
| filters per row | 5949 | 5784 5802 5930 **5949** 5968 5996 7156 |
| filters per row | 5954 | 5678 5906 5912 **5954** 5965 5979 6871 |
| filters per row | 5932 | 5774 5797 5812 **5932** 5972 6834 6887 |
| **hoisted** | **5555** | 5435 5440 5455 **5555** 5567 5650 6716 |
| **hoisted** | **5441** | 5312 5367 5437 **5441** 5452 5582 6777 |

Medians cluster at 5932–5954 before and 5441–5555 after: **~400–500µs on 5,000
rows, about 7%.** The fast clusters do not overlap (hoisted tops out at 5650
below the run-of-the-mill 5678), and every sample carries one high outlier
around 6.8–7.2ms that both shapes share and that is this machine rather than
this code. Contrast with the grant hoist in the previous entry, whose medians
differed by less than the outlier gap: that was noise and this is not.

**Three mutations, and the third is the finding:**

| mutation | result |
| --- | --- |
| `check_row_against` loses the soft-delete guard | `a_write_that_supplies_the_soft_delete_column_names_that_column` FAILED |
| the existing-row filter ignores `retired` | `without_read_deleted_the_bulk_writes_stay_out_of_reach_too` FAILED |
| the loop's two filters are swapped | **survived — 55 suites, 0 failures** |

The third is not a missing test. Every row that reaches the loop's *insert*
branch has no `previous`, which happens only under `Insert` or `Upsert` — and
both satisfy `may_insert()`, so the pre-check above has already run the
identical filter over every row in the batch. **The branch is redundant**, and
`write_many` now says so, in the manner `purge_deleted`'s deliberately redundant
`IS NOT NULL` conjunct is already documented in this file.

It stays, because the redundancy is one edit away from not being one: a mode
that writes new rows without `may_insert()`, or a pre-check narrowed for speed,
would make it the only `WITH CHECK` a fresh row ever gets, and the failure would
be a policy silently not applied. **What is load-bearing is the pre-check, and
that is now demonstrated** — deleting it fails four tests:
`insert_many_discloses_another_tenants_primary_keys`,
`insert_many_discloses_another_tenants_unique_values`,
`upsert_many_discloses_another_tenants_primary_keys`, and the new one below.

**A test for a decision nobody had pinned.** Chasing that mutation found that
`write_many`'s comment claims something no test asserted: an upsert is judged by
the *insert* policy even for a row that exists. My first attempt at a
discriminating test assumed the opposite and failed on its own premise.
`an_upsert_must_satisfy_the_insert_policy_even_for_a_row_that_exists` now pins
it, with `update_many` beside it taking the update policy alone — which is what
makes the difference a decision rather than an accident of where a check sits.

**Suites**: `slate-kernel` (48 in `soft_delete`), `slate-server`, `slate-orm`,
`slate-schema` all pass. `cargo clippy --workspace --all-targets` clean,
`scripts/check.sh` 20/20.

## What this does not do

**The single-row paths still build a filter per call**, which is correct — one
row, one filter — and means `insert`, `update` and `upsert` gain nothing here.
A caller writing a thousand rows one at a time still pays a thousand builds, and
the fix for that is the transaction-scoped cache rejected above.

**Not measured anywhere but this machine, this shape, this batch size.** 5,000
rows, 65 grants, one policy-free table, in-process, `MemoryStore`. A deployed
head node puts RPC and storage in front of this; the proportion there is
unmeasured and is certainly smaller.

**`update_where` and `delete_where` were not touched.** They evaluate their
predicate through the planner rather than row by row through `permits_row_with`,
so they do not have this shape — an inference from the code path, not a
measurement.

**The redundant branch is still redundant.** This entry documents it rather than
removing it, and that is a judgement: removing it would be a smaller file and a
worse one the first time somebody adds a bulk mode.
