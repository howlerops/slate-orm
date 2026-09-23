# Moving `SCAN_ROW_COST` flips a named plan decision at 1.3× — measured — and looking for that found eight files still quoting a constant that changed two tasks ago.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #277 (F5w)
- **Touches:** `crates/slate-kernel/src/{stats.rs,join.rs}`, `crates/slate-kernel/tests/{chain,fulltext,histogram,planner,point_gets,security}.rs`, `scripts/{check_cost_prose.py,test_check_cost_prose.py,check.sh,test_check_sh.py}`, `.github/workflows/ci.yml`
- **Kind:** measurement, docs, and a guard

## What changed

**No constant.** `SCAN_ROW_COST` is still `0.000_125` and every plan is the
plan it was.

Its doc comment now carries the blast radius of changing it — which tests fail
at each candidate value, and the bisected point at which the first one flips.

Eight files that said a point read costs about three say one, and the five that
derived a crossover from it say eight thousand rather than twenty-four.
`scripts/check_cost_prose.py` keeps them that way.

## Why

#270 explained why two benchmarks disagreed about a cold scan and declined to
recalibrate, giving a reason that was honest and unquantified: changing this
"would flip plans far more widely than `POINT_READ_COST` did — that change
moved 13 costs and no access path, and this one would move access paths". How
widely was not known. It is a measurable thing and now it is measured.

The result is sharper than expected and argues for the status quo more strongly
than the guess did. **The current value is about 1.3× from flipping a decision
this crate has a named test for, while the spread of its own calibration is
7×.** Every candidate recalibration — including the mildest, a completely cold
store — is past that edge.

The stale prose was not the task and is the larger defect. Looking for what
justified the current value meant reading the comments around it, and eight of
them assert a figure `POINT_READ_COST` has not had since #269. Five go on to
derive a crossover from it, so a reader deciding whether an index is worth
adding would have got the answer three times wrong, from a comment written in
the voice of a measurement. `stats.rs` itself had the same defect and was
corrected in #275; this is the same defect in the eight files around it.

## Alternatives rejected

**Recalibrate to one of the three measured values.** The measurement below is
the argument against, and it is the argument #270 wanted: the mildest candidate
breaks `a_large_set_goes_back_to_a_scan` and
`the_planner_picks_a_loop_only_for_a_small_outer_side`, which are decisions
somebody wrote a test for because they are the ones that matter. Choosing one
cache state's number and shipping it would trade a known wrongness for an
unknown one.

**Recalibrate to the post-`analyze` value, which makes `cost_at_scale`'s
verdict come out right.** The most tempting, because that benchmark's fixture
has run `analyze` and 400 probes and so measures 542 rows per request — the
model would then call the index a near-tie rather than 15× worse, matching
what happens. It is tuning one number until one benchmark agrees. It is also
the most disruptive of the three, at six failing tests.

**Add a cache-state statistic and cost the scan against it.** The real fix and
out of scope here: nothing in `TableStats` represents it, gathering it is a
storage-layer question, and what a server should *assume* about its own cache
depends on its workload rather than its data. Named in #270 and still open.

**Fix the eight comments and not write the guard.** This is the second time —
`stats.rs` in #275, eight files here — and the numbers are derivable from the
constants, so it is checkable rather than rememberable. A third occurrence was
the expected outcome of stopping at the fix.

**Check the prose by rostering the sentences.** A roster of approved sentences
would go stale the first time somebody rewords one. The guard parses the figure
out of the sentence and compares it to arithmetic on the constants, the way
`check_demo_surface.py` recomputes the README's book counts rather than
storing them.

## Evidence

`sh scripts/check.sh`: **42 passed, all of them** (40 before; this adds two).
`cargo clippy --workspace --all-targets`: clean.
`cargo test -p slate-kernel --no-fail-fast`: **689 passed, 0 failed**, before
and after the comment edits.
`python3 scripts/test_check_cost_prose.py`: **17 passed, 0 failed.**
`python3 scripts/check_cost_prose.py`: *ok    8 prose claims about the cost
constants, all current*.

**The blast radius.** Each value set in `stats.rs`, the kernel's whole suite
run, the file restored in a `finally`:

| rows per request | `SCAN_ROW_COST` | failing | the ones it adds |
|---:|---|---:|---|
| 8,000 — shipped | `0.000_125` | 0 | |
| 3,774 — a store that has read nothing | `0.000_265` | 4 | `a_large_set_goes_back_to_a_scan`, `the_planner_picks_a_loop_only_for_a_small_outer_side`, `plans_match_the_committed_snapshot`, `the_fixture_and_the_cost_model_agree_on_rows_per_block` |
| 980 — after 200 random point reads | `0.001_02` | 5 | `a_wide_in_loses_to_a_table_scan` |
| 542 — after `analyze` and 400 reads | `0.001_845` | 6 | `the_planner_picks_a_loop_when_the_accumulated_side_is_small` |

The failures nest exactly, which is what a monotone constant should do and is
the reason to believe the runs rather than the arithmetic.

Two of the four at the mildest value are mechanical: the committed plan
snapshot, and `the_fixture_and_the_cost_model_agree_on_rows_per_block`, which
exists to say the latency fixture holds the same rows-per-block figure and to
demand both move together. The other two are access-path choices.

**The headroom, bisected.** `a_large_set_goes_back_to_a_scan` run alone at five
values:

| `SCAN_ROW_COST` | verdict |
|---|---|
| `0.000_125` (shipped) | passes |
| `0.000_150` | passes |
| `0.000_180` | `got Point Gets (400 keys) on notes (rows=400 cost=400.00)` |
| `0.000_200` | the same |
| `0.000_265` | the same |

So the flip is between 1.5e-4 and 1.8e-4 — **1.2× to 1.4× above the shipped
value.** A four-hundred-key `IN` over a five-million-row table becomes four
hundred point reads at that point.

**What was not established.** Why each test flips, in the model's own terms. My
arithmetic from the constants does not reproduce the observed crossover, so it
is wrong somewhere and the bisection is reported instead — a measured boundary
rather than a derived one. `touched`, the residual selectivity and the RLS
term all enter, and chasing which was not done.

**Mutations via `scripts/mutate.py`, twelve.** Nine over the guard, two over
the real constants, one over a real module doc; all three runs exit 0. The
constant mutations are the ones that matter for staleness: `POINT_READ_COST`
put back to 3.0, and `SCAN_ROW_COST` doubled, each caught by *every claim in
the real kernel is current*.

Three survived the first run, and all three are shapes this week has now met
before:

- **A thousands figure in digits** (`8 thousand`) was read as 8. No case used
  that spelling; one does now.
- **The never-fires guard lives in `main`**, which read the module constants,
  so no fixture could reach it. `main` takes both paths now — the third guard
  in three tasks with this exact hole.
- **Deleting the missing-constant check crashed the suite** rather than failing
  a case, which `mutate.py` reports as `NOTHING RAN`. Both call sites catch it
  now.

**A defect in the guard, found by running it.** Its first pass reported
`planner.rs` as stale because *"twenty point reads cost about twenty
object-store requests"* matched the per-read pattern. That is a total, not a
unit cost. The sentence now says "one apiece" and means what it says — the
guard was right that the sentence was ambiguous and wrong about which way.

**And one real stale claim I had missed.** The guard's first pass found a
*second* `n > 24000k` further down `histogram.rs` that my own reading of the
file had not. That is the whole argument for the guard, on its first run.

## What this does not do

**It does not fix the cost model.** `cost_at_scale` still prints `chose
TableScan but Index is faster — WRONG at this scale`. The model has one number
for a scanned row and the measurement has four, differing only by what the
store read before. That is unchanged and now has its blast radius written
beside it.

**The blast radius is one crate.** `cargo test -p slate-kernel` only. The ORM,
the server and the three clients were not run under the altered constant, and
the oracle suites that would say whether a *different* plan still returns the
right rows do not fit on this disk in one go.

**The guard reads two figures, not three.** `SCAN_OPEN_COST` is quoted in prose
too ("a scan costs one request to open") and is not checked, because at 1.0 it
is indistinguishable from the many other ones in these sentences and every
pattern I tried matched things that were not claims about it.

**It reads `crates/slate-kernel` only.** `docs/performance.md` quotes
measurements rather than constants and is left to `site/check/docs.py`;
`ledger/` entries are dated records and are exempt by design. A comment in
`slate-slatedb`'s examples asserting a constant's value would not be caught.

**A struck-through figure is skipped by the line.** A withdrawn claim and a
current one on the *same* line means neither is checked. Both corrected
passages put them on separate lines, and nothing enforces that.
