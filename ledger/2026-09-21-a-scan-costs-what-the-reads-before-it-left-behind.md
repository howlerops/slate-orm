# Neither example was wrong about the scan: a partially warm cache makes one seven times dearer.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #270 (F5p)
- **Touches:** `crates/slate-slatedb/examples/cost_at_scale.rs`, `docs/performance.md`
- **Kind:** measurement

## What changed

`cost_at_scale.rs` gains a full scan on a **pristine** store — opened, read
from, closed, before anything else touches the data — and repeats its existing
scan three times. Between them they explain the 8× disagreement #269 recorded
as unresolved, and the explanation is not that either file is wrong.

`docs/performance.md`'s "the calibration was not measuring a warm cache"
section is superseded in place, and the two conclusions of its constants
section are struck through.

## Why

#269 changed `POINT_READ_COST` and deliberately left `SCAN_ROW_COST` alone,
because `cost_calibration --cold` measured a full scan of 200,000 rows at 58
requests and `cost_at_scale` measured the same scan on a byte-identical
fixture at 205, and `cost_at_scale` reported a *warm* scan costing more than a
cold one. Half a recalibration is a real risk, and shipping one against a
measurement that contradicts itself would be the error this repository has
already made twice in this area.

The impossible ordering was the thread worth pulling. A cache cannot make a
second scan dearer. So either the count includes something that is not the
scan — background compaction was the obvious candidate — or "cold" and "warm"
are not the variable.

## Alternatives rejected

**Assume compaction and stop.** It was the first hypothesis and the plausible
one, and it is wrong: three consecutive identical scans give 204, 208, 205, and
background traffic does not arrive in a flat line. Testing it cost one run and
it is the reason the next hypothesis was looked for at all.

**Change `SCAN_ROW_COST` to one of the three measured values.** Now that the
three are explained they are still three: 3,774 rows per request, 980, and 542,
spanning 7×. Picking one is a decision about which cache state the planner
should assume, not a calibration, and it would flip plans far more widely than
`POINT_READ_COST` did — that change moved 13 costs and no access path, and this
one would move access paths. It also wants the oracle suites, which do not fit
on this disk in one run.

**Make the examples agree by changing one of them.** They do not disagree.
Making one measure the other's state would delete the finding and leave a
number that describes one situation pretending to describe all of them.

**Leave `docs/performance.md`'s section standing with a note.** It says "the
cache is not what the calibration measured", which is now known to be false in
a specific and interesting way. `CLAUDE.md`: stale documentation is worse than
none, because it is read as current.

## Evidence

The same full scan of the same 200,000 rows, same process, same server:

| the store has already | GETs | rows/GET |
| --- | ---: | ---: |
| read nothing at all | 53 | 3,774 |
| served 200 random point reads | 204, 208, 205 | 980 |
| served `analyze` and 400 point reads | 369 | 542 |

**Three consecutive scans give the middle row**, so this is not "the first one
paid for the cache and the rest are free" — which is what a cache normally
does and is what made the numbers look broken. A *partially* populated block
cache fragments a scan into many small ranged reads instead of a few large
ones, and more of it fragments it further. That is why a scan after `analyze`
costs more than one before: `analyze` reads the whole table and leaves it
scattered through the cache.

**Both files were right.** `cost_calibration --cold` opens a fresh store per
case and scans one that has read nothing: 58. `cost_at_scale` scans one that
has just walked 200 random keys: 205. The pristine arm added here reads 53 on
`cost_at_scale`'s own fixture, which is the other file's number on this file's
data, and closes the question.

**`SCAN_ROW_COST` says 8,000 rows per request and that is none of the three.**
It is what a scan costs when the cache already holds the entire table, which is
the state `cost_calibration`'s default run measures in and now says so.

**A defect in the probe, found by running it.** The pristine arm opened a
second writer on a path another store already held, and SlateDB fenced the
first — every later measurement died with `WriterFenced`. The probe now closes
its store rather than dropping it. The storage layer was right and the
benchmark was careless, which is the better way round.

**The planner's choice on this fixture is now wrong and the file says so.**
`chose TableScan but Index is faster — WRONG at this scale`: at 200,000 rows
the index does 368 requests against the scan's 370 and finishes 11× sooner,
while the model calls it 15× worse. That is not acted on here — see above — but
it is no longer a hypothetical.

## What this does not do

**It does not fix the cost model.** The model has one number for a scanned row
and the measurement has three, differing by how scattered the reads before it
were. Nothing in `TableStats` represents that, and inventing a statistic for it
is a planner change with its own design question: what a *server* should assume
about its own cache, which depends on the workload rather than on the data.

**It does not establish which state a deployment is in.** All three are
reachable and the middle one is probably commonest — a table that has served
some point reads and is now scanned — but "probably" is the whole content of
that claim. Nothing here measures a real workload.

**One fixture, one row width, one scale.** 200,000 rows of a four-column table
with one index, on a loopback S3 server in a debug build. The GET counts do not
move with the optimisation level, which is why a debug build is honest here;
they do move with row width and block size, neither of which was varied.

**The mechanism is inferred, not observed.** "A partially populated cache
fragments the scan's reads" fits every number above and is not measured: that
would want the *sizes* of the ranged GETs, which the counting store does not
record. It counts requests. A different mechanism with the same request counts
is not excluded.

**Nothing runs this example either.** It is the seventh in `slate-slatedb`, all
of them compiled by `clippy --all-targets` and run by nobody, which is the
state `slate-headbench` was in before #265 and is how a benchmark's numbers
become historical without anyone deciding.
