# ~~A timing threshold turned CI red because the clean reading under it had moved 23%, not because the machine was busy.~~ The threshold was never portable to this class of machine, and nothing had moved.

> **Withdrawn the same day, by measurement.** The "23% shift" below is wrong.
> It compared a number recorded on *another machine* in September against one
> measured here, which is the mistake `CLAUDE.md` names outright — a ratio is
> not as portable as I assumed it was. Checking out `af3c975`, the commit whose
> entry recorded the small pair clean at **0.868**, and measuring it on this
> four-core container gives **1.111 – 1.175, median 1.138**. Nothing drifted:
> the pair reads about the same today as it did then, and if anything slightly
> lower. See "The correction" below. The change itself — a bound per pair, set
> from readings taken here — stands, and is better supported by this than by
> what it was written on.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #259 (F5e)
- **Touches:** `crates/slate-wasm/tests/timing.rs`
- **Kind:** fix

## What changed

`the_clock_stops_before_the_rows_are_rendered` had one threshold, `1.15×`, for
both of its pairs. It now has one per pair — `1.40×` for the 4,837-row pair and
`1.15×` for the 100,000-row one — each set from a measurement of what that pair
reads clean and what it reads with the mutation it exists to catch. The failure
message prints the observed ratio and the bound, so the next reader does not
have to divide two numbers to see how close it was.

## Why

CI run 295 went red on this test at `23.620 / 18.862 = 1.252`, on a commit
touching `slate-server`, `slate-serverd`, scripts and examples — none of which
`slate-wasm` compiles or links (it depends on `slate-kernel`, `slate-sql`,
`slate-schema`, `slate-tuple`). The runs before and after were green on
byte-identical `slate-wasm` code.

~~The interesting part is *why* it was close enough to fail. The ledger entry of
2026-09-14 records the calibration: small pair clean at 11.2 vs 12.9 ms, a ratio
of 0.868, with the mutation worth 1.26×, and a threshold at 1.15×. Today the
same pair reads 1.07 clean — the plain select has gone from 13% faster than its
grouping to 7% slower. The threshold did not move; the thing under it did,
until 1.15 was six percent above a clean reading.~~

## The correction

**That was wrong, and the way it was wrong is worth more than the claim.** The
0.868 was measured on whatever machine ran it in September; 1.07 was measured
here. Comparing them is comparing two machines, and `CLAUDE.md` says not to
report a number you did not observe — a *ratio* felt portable enough to compare
across machines, and it is not.

Measured instead: `git checkout af3c975`, the calibration commit itself, patched
to print rather than assert, three runs on this four-core container —

| pair | at `af3c975`, here | at HEAD, here |
|---:|---|---|
| 4,837 | 1.111 – 1.175 (median 1.138) | 1.045 – 1.092 |
| 100,000 | 0.912 – 0.956 (median 0.915) | 0.888 – 0.920 |

So the small pair reads *slightly lower* today than at the commit the threshold
was set on. Nothing drifted. (The two columns are not perfectly comparable —
that version sampled `best(..., 7)` per statement and this one alternates
`paired(..., 9)`, and alternating with more samples should read lower — but no
amount of sampling difference turns 0.868 into 1.14.)

**What the measurement actually shows is worse than drift.** At the calibration
commit, on this machine, the small pair's *maximum* was 1.175 — already past the
1.15 bound that commit shipped. The threshold was never adequate here; it was
adequate on the machine it was measured on. A single number measured once, on
one box, is a claim about that box, and this one was read as a claim about the
code.

A threshold is a claim about a measurement, and a claim whose measurement was
never portable is stale documentation with a `cargo test` attached to it.

## Alternatives rejected

**More samples.** The first hypothesis, and the measurement withdrew it. At
best-of-25 the small pair reads 1.046–1.078; at best-of-9, 1.048–1.080. No
movement, because ~1.06 is what the pair *costs* rather than what the machine
was doing to it. `paired` stays at nine. (The large pair did improve —
0.917–1.006 at nine, 0.878–0.934 at twenty-five — which is consistent: its noise
is sampling noise and the small pair's floor is not.)

**Re-run the job and move on.** The repository's own rule: "flake is not a root
cause." A re-run would have been green and the next commit would have paid the
same coin toss.

**Widen the single threshold to 1.20 for both.** Would have left the small pair
at ten points of headroom and pushed the large pair's bound to within 0.09 of
its own mutated reading — trading one pair's false positives for the other's
false negatives. The pairs are not alike and one number for both was the defect.

**Delete the small pair and keep the large one.** Tempting, because the large
pair is the comfortable one. Refused: the small pair is the one that *catches*
the mutation first, at 1.597× against its bound, and it is the pair where
rendering is a larger fraction of the work. Dropping it would have removed the
sharper of the two detectors to avoid recalibrating it.

~~**Chase why the ratio moved from 0.868 to 1.07.** Out of scope here and
deliberately left open — see below.~~ Chased immediately afterwards, and there
was no move to explain; see "The correction" above. Leaving it as an open
question was still the right call at the time the entry was written — the
alternative was to absorb an unexplained 23% into a new threshold — and the
question turned out to be cheap to answer, which is an argument for answering
rather than recording one.

## Evidence

Measured on this four-core container, best of nine, four runs each, with the
assertion temporarily replaced by a print (anchor asserted to occur exactly
once, file restored in a `finally`):

| pair | clean | `render` inside the clock |
|---:|---|---|
| 4,837 | 1.045 – 1.092 | 1.699 – 1.744 |
| 100,000 | 0.888 – 0.920 | 1.294 – 1.300 |

So the small pair's gap is 1.09 → 1.70 and the large pair's is 0.92 → 1.29. The
new bounds sit roughly at the geometric middle of each: 1.40 and 1.15. The old
single 1.15 sat 6% above the small pair's clean reading and 25% above the large
pair's — the asymmetry is the whole finding.

Sampling, measured under four spinning CPUs and again idle:

| | best-of-9 | best-of-25 |
|---|---|---|
| 4,837, four spinners | 1.048 – 1.080 | 1.046 – 1.078 |
| 100,000, four spinners | 0.917 – 1.006 | 0.878 – 0.934 |
| 4,837, idle | 1.051 – 1.103 | — |
| 100,000, idle | 0.908 – 0.927 | — |

`scripts/mutate.py`, one case: `render` moved back inside the clock is caught,
by name, and the message says which pair and by how much —
*"returning 4837 rows took 27.508 ms but folding the same read into one group
took only 17.228 ms (1.597×, and this pair's bound is 1.40×)"*. So the small
pair still catches it with 0.20 to spare while gaining 0.31 of headroom over
its clean reading, where before it had 0.06.

`cargo test -p slate-wasm --test timing`: 6 passed. It also passes here 10 out
of 10 including under four spinning CPUs, which is why the failure had to be
diagnosed from CI's numbers rather than reproduced.

**Two mutations I wrote were worthless and are recorded as such.** Widening each
bound and running the *unmutated* suite: both "survived", which is what should
happen — a wider bound on clean code passes, and the mutation tests nothing
about whether the bound still catches anything. The useful form is to mutate the
binding and read which pair fires, which is the run above. A mutation that
cannot fail is not evidence, and `mutate.py` reporting it as a survivor was the
tool doing its job on a badly chosen case.

## What this does not do

~~**It does not explain the 23% shift.**~~ There was no shift. Answered by
measurement rather than by bisection, in one checkout: see "The correction".
What remains true is the weaker and more useful statement — **nobody has ever
measured this pair on more than one machine at a time**, and both the old
threshold and the two new ones are numbers from a single box.

**It does not make the test machine-independent, and that is now known to be
the actual defect.** The bounds are still numbers measured on one container,
and CI's box is shared with tenants this one is not. The small pair's CI
excursion of 1.252 now fits under 1.40 with room, but nothing proves a worse
excursion is impossible — only that the margin went from 0.06 to 0.31 on this
machine. The correction above shows the previous bound failing on this machine
*at the commit that set it*, which is the same failure one machine later.

**Nothing re-runs the measurement automatically.** The table above is a comment,
and a comment goes stale exactly the way the one it replaces did. A check that
compared today's ratio against a recorded one and failed on a large move would
be the real fix, and would also have caught the 23% shift when it happened. It
is not here.

**The large pair's catch is inferred, not observed.** The assertion loop stops
at the first failing pair and the small one fires first, so the mutated run
never reaches the large pair's assertion. Its 1.294–1.300 mutated reading
against a 1.15 bound is the evidence instead, which is a measurement rather than
a test failure.
