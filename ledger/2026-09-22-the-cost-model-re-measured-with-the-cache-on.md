# Finding 8's `3.0` came from a build with the block cache compiled out; nothing had re-run the calibration with it **on**, at `--release`, and recorded the result beside the build that produced it.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #284 (F6d)
- **Touches:** `docs/performance.md`
- **Kind:** a measurement, and the first recorded table in this repository that says what built it

## What changed

A new §8b in `docs/performance.md`: the cost calibration re-run at
`--release`, on a build with `cache` on, three times, with the build line
above the table.

**Both constants hold.**

| what | measured | the constant |
|---|---|---|
| a point read | **408 GETs for 400 rows**, identical in all three runs | `POINT_READ_COST = 1.0` |
| a full scan | 9,524 – 10,000 rows per GET | `SCAN_ROW_COST` → 8,000 |

<!-- not a measurement -->

It is also the first table on that page the provenance guard from #282 counts
as **stamped** rather than frozen: 71 measurement tables, 70 predating the
rule.

## Why

`POINT_READ_COST` was 3.0 for nine tasks. #269 re-measured it to 1.0 without
being able to explain the 3.0; #278 found the explanation — SlateDB's block
cache compiled out — and fixed the manifest. What nobody had done was run the
calibration *after* that fix, at the profile the page's numbers are taken at,
and write down the build alongside the answer. The constant was right for a
reason that had never been confirmed by a run.

It has now been confirmed by one, and the run can say what it was.

## Alternatives rejected

**Re-measure `SCAN_ROW_COST` and move it.** The full scan returns 9,524–10,000
rows per request and the constant says 8,000, so there is a 20–25% argument for
raising it. Rejected for the reason already written into `correctness.md`:
`cost_at_scale` and `cost_calibration` disagree about what a cold full scan of
this fixture costs — 58 requests against 205 — and calibrating against a
measurement that another example contradicts is precisely how the errors this
whole sequence has been unwinding were made. 10,000 is inside the 8,000–10,526
band already recorded. A constant does not move on one warm-cache run.

**Report the point-get row as the cost of a point read.** It says 22, and it is
wrong. It is the first query against a freshly loaded store and pays for the
manifest and SST metadata that every row below it then reuses. Taking it at
face value would have re-created finding 8 in the opposite direction, from the
same table, one section below the write-up explaining it.

**Run once.** `CLAUDE.md` asks for spread rather than a single number, and it
paid: the point-get row moves 19 → 25 → 19 across runs while every other GET
count is identical or within one. That contrast is what identifies it as
warm-up rather than as a shape the model gets wrong.

**Re-measure at scale as well** (`cost_at_scale`, 600k+). It is the run that
would settle the `SCAN_ROW_COST` question properly, and it does not fit on this
disk today — the release build of one example took the free space down to
2.5 GB and `cost_at_scale`'s fixtures are far larger. Recorded below as
not done rather than attempted and abandoned quietly.

## Evidence

Three runs of `cargo run --release -p slate-slatedb --example cost_calibration`,
200,000 rows over the in-process `s3s` server.

**Identical in all three runs:** forced index 408 GETs / 400 rows; 400 rows via
index 40 GETs; the same rows via full scan 418 GETs; narrow key range 1 GET;
wide key range 10 GETs.

**Moved between runs:** full scan 20 / 21 / 21 GETs. Point get 22 / 25 / 19 —
the only figure with real variance, and the only one that is a cold first
query.

**ms per GET:** 10.665, 10.930, 10.860, against a loopback server. The page's
2.2 ms is a wide-area figure; these are not a correction to it and are not
comparable with it. The GET counts are what transfers, which is why the table
records those and the prose says so.

The planner's decision at this fixture is consistent with the recorded
crossover: `scanning 200,000 rows beats 400 point reads: false`, and
`n > 8000k` puts the crossover at 3,200,000 rows for k = 400.

`sh scripts/check.sh`: **47 passed, all of them.** The provenance guard
accepts the new table as stamped; the cost-prose guard required
`<!-- not a cost-model claim -->` on the paragraph stating the *measured*
rows-per-request, which is the third category #283 named and the first time it
has been needed on new writing.

## What this does not do

**It does not settle `SCAN_ROW_COST`.** One warm-cache run at 200,000 rows
agrees with the recorded band and contradicts nothing. The two examples that
disagree 3.5× about a cold full scan still disagree.

**It measures a warm store only.** The example holds one store throughout, as
the page has always recorded. Every GET count here is a warm-cache count —
#269's finding, unchanged, and the reason the point-get row is what it is.

**It is a loopback measurement.** No network, no real S3, no latency
distribution. Only the request counts are claimed to transfer.

**One table on that page is stamped and seventy are not.** The other seventy
cannot be; their runs are gone. This is the first of however many get
re-measured.

**The re-measurement was not repeated at scale**, per *Alternatives rejected*.
Until it is, the 200,000-row result is the only one taken on a build that can
name itself.
