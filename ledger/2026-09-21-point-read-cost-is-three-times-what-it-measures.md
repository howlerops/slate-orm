# `POINT_READ_COST` was 3.0 on a figure nothing reproduces; four measurements say 1.0.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #269 (F5o)
- **Touches:** `crates/slate-kernel/src/stats.rs`, `crates/slate-kernel/tests/snapshots/plans.txt`, `crates/slate-slatedb/examples/cost_calibration.rs`, `docs/{correctness.md,full-text.md,orm-comparison.md}`
- **Kind:** fix

## What changed

`POINT_READ_COST` is **1.0**, from 3.0. The crossover at which a non-covering
index beats a table scan moves with it, from one row in 24,000 to one in 8,000.

`cost_calibration.rs` gains `--cold`, which reopens the store before every
measurement. `SCAN_ROW_COST` is unchanged, deliberately.

## Why

The constant's recorded provenance is "400 rows reached by index cost 1,217
requests" — 3.04 each — and a later re-measurement at 3.40–3.57 that left it
at 3.0 as within the spread. Neither reproduces. Four measurements today, three
benchmarks, two fixtures, both cache states:

| measurement | requests per row |
| --- | ---: |
| `cost_calibration`, forced index, 400 of 200,000, warm | 1.02 |
| `cost_calibration`, the same, cold | 1.09 |
| `cost_at_scale`, 200 probes on a pseudo-random walk, cold / warm | 1.16 / 0.96 |
| `ascending_walk`, ordinary and inverted index, stride 500 | 1.015 |

Nothing produces 3, and the spread across all four is tighter than the gap to
it. What changed since the recorded runs is not known and this entry does not
guess: a SlateDB release, a block size, the readahead #34 turned on.

A constant four measurements contradict is not a conservative choice. A cost
model wrong by 3× in the direction of "never use an index" is the same class
of error the recalibration that produced 3.0 was written to fix, pointing the
other way, and `cost_at_scale`'s verdict line already reads *chose TableScan
but Index is faster — WRONG at this scale*.

**1.0, not 1.02**, because a bound with a meaning survives a fixture change
where a fitted decimal does not: one object-store request for a row that shares
its block with no neighbour. No measurement here exceeds it.

## Alternatives rejected

**Leave it at 3.0 and only write the finding down.** The tempting option,
because the previous two calibrations were careful and disagreeing with both is
uncomfortable. Rejected because the discomfort is not evidence: the earlier
figures are two runs of one method and this is four runs of three, and every
day the constant stays wrong is a day the planner declines an index it should
take. The change is also cheap to undo — one line and a snapshot.

**Fit the constant to 1.02.** Two significant figures from one machine on one
afternoon, and it would read as more precision than exists. It also sits in the
wrong place conceptually: the measured value ranges from 1.015 down to 0.043
with how clustered the matches are, so any single number is a bound, and the
bound worth choosing is the one that means something.

**Move `SCAN_ROW_COST` at the same time.** The two examples disagree about what
a cold full scan of the *same* fixture costs — 58 requests against 205 — and
`cost_at_scale` reports a warm scan costing *more* than a cold one, which
cannot be true. Calibrating one constant against a measurement the other
contradicts is how the errors this replaces were made. #270.

**Make `--cold` the default.** `docs/performance.md` quotes the warm numbers.
A file that silently starts answering differently is worse than one that
answers two ways on request.

**Make the cost a function of clustering now.** It is what the measurements
actually say — 1.015 at stride 500, 0.043 contiguous — and it is a planner
change, needing a statistic nothing collects. Named, not built.

## Evidence

**The kernel suite: 689 passed, 0 failed, before and after.** The one test that
failed on the change was `plans_match_the_committed_snapshot`, and its diff is
the finding's blast radius stated exactly: **13 plans changed cost, and not one
changed access path.** Every line is the same operator with a lower number —
`Point Get … cost=3.00` to `cost=1.00`, `Index Scan using by_kind … 31.00` to
`11.00`. Regenerated and re-run green. `slate-orm` 122, `slate-sql` 7,
`slate-server` 352, `slate-wasm` and `slate-slatedb` libs 6; workspace clippy
`--all-targets` clean; `sh scripts/check.sh` 34.

That no path flipped is also the honest limit: **the suite cannot see whether
this change is right.** The crossover it moves lives at 200,000 rows and up,
and no fixture in the kernel's tests is near it.

**Two mistakes of mine, both caught by measuring rather than reasoning.**

*The first:* #266's entry claimed `cost_calibration` measured a warm cache
where `ascending_walk` measured cold, citing 27 GETs against 406. Those are not
the same query — that file's "index equality" row does not use the index,
because the planner picks a scan for it, which is the whole subject of
`full-text.md` §6. Withdrawn in that entry, struck through.

*The second, in this task's own code:* the first `--cold` reopened the store
and then called `analyze`, which is a full table scan. Every "cold" case had
the whole table read into memory on its behalf, and the run duly reported that
the cache changed nothing. Statistics are now gathered once and handed to each
fresh store, and the corrected table is not a null result at all:

| 200,000 rows | warm | cold |
| --- | ---: | ---: |
| point get, one row | 2 | 18 |
| narrow key range, 1,000 rows | 1 | 13 |
| full scan | 25 | 58 |
| forced index, 400 rows | 410 | 435 |

A cold store pays for its manifest and index blocks before it reads a row,
which is why one row costs 18 requests and a thousand cost 13. **The marginal
figure the model is denominated in is what survives both modes**: 1.02 warm,
1.09 cold.

The one place the cache *reverses* a verdict rather than scaling it is the
"probe, or scan?" block, which runs immediately after the forced-index case has
pulled the index into memory: warm it reads 40 GETs for the probe against 424
for the scan, cold 439 against 65. That block's warm numbers mean nothing and
its header now says so.

## What this does not do

**It does not explain why the old figures were what they were.** Three point
oh, then three point five, now one — all measured, none reconciled. If the
cause is a SlateDB version then the constant will move again on the next
upgrade and nothing here notices.

**Nothing re-runs these benchmarks.** `slate-slatedb`'s seven examples are in
the state `slate-headbench`'s five were before #265: compiled by `clippy
--all-targets`, run by nobody. A constant calibrated by a benchmark nothing
runs is a constant that goes stale exactly the way this one did. #270.

**`SCAN_ROW_COST` is untouched and at least one measurement of it is wrong.**
The ratio of the two constants is what decides plans, so half a recalibration
is a real risk and this entry takes it knowingly: the point-read side is
contradicted by four measurements and the scan side by one that contradicts
itself.

**No end-to-end confirmation that the new constant chooses better.** The
argument is that the model should match the measurement, and it now does for
one shape at one scale. Whether ClickBench, the deployed harness or the demo
get faster is unmeasured; ClickBench alone is 70 s a run and needs the
`slate-clickbench` fixture this container has been reclaiming disk by deleting.

**The 18-requests-cold figure is not investigated.** A fresh store spending 18
requests before returning one row may be reasonable or may be a defect in how
the manifest is read; it is reported because it is what the counter said, and
it is the reason the cold column cannot be read as a per-row cost.
