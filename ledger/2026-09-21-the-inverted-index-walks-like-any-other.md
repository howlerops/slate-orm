# An inverted index's walk costs exactly what an ordinary index's does, and the reason the doc gave for expecting otherwise was not true.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #266 (F5l)
- **Touches:** `crates/slate-slatedb/examples/{ascending_walk.rs,cost_calibration.rs}`, `docs/full-text.md`
- **Kind:** measurement

## What changed

A new benchmark, `crates/slate-slatedb/examples/ascending_walk.rs`, answers the
second of the two open questions in `docs/full-text.md` §6. The answer is no:
`POINT_READ_COST` does not need a second value for the text path, and the
reason offered for thinking it might was mistaken.

`docs/full-text.md` §6 now carries the measurement, with the withdrawn
paragraph struck through rather than deleted. `cost_calibration.rs` gains a
header paragraph recording that its GET counts are warm-cache counts, which
nothing said before.

## Why

The doc asked a question it could not answer and said so, which is the right
shape for an unmeasured hypothesis. It also gave a reason:

> It was calibrated on 400 rows reached through an ordinary index, whose
> entries are in *column* order, so the row keys are scattered. Under one term
> of an inverted index the primary keys are ascending, and ascending reads may
> coalesce into far fewer block fetches.

`keys.rs` contradicts the premise in one line. An index entry is
`0x02 <index id> <tenant?> <indexed tuple> <primary key tuple>` — the primary
key is the *suffix*, so under one indexed value an ordinary index's rows are
already in ascending primary-key order. An equality and a term produce the same
walk. There was never a difference for a second constant to hold.

That is an argument, though, and this repository's standard is that a finding
is demonstrated. It is also the kind of argument that is wrong in the
interesting cases: block layout, readahead and the cache could all have made
the two differ for reasons the key encoding does not show.

## Alternatives rejected

**Change `docs/full-text.md` from the code reading alone.** It would have
reached the same conclusion and been worth less. Two things only the
measurement gives: that the dense and spread arms differ by 21× — the variable
the model actually does not know — and that a first version of this benchmark
said the opposite, which is recorded under Evidence and is the reason to
distrust a reading.

**Extend `cost_calibration.rs` rather than write a new file.** Its fixture
would need a text index, which changes the table's block layout and therefore
every number it prints — numbers `docs/performance.md` quotes. A separate file
with its own fixture costs a hundred lines and leaves the recorded ones alone.

**Measure wall clock.** GETs is the unit the cost model is denominated in, it
does not move with the machine, and — the practical part — it does not move
with the optimisation level either, so this runs usefully in a debug build. On
this container a release build of `slate-slatedb` is most of the free disk.

**Fix `cost_calibration`'s warm cache in this change.** It would move every
figure in `docs/performance.md` that comes from it, on a task about full-text
search. Recorded in the file, in this entry and as #269.

## Evidence

Four arms, each returning the same 400 rows of 200,000, each from a freshly
reopened store. Three runs:

| arm | GETs per row |
| --- | --- |
| spread (every 500th row), ordinary index | 1.015, 1.015, 1.015 |
| spread, inverted index | 1.015, 1.015, 1.015 |
| dense (one contiguous run), ordinary index | 0.048, 0.048, 0.043 |
| dense, inverted index | 0.048, 0.043, 0.048 |

**The index's kind changes nothing.** The spread arms are identical — 406 GETs
each, all three runs, both kinds. The dense arms differ by less between kinds
than they differ between runs of themselves, and which kind is higher swaps run
to run, which is what noise looks like.

**Density changes it by 21×**, and the same 21× for both kinds. At 20,000 rows
the same experiment gives 1.067 / 1.067 / 0.040 / 0.040 — the same conclusion
at a tenth the scale, which is the closest thing to a second opinion available
here.

**The first version of this benchmark said the opposite, loudly.** Over one
open store the arms read:

```
spread, ordinary index        400      427        1.067
spread, inverted index        400        1        0.003
dense, ordinary index         400        6        0.015
dense, inverted index         400        3        0.007
```

An inverted index 400 times cheaper — and every bit of it SlateDB's block
cache holding what the first arm pulled in. The arms had been written in the
order that makes the wrong answer look best. `cache_probe` in `slate-headbench`
exists because that cache is real, and this session still walked into it; the
fix is the reopen per arm, and the episode is why the "which variable moved
it?" block prints both comparisons rather than the flattering one.

**`cost_calibration.rs` measures across that same warm cache**, which it never
said. It opens the store once and runs `analyze` — a full table scan — before
the first case, so every case after it reads cached blocks. Run on this machine
today it reports **27 GETs** for the 400-row index equality that
`ascending_walk` measures at **406**. `POINT_READ_COST` is 3.0, from that
file's "400 rows reached by index cost 1,217 requests", and reproduces as
neither number here.

## What this does not do

**It does not settle `POINT_READ_COST`.** Three numbers are now on the table
for the same query shape — 1,217 requests recorded, 27 warm here, 406 cold here
— and this change explains only why two of them differ. The recorded one is
from a fixture and a SlateDB version that may both have moved. #269 is that
work, and until it is done the constant stands, because a constant nobody has
re-measured is better than one changed on a partial reading.

**The density finding is stated, not acted on.** The model charges
`POINT_READ_COST` per row reached by index regardless of how far apart those
rows are; measured, that is 1.015 at a stride of 500 and 0.043 contiguous. A
density-aware read cost would change ordinary index plans far more often than
text ones, which makes it a planner change with a much wider blast radius than
this task, and it wants the cold recalibration first.

**Two arms, two densities, one row width.** A stride of 500 and a contiguous
run are the endpoints, not a curve, and the crossover between them is not
located. Row width is fixed at the padding `cost_calibration` uses, and
`SCAN_ROW_COST`'s own comment says width is what it depends on.

**No arm is unhinted.** Every arm forces its access path, because the planner
would choose a table scan for all four — which is §6's whole subject and is
already measured there. This says what the path costs, not which one is picked.

**Nothing runs this file, either.** It joins the six other examples in
`slate-slatedb`, in exactly the state `slate-headbench`'s five were in before
#265 — compiled by `clippy --all-targets` and run by nobody. Named in #269.

**The tokenizer's cost is inside the numbers and not separated.** The inverted
arms tokenize the caller's text and the ordinary arms do not. It is a few
microseconds against reads that dominate, and it is not zero, and nothing here
measures it apart.
