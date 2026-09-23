# The 3.5x disagreement was not open — it had been explained and `correctness.md` had not been updated to say so. With that out of the way, 400,000 rows fit, and gave this model its first second data point.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #289 (F6i)
- **Touches:** `docs/{performance.md,correctness.md}`, `README.md`, `scripts/reclaim.py`, the #284 entry
- **Kind:** a stale argument withdrawn, and a measurement that contradicts the model's shape

## What changed

**A stale reason, corrected.** `correctness.md` said `SCAN_ROW_COST` was not
moved because the two examples *contradict* each other — 58 requests against
205 for the same cold scan. `performance.md` had already established that both
are right: they scan stores in different cache states, and a partially
populated cache fragments a scan. The conclusion was right, the argument had
gone stale, and **my own #284 entry inherited it by citation** — corrected
there too.

**A second scale.** Every cost number on that page was taken at 200,000 rows.

| the store has already | 200,000 | 400,000 | requests, for 2× the rows |
|---|---:|---:|---:|
| read nothing at all | 51 GETs | 69 – 77 | 1.35 – 1.51× |
| served 200 random point reads | 199 – 201 | 228 | **1.14×** |
| served `analyze` and 400 of them | 365 | 406 | **1.11×** |

<!-- not a measurement -->

**A scan gets cheaper per row as the table grows** — 1,005 → 1,754 rows per GET
in the middle state, 548 → 985 in the third. The cost model charges scans
strictly linearly in rows, so this is a direction it cannot express at all.

## Why

The task took #284's word that the disagreement was open. It was not, and
finding that out first is what made the rest of the task worth doing: the
3.5× gap needed no new measurement, so the run could go at the question
nobody had answered instead — whether anything about this model holds at a
scale other than 200,000 rows.

## Alternatives rejected

**Move `SCAN_ROW_COST` to fit the new numbers.** More tempting now than before,
because there are six measurements instead of two. Still no: a constant that
fits 200,000 and 400,000 in three cache states is fitting a *surface* with a
scalar. The docstring's argument — no single value, because the model has no
input for cache state — now has a second axis it has no input for either.
Fitting the six would hide that behind a number that looks better calibrated.

**Reach for 1,000,000.** The README's open item, and the loader cliff between
500,000 and 600,000 still blocks it; four attempts past it have never finished.
400,000 is below the cliff and fits the disk, which is why it is the number
here. This does not narrow the cliff.

**Take the one 400,000 run.** `CLAUDE.md` asks for spread and it was worth the
second run: the pristine scan moved 77 → 69, the only figure that moved at
all, and knowing it is the unstable one is what lets the other two rows be
quoted as single numbers.

**Report the disagreement as resolved by my measurement.** It was resolved
before I started, in a section I had read. Recording it as a fresh finding
would have been taking credit for reading.

## Evidence

Three runs at `--release` with the cache on, all carrying a build line:
`SCALE_ROWS=200000` once, `SCALE_ROWS=400000` twice.

**The 200,000 run reproduces the recorded three-state table** — 53 / 205 / 369
as recorded, 51 / 199 / 365 now — which is the first time those numbers have
been confirmed on a build that can name itself.

**Stability:** `full scan, three times` printed `[228, 228, 228]` in *both*
400,000 runs, and the two probe-state rows are identical across runs. Only the
pristine scan moves.

**Point reads hold across the scale change:** 1.14, 1.19, 1.16 requests per
read against `POINT_READ_COST`'s 1.0 — independent of #284's 1.02, from a
different example, a different fixture size and a colder store.

**A trap, met and now written down.** `reclaim.py` removes
`target/*/examples`, which includes the `--release` example binary you are
about to measure with. Reclaiming to make room for the 400,000-row fixture
deleted `cost_at_scale`, and the next run exited 127 with no output. The
docstring says so now.

`sh scripts/check.sh`: **47 passed, all of them.** 75 measurement tables, 2
stamped.

## What this does not do

**It does not reach a million rows**, and the cliff that stops it is unchanged
and still unattributed.

**Two points are a line through two points.** 200,000 and 400,000 say the
curve is sub-linear over that interval and nothing about its shape. A third
point could easily show it flattening, or reversing.

**It does not change any constant or any plan.** The sub-linearity is recorded
as something the model cannot express, not as a correction to it, and no
planner behaviour changes.

**The fixture is one shape.** Same row width, same key distribution, same
in-process `s3s` server as every other number here. Whether a scan of a
different row width gets cheaper per row the same way is untested.

**It is still a loopback measurement**, so only the request counts transfer.
