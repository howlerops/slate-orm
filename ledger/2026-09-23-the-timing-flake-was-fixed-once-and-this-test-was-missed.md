# CI inverted a 1.8x ordering to 1%, and the fix was already in the same file — applied to the test below this one and not to this one.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #297 (F6q)
- **Touches:** `crates/slate-wasm/tests/timing.rs`
- **Kind:** a red CI on my own push, root-caused to a known pattern the repository had already solved

## What changed

CI run 325 failed `the_reported_time_grows_with_the_work` on a commit that
touched one Python regex:

```text
one zone (563.469 ms) should beat the whole table (558.564 ms)
```

Sampled alone on this container the two are **355 ms and 650 ms** — a 1.8x
ordering, not a marginal one. What inverted it is that the three statements
were sampled in three separate `best` batches, so each met whatever load the
machine happened to be under during its own block.

That failure mode is written down four lines above, on `paired`:

> Two separate batches of `best` is what this was, and it is flaky on a busy
> machine: whichever statement happens to be sampled during a bad patch loses,
> and under four spinning CPUs the pair inverted by 2.6×.

`paired` took exactly two statements, so it was applied to the `GROUP BY` test
and this three-statement one was left on the old pattern. It takes a slice now,
as `round_robin`, and both use it.

## Why

My diff could not have caused this — a regex in `scripts/mutate.py` does not
reach `slate-wasm` — but it is red CI on a branch I own, so it is mine to get
green. The interesting part is that root-causing it found the repository had
already diagnosed and fixed this exact thing, and the fix had a shape
(`left`, `right`) that quietly excluded the one test with three subjects.

A helper that fits two cases and the bug affects three is how a fixed bug stays
half-fixed.

## Alternatives rejected

**Re-run the job and move on.** The rules here allow one re-run to confirm a
flake. It would have gone green — it passes 3/3 locally and passed twice more
under 12 spinners — and left a test that inverts on a loaded runner for the
next person. A flake is the failure this repository is least equipped to ignore:
`docs/performance.md` is built on these numbers.

**Widen the margin, or assert a ratio.** The suite's own preamble argues against
it: none of these tests assert anything is *fast*, only that the ordering holds,
because an ordering survives a noisy machine and a ratio does not. A threshold
picked to pass on CI is a threshold that stops catching the mutation it exists
for — and the mutation *is* caught, twice, in the record below.

**Drop the `zone < scan` assertion.** My first reading was that it compares two
table scans and is thin by construction. That was wrong: measured, the gap is
1.8x and the ordering is real. I had inferred "under a percentage point" from
the single failing observation rather than from a measurement, which is the
error this repository's standards name first.

**Fix `best` instead.** It is now down to one caller, comparing a batch of five
against a single `timed` UPDATE — the same batched shape, with a structurally
larger margin because the UPDATE contains the read it is compared against.
Left alone and named here rather than changed without a reason to.

## Evidence

Two mutations, **all caught**, in
[`ledger/mutations/20260923T142543-crates-slate-wasm-tests-timing-rs.json`](mutations/20260923T142543-crates-slate-wasm-tests-timing-rs.json):
returning the samples in the wrong order, and keeping the worst sample instead
of the best. A third, sampling only the first statement, was caught in
[`20260923T142341`](mutations/20260923T142341-crates-slate-wasm-tests-timing-rs.json).

Six timing tests pass, three runs. `sh scripts/check.sh`: **48 passed, all of
them.**

## What this does not do

**I could not reproduce the inversion.** Not under four spinning CPUs, not
under twelve on four cores, and not with a burst timed onto the middle batch
across three attempts. Steady load slows both batches alike; what CI met was
presumably a burst against twenty-one concurrent jobs, and I could not
synthesise it. So this is a principled fix for a mechanism the file already
documents — **not** one I demonstrated end to end, and that is weaker than this
repository's usual bar.

**Round-robin does not make the comparison load-proof**, only load-*fair*. A
burst long enough to span several rounds still hits whichever statement is
sampled during it; the minimum over five rounds is what absorbs that, and five
is a number nobody has justified.

**The third mutation I tried was a no-op** — `statements.iter().cycle().take(1)
.chain(statements.iter().skip(1))` is `statements.iter()` — and `mutate.py`'s
new message about equivalent mutations, written an hour earlier in #296, is what
flagged it. Fourth of the day.
