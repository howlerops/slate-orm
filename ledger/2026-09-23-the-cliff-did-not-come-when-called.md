# The loader-cliff hypothesis stood unchallenged since #118 behind one sentence: "the size that would settle it is the size that will not finish." That sentence was wrong, and the hypothesis it protected is now refuted.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #292 (F6l)
- **Touches:** new `crates/slate-slatedb/examples/loader_cliff.rs`, `docs/performance.md`, `README.md`, `scripts/{run_examples.sh,test_run_examples.py}`
- **Kind:** a negative result, and the argument that made it reachable

## What changed

`README.md` has said since #118 that the loader stops being linear between
500,000 and 600,000 rows, that SlateDB's 64 MiB `l0_sst_size_bytes` "lines up
suspiciously well", and that **the size that would settle it is the size that
will not finish.**

That last clause assumes the test must run *at* the cliff. It need not.
SlateDB pauses writers when L0 fills, and L0's capacity is `l0_max_ssts` ×
`l0_sst_size_bytes`. If that is the mechanism, shrinking the knob shrinks L0 in
proportion and **the cliff comes down to meet you**.

It does not come.

| `l0_sst_size_bytes` | `max_unflushed_bytes` | L0 capacity | 120,000 rows | slow chunks |
|---|---|---:|---:|---|
| 64 MiB (stock) | 1 GiB (stock) | 512 MiB | 2.4 s | none |
| 8 MiB | 1 GiB | 64 MiB | 2.4 s | none |
| 64 MiB | 128 MiB | 512 MiB | 2.4 s | none |
| 4 MiB | 8 MiB | 32 MiB | 2.6 – 2.7 s | one, at 90–100k |

<!-- not a measurement -->

Sixteen times less L0 and 128 times less unflushed budget produce one slow
chunk and a 12% slower load. Not a cliff.

## Why

A hypothesis nobody can test is indistinguishable from a hypothesis nobody has
tested, and this one had sat for as long as the repository has existed. The
blocker was never the machine; it was an argument about what the experiment had
to be. Moving the threshold instead of reaching it costs two minutes a run.

## Alternatives rejected

**Reach the cliff.** 500,000 rows of this fixture need roughly 550 MB and this
container has under a gigabyte free. It is also the run that "never finished"
four times before. Every attempt spends an hour to reproduce something already
recorded as reproducible.

**Predict the cliff's row count and print it.** The first draft divided L0
capacity by bytes per row. The in-process `s3s` counts requests and not bytes,
so that denominator would have had to be a *recorded literal* — the exact
defect half this session has been unwinding. The claim needs no such number:
run it at two knob settings and see whether the cliff moves.

**Conclude from one run.** The tightest setting produced a single slow chunk,
which is what a cliff would look like at its onset. Three runs give 0.31 s,
0.40 s, 0.31 s against a 0.21 s baseline, at 90,000 or 100,000 rows. Repeatable,
small, and not a level shift — reporting the first run alone would have claimed
a confirmation.

**Leave `max_unflushed_bytes` alone.** It was not in the hypothesis, but the
run made it obvious: a 120,000-row load issues **under thirty PUTs**, so the
data barely reaches L0 and a knob governing L0 governs almost nothing. Testing
the neighbouring backpressure knob is what turns "the hypothesis failed" into
"neither write-backpressure knob does this".

## Evidence

Four configurations, `--release`, build line on every run, in
[`performance.md` §8d](../docs/performance.md). The instrument is committed as
an example so the next person re-runs rather than rebuilds it, with
`SCALE_ROWS` for the smoke path and `L0_SST_MB` / `MAX_UNFLUSHED_MB` for the
experiment.

What is now known: the cliff is **above 400,000 rows** — §8c loaded that on
stock settings — and is **not set in proportion to either backpressure knob**.

**Two guards fired on this change and both were right to.** The example roster
floor needed bumping 8 → 9; and `test_run_examples.py` then failed four cases,
because it carried `LEAST = 8`, a hand-kept copy of a number living in a `case`
arm one file over. It reads the arm now. A constant copied into a test is worse
than one copied into prose: it fails the next unrelated change rather than the
next reader.

`sh scripts/check.sh`: **47 passed, all of them.**

## What this does not do

**It does not say what the cliff is.** It removes one candidate and its
neighbour. The remaining suspects — compaction, manifest growth, memory, the
index write path — are untouched, and nothing here narrows between them.

**It never reproduced the cliff.** Every configuration tested loads 120,000
rows in about two and a half seconds. The refutation is that the cliff *did not
appear where the hypothesis says it should*, which is weaker than reproducing
it at a smaller scale and watching it move. If the mechanism is one that only
engages above some absolute size, this design cannot see it, and that is a real
limit rather than a caveat.

**One row shape, one machine, a loopback `s3s`.** Row counts here are
properties of this fixture.

**The example is not in CI's measured path** beyond the smoke run at 2,000
rows, which loads in 0.1 s and would not show a cliff if one existed.
