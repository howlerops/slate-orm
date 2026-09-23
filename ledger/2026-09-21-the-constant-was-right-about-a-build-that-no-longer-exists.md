# `POINT_READ_COST` was 3.0 because SlateDB's block cache was compiled out — reproduced to the unit — so neither measurement was ever wrong, including the one I withdrew this morning.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #278 (F5x)
- **Touches:** `crates/slate-kernel/src/{stats.rs,join.rs}`, `crates/slate-kernel/tests/chain.rs`, `crates/slate-slatedb/examples/{cost_at_scale,cost_calibration}.rs`, `docs/correctness.md`, `scripts/{check_cost_prose.py,test_check_cost_prose.py}`
- **Kind:** measurement, and a withdrawal of my own

## What changed

`stats.rs` no longer says what changed "is not known". It knows, and carries the
A/B.

`cost_at_scale` prints the two constants **from** `slate_kernel::stats` rather
than restating them beside the table; its header had said `POINT_READ_COST =
3.0` since #269 made that false. `check_cost_prose.py` reads every crate rather
than only `slate-kernel`, which is what would have caught it.

Two passages I edited a few hours ago in #277 are corrected: I wrote that the
1,221-request figure "does not reproduce", and it does.

## Why

#269 re-measured `POINT_READ_COST` from 3.0 to 1.0 and wrote, honestly, that it
could not explain the old figure: *"What changed since the recorded runs is not
known — a SlateDB release, a block size, the readahead #34 turned on."* Three
guesses, none checked. #277 left that standing.

`docs/performance.md` holds a fourth candidate, in a section about something
else. **Finding 8**: every crate declared `slatedb = { default-features =
false }` and `slate-slatedb` re-enabled only `aws`, so SlateDB's block and
metadata caches were compiled out — a `SplitCache` with two empty halves that
answers `Ok(None)` to every lookup and discards every insert. On the probe
there, a point read went from **3.02 GETs to 0.00** when the `cache` feature
was turned on.

3.02. That is a testable explanation rather than a fourth guess, because the
feature is still a flag.

## Alternatives rejected

**Leave "not known" and move on.** It had survived two tasks already, and a
cost constant whose provenance is a mystery is one nobody can re-derive when
the storage layer moves again. The experiment is one build and one run.

**Argue it from the 3.02 in `performance.md`.** The number matches and the
mechanism is plausible, which is exactly the point at which this repository
asks for a demonstration rather than a hypothesis. Arguing it would also have
missed the part that makes it conclusive: it reproduces **1,221**, not
"about three".

**Rewrite the old comments as simply wrong.** They are not. Every figure in
`join.rs`, `chain.rs` and the struck-through text in `stats.rs` is a correct
measurement of the build it was taken on. Calling them wrong would have thrown
away the more useful fact — that a feature flag moved a planner constant by 3×
— and would have been the second error on top of my first.

**Widen the guard to `docs/` too.** `correctness.md` narrates the history of
these numbers at length, and a guard that cannot tell "it costs three" from "it
cost three until #269" either fires on every paragraph or needs an exemption
per paragraph. Stated in the guard's docstring; `site/check/docs.py` is that
tree's guard.

## Evidence

The same `cost_calibration`, same machine, same 200,000 rows, two builds, one
session apart:

| build | forced index, 400 rows | per row | 400 rows via index (probe block) |
|---|---:|---:|---:|
| `--no-default-features --features aws` | **1,221 GETs** | **3.05** | 1,223 GETs, 2.80 s |
| default, with `cache` | **414 GETs** | **1.035** | 40 GETs, 0.10 s |

**1,221 is the recorded figure to the unit.** `join.rs` and `chain.rs` say
"four hundred point reads cost 1,221 requests"; the struck-through line in
`stats.rs` says 1,217. The run that produced the old constant is reproducible
today by one flag.

Controls, from the same two runs: the full scan reads **29 GETs on both arms**,
and a point get by primary key reads **19 on both**. So the cache moved the
index-following path and nothing else, which is what a *block* cache should do
to a walk that re-reads blocks and not to one sequential pass. `SCAN_ROW_COST`
is untouched by this and stays where #277 measured its headroom.

`sh scripts/check.sh`: **42 passed, all of them.**
`cargo clippy -p slate-kernel -p slate-slatedb --all-targets`: clean.
`python3 scripts/test_check_cost_prose.py`: **17 passed, 0 failed.**
`python3 site/check/docs.py`: the docs site holds together.

**A mutation that proves the widening.** Putting `cost_at_scale`'s header back
to the hand-written `POINT_READ_COST = 3.0` is caught by *every claim in the
real crates is current* — and would not have been an hour ago, when the guard
read `crates/slate-kernel` only. The defect and the blind spot were the same
shape: a guard scoped to where the constant is *decided* rather than to where
it is *quoted*.

> **Corrected by #279, the next day.** That mutation is caught, but it is
> caught by the *English clause* in the old header — "a point read costs ~3
> requests" — and not by the literal `POINT_READ_COST = 3.0` beside it, which
> this entry implies. The distinction was not academic: **the same file still
> held two stale claims** after the widening described here, in two forms the
> guard could not see, and neither was found by this mutation. The sentence
> below about `cost_at_scale` printing its constants was true of the one line
> I changed and wrong about the file. #279 has the two survivors, a ninth
> stale claim in `latency.rs` that fell out of fixing them, and the lesson:
> widening *which tree* a guard reads is not the same as widening *what it
> recognizes as a claim*.

### A withdrawal of my own, which is the part worth reading

In #277, a few hours before this, I struck through the 1,221 figure in
`join.rs` and wrote **"The request figure does not reproduce"**, and in
`chain.rs` "that figure does not reproduce". I had four measurements at ~1.0
and one recorded figure at ~3.0, and I concluded the odd one out was an
artefact.

It reproduces. Exactly. The four measurements and the one were taken on
different builds, and nothing in either comment said which — so the comparison
I made was between numbers that were never comparable.

`CLAUDE.md` asks that a hypothesis the evidence contradicts be withdrawn and
that the withdrawal be said out loud, and that is what the two passages now do:
the strikethrough is on *my* sentence, not on theirs. The lesson is narrower
than "be careful": **a measurement in this repository does not record which
feature set produced it**, and four agreeing runs are not evidence against a
fifth if they all share a flag the fifth did not have.

## What this does not do

**It does not re-derive the constant.** `POINT_READ_COST` stays 1.0, which is
right for the build that ships. Nothing here re-opens #277's question about
`SCAN_ROW_COST`.

**It does not measure the cacheless build at any other shape or scale.** One
fixture, 200,000 rows, one run per arm. The arms differ by 3× on the index path
and agree on the other two, which is a large enough gap that a second run was
not taken — and that is a judgement, not a spread.

**Nothing records which features a measurement was taken under.** That is the
general form of the defect, and it is not fixed: every table in
`docs/performance.md` and every figure in a doc comment is still a number
without a build beside it. A convention would help and one was not introduced
here.

**The guard still reads Rust only.** A stale figure in a Python harness, a Go
client comment or a TypeScript doc block is not caught. Nothing there quotes
these constants today; that is an observation about today.

**`cost_at_scale`'s header is now derived, and the other benchmarks' are not.**
`cost_calibration`'s module doc quotes 1.02 and 8,000 in prose, which the guard
checks, but several examples print measured tables whose *labels* are still
hand-written — the class of defect #276 found in `perf_report`.
