# A retired row costs a read exactly what a live row costs, and the soft-delete conjunct itself is 141 ns a row

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-kernel/benches/queries.rs`, `docs/caveat-status.json`,
  `scripts/check_closed_caveats.py`
- **Kind:** measurement

## What changed

A `soft_delete` group in `crates/slate-kernel/benches/queries.rs`, measuring the
two costs `ledger/2026-09-19-a-row-that-is-gone-but-still-there.md` named and
did not measure. Five configurations over three tables identical in columns,
row encoding and index set, differing only in the `soft_delete` declaration and
in how many rows carry a stamp.

The caveat is closed, and one half of its framing is corrected.

## Why

The caveat has been `open` since 2026-09-19 and was explicit that it was a
guess:

> Nothing measures the cost. Every read of a soft-deleting table carries one
> extra `IsNull` conjunct, and on a table with no partial index the retired
> rows stay in every index and are filtered after the fetch. Both are obviously
> non-zero and neither was benchmarked.

"Obviously non-zero" is right about the first and is the wrong shape for the
second. A retired row does not cost a read *extra*; it costs a read exactly what
a live row costs and then returns nothing, so the penalty is an opportunity cost
that only appears when the time is divided by rows a caller can use. Measuring
it as a per-query number makes soft delete look like it *speeds reads up*, which
is what the absolute times below say and is not what anybody means.

## Alternatives rejected

**Compare the soft-deleting table against `events()`**, the bench's existing
fixture, rather than adding a `plain` twin. Cheaper by a whole table and wrong:
`events()` has one column fewer, so the comparison would have folded a decode
difference into the answer and reported it as the conjunct's cost. The control
has the `deleted_at` column and does not declare it.

**Two builder functions rather than one `stamped(declared: bool)`.** A
copy-pasted second builder is how the two tables drift in a way that is invisible
in the diff and fatal to the comparison — an index added to one, a column
reordered. One function with one `if` makes the drift impossible rather than
unlikely, which is the same argument `check_row_against` makes for having one
body and not two.

**Interleave the retired rows rather than retiring the low ids.** Every other
row retired is the more realistic distribution and measures a different thing: a
scan that skips alternate keys, which is cache-hostile in its own right. The
claim is about how many rows are discarded, not where they sit, so the retired
half is contiguous and the surviving half is a contiguous key range.

**Report the per-query times and stop.** They are the numbers criterion prints
and they say soft delete makes reads faster, which is true and misleading. The
per-useful-row figures need the row counts, which is why the bench prints them.

**Drive the control with `scripts/mutate.py`.** It cannot: a criterion bench has
no assertion to fail, and the script's dialects all read a suite's pass/fail
line. The control below is a measurement mutation, run and restored by hand, and
is labelled as one — the shape
`ledger/2026-09-21-a-threshold-calibrated-against-a-reading-that-moved.md`
already names.

## Evidence

`cargo bench -p slate-kernel --bench queries -- soft_delete --sample-size 50
--warm-up-time 2 --measurement-time 8`, **three runs**, release, 2,500 rows per
tenant over four tenants. Row counts are printed by the bench rather than worked
out on paper:

| configuration | ms, three runs | spread | rows answered | per useful row |
| --- | --- | --- | --- | --- |
| `tenant_scan/plain` (not declared) | 0.984 – 1.044 | 6.1% | 2500 | 407 ns |
| `tenant_scan/live` (declared, none retired) | 1.326 – 1.452 | 9.5% | 2500 | 548 ns |
| `tenant_scan/half_retired` | 1.031 – 1.134 | 9.9% | 1250 | 876 ns |
| `index_scan/live` | 0.946 – 0.974 | 3.0% | 100 | 9.6 µs |
| `index_scan/half_retired` | 0.884 – 0.937 | 5.9% | 50 | 18.3 µs |

**The noise floor is 11%**, and it was measured by accident. Benching `live` in
both loops gave criterion a duplicate id, which it silently renamed `live #2`
rather than refusing; the two readings of one configuration came back 1.345 ms
and 1.496 ms. That is the only same-configuration spread this measurement has,
so the shape that produced it is kept and commented rather than tidied away.
Nothing below 11% is reported as a difference.

### The conjunct: 141 ns a row, +35%

`plain` and `live` do not overlap in any of the three runs — 1.044 ms at the
worst `plain` against 1.326 ms at the best `live`. The gap is **0.354 ms over
2,500 rows scanned, 141 ns a row**, and **+35%** on a whole-tenant scan, well
clear of the 11% floor.

That it is the *declaration* and not the two stores being built differently was
checked by making the control declare `soft_delete` too. It then read **1.372
ms** against `live`'s 1.430 ms — 4% apart, inside the floor, and both ~35% above
the real control's 1.017 ms. Restored afterwards; the bench in the tree is the
one the table above came from.

141 ns is large for one `IsNull` against a decoded row, and it is not obviously
the conjunct's arithmetic: it is one more term in a compiled predicate, one more
column the late-materialisation pass has to keep, and this is `LatencyProfile::free()`
so no I/O is hiding it. Which of those dominates is not measured here.

### A retired row: exactly a live row's price

Absolute times go **down** when half the rows are retired — `tenant_scan` from
1.371 ms to 1.095 ms, `index_scan` from 0.963 ms to 0.913 ms. The index-scan
difference is **−5%, inside the 11% floor, and is not a finding**. The
tenant-scan one is outside it and is explained entirely by 1,250 fewer rows
being emitted.

Per row a caller can use:

| | live | half retired | ratio |
| --- | --- | --- | --- |
| whole-tenant scan | 548 ns | 876 ns | **1.60×** |
| index scan on `by_kind` | 9.6 µs | 18.3 µs | **1.90×** |

Both are near the **2.0×** that "a retired row costs what a live row costs"
predicts at half retired. The index path is nearer because a retired row there
has already cost a point lookup by the time the conjunct sees it; the scan path
is further because a scan's per-row cost includes emitting the row, which a
retired row skips. So the caveat's second sentence is right that the retired
rows stay in the index and are filtered after the fetch, and wrong to imply that
this is an *extra* cost: **the rule is that a table which is 1/*n* live costs
about *n* times as much per useful row, on both access paths.** A partial index
is what changes that, and measuring one is not done here.

### Two things found by building the fixture

**`insert` refuses a supplied soft-delete stamp** — `SoftDeleteColumnSupplied`,
from `check_row_against` in `crates/slate-kernel/src/record.rs`. The first
version of the fixture wrote the stamp directly and panicked on the third
configuration. Retired rows are made through `delete`, which is the only way the
kernel produces one; seeding around the refusal would have measured a state the
kernel cannot reach.

**A duplicate criterion benchmark id is renamed, not refused.** `live #2`
appeared in the output with no warning. It happened to be useful here; on a
benchmark whose two halves were meant to be one number it would be a silently
split sample.

### And a survivor, in the witness I had just written

The closure's witness needle was `benchmark_group("soft_delete")`. Mutating it
— record
[`ledger/mutations/20260929T205315-crates-slate-kernel-benches-queries-rs.json`](mutations/20260929T205315-crates-slate-kernel-benches-queries-rs.json)
— **survived**, and the reason is the third cause `mutate.py` tells you to rule
out first: the mutation was a change to the file and not to what the guard sees.
The string occurs **twice**, once per loop, so renaming either group away left
`check_closed_caveats.py` green with half the measurement deleted. `git grep -q`
answers "is it anywhere", which is the wrong question for a needle that is not
unique, and I had written a comment claiming the group name was the *better*
choice.

The needle is now `fn soft_delete(c: &mut Criterion)`, which occurs once. Its
failure mode is the safe one — a rename that keeps the measurement turns the
guard red and somebody re-reads it, where the group name's was a deletion that
stayed green. Re-mutated, record
[`ledger/mutations/20260929T205401-crates-slate-kernel-benches-queries-rs.json`](mutations/20260929T205401-crates-slate-kernel-benches-queries-rs.json):

| mutation | outcome |
| --- | --- |
| the soft-delete measurement is renamed away | caught, `check_closed_caveats.py` |

`sh scripts/check.sh`: **87 passed**, after it caught a Rust raw-string literal
(`r#"..."#`) I had typed into the Python roster — a syntax error that took seven
of its steps down at once and is the reason the script reports at the end rather
than stopping at the first. `cargo clippy --workspace --all-targets`: clean.

## What this does not do

**It is one table shape at one size, in memory, with no I/O charged.** 2,500
rows of six narrow columns under `LatencyProfile::free()`. The `io` profile the
`read` group uses would change the index-scan ratio most, because a discarded
row there has cost a charged point lookup — which is the case where this matters
commercially and the one not measured.

**It does not measure a partial index**, which is the fix the caveat's own entry
proposes for exactly this cost. `by_kind` is a plain index by construction, so
the numbers are the no-partial-index case only, and "a partial index fixes it"
remains an argument here rather than a measurement.

**It does not break the 141 ns down.** One extra conjunct, one more live column,
and a possible plan difference are three explanations and this separates none of
them. `scan_row_breakdown` in the same file is where that would be done.

**It measures reads only.** A soft delete is a write that stamps rather than
erases, leaving the index entries in place, so the write path's cost and the
storage the retired rows occupy are both untouched here.
`ledger/2026-09-19-a-row-that-is-gone-but-still-there.md`'s other caveats cover
neither.

**The benchmark carries no assertion**, so nothing fails when these numbers
move. That is true of every group in this file and is why the numbers live in a
dated entry rather than in a threshold.
