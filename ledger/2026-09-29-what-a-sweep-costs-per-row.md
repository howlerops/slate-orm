# A purge costs about four microseconds a row, and the two-point line I wanted to quote is noise

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-kernel/benches/queries.rs`, `docs/caveat-status.json`
- **Kind:** measurement

## What changed

A `purge` group in the kernel's bench, over the soft-deleting fixture added
this afternoon: 2,500 and 10,000 retired rows erased by `purge_deleted`, seeded
fresh per iteration.

## Why

`ledger/2026-09-20-the-sweep-nobody-scheduled.md` shipped a retention job and
said what it did not know:

> **Nothing measures a sweep's cost.** How long a purge of a million rows takes,
> and what it does to concurrent writes, is unknown and unasked.

A purge erases a row *and every index entry that pointed at it*, so it is the
one write path whose cost scales with the index count as well as the row count,
and the retention example shipped with no number at all beside it. An operator
choosing between `--once` nightly and `--every` hourly is choosing on nothing.

## Alternatives rejected

**`iter` rather than `iter_batched`.** A purge is destructive: the second
iteration would find nothing left and measure an empty scan, which is a number
that looks like a fast purge. The batched form seeds a fresh store per
iteration and criterion excludes the setup from the timing, which matters here
because the seeding is several times the purge.

**The async bencher, to match the other groups.** It cannot: `stamped_store`
seeds through `runtime.block_on`, and `iter_batched`'s setup runs on the async
bencher's own runtime thread — "cannot start a runtime from within a runtime",
met on the first run. The sync bencher with `block_on` in the routine is the
only arrangement that reuses the same seeding helper.

**Measure a million rows directly.** Seeding a million rows runs once per
iteration and the disk on this container has been at 100% twice today. Two
sizes an order of magnitude apart, with the extrapolation stated as an
extrapolation, is what fits — and the section below is about how much less that
buys than it looks.

## Evidence

Three runs, release, `--warm-up-time 1 --measurement-time 5`, sample size 10.
Rows are the count actually erased (four tenants × the per-tenant figure):

| rows erased | ms, three runs | per row |
| --- | --- | --- |
| 2,500 | 14.49, 16.45, 16.12 | **5.80 – 6.58 µs** |
| 10,000 | 44.67, 40.48, 42.36 | **4.05 – 4.47 µs** |

The two per-row bands **do not overlap**, so the cost per row genuinely falls
with size — there is a fixed component. At 10,000 rows a purge costs about
**4 to 4.5 µs a row**, which puts a million rows at **roughly four seconds**
in memory with no I/O charged.

### The decomposition is noise, and I nearly published it

The obvious next step is to solve the two points for a line and quote "X µs a
row plus Y ms fixed". I did, and the first run gave a clean-looking **4.02 µs a
row, 4.43 ms fixed**. Across all three runs:

| run | per row | fixed |
| --- | --- | --- |
| 1 | 4.02 µs | 4.43 ms |
| 2 | 3.20 µs | 8.44 ms |
| 3 | 3.50 µs | 7.38 ms |

**The slope spreads 26% and the intercept very nearly doubles**, because
subtracting two measurements that each carry ~10% noise and dividing by their
difference amplifies it. Two points and a line is one equation pretending to be
two. The per-size bands above are the measurement; the decomposition is not
reported as a finding, and the "four seconds" extrapolation is an
order-of-magnitude statement rather than an arithmetic one.

Had I run once, the 4.02 and 4.43 would have gone into this entry as a
calibration. That is the third time today a single run would have published
something the spread contradicts.

## What this does not do

**It says nothing about concurrent writes**, which is the caveat's other half
verbatim. A purge holds a transaction and the question is what a writer
contending with it sees; that needs the concurrency harness, not this bench.
The caveat is `narrowed` rather than `closed` for exactly this.

**A million rows is extrapolated, not run.** Two sizes an order of magnitude
apart show the per-row cost falling; nothing says it keeps falling, flattens,
or turns over at a size where the store's own structure changes — and
`ledger/2026-09-23-the-cliff-does-not-reproduce-and-the-knob-does-place-it.md`'s loader cliff is this repository's own example of a
curve that bends at a size nobody had reached.

**In memory, with no I/O charged.** `LatencyProfile::free()`, like the
`soft_delete` group beside it. On object storage a purge is erasing keys, so
the charge per row is a write round trip and the figure here is the CPU floor
under it rather than an estimate of it.

**One index set.** Three secondary indexes on the fixture's table, so three
entries erased per row plus the row. A table with ten indexes costs more per
row and nothing here says by how much — it is the most obviously linear knob
and the cheapest follow-up.

**The table is rebuilt inside the timed region.** `iter_batched`'s routine is
an `FnMut` and the async block must own what it moves, so `stamped(true)` is
called per iteration inside the timing. It is a handful of allocations and it
is the same cost at both sizes, so it inflates both readings equally and does
not move the per-row comparison — but it is in the numbers above.
