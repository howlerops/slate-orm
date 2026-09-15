# Half of the 1,290 bytes a row was never the store's, and 88 of what was is now gone

- **Date:** 2026-09-15
- **Author:** Claude Opus 5, at Jacob's direction
- **Touches:** `crates/slate-kernel/src/memory.rs`, `crates/slate-kernel/tests/footprint.rs`, `crates/slate-slatedb/examples/row_footprint.rs`, `crates/slate-slatedb/tests/footprint.rs`, `crates/slate-slatedb/Cargo.toml`, `docs/performance.md`
- **Kind:** performance

## What changed

`docs/performance.md` §7b said the in-memory store holds about 1,290 bytes for
rows that serialise to 110 — "twelve times what the data weighs" — and said
plainly that this had never been investigated. A new example
(`row_footprint`) counts allocations instead of reading RSS, and the number
comes apart into three pieces, two of which are not the store.

Then two lines: `MemoryTransaction::put` shrinks its buffers before they become
`Bytes`. That is worth 88 bytes a row, and the load got *faster*.

## Why

The factor decides what a browser tab holds and what a node holds, so it is
load-bearing for every scale claim on the site. It was also the largest
unexamined number in the performance document, and the document says so —
which is the right way to carry a debt, but not forever.

## Alternatives rejected

**Trust RSS and optimise against it.** What the old harness did. Rejected once
it was clear RSS could not distinguish the store from the source rows sitting
beside it; every conclusion drawn from it would have been about the harness.

**A hand-rolled counting global allocator.** Fifteen lines, no new dependency,
and what this started as. Rejected because the workspace `forbid`s
`unsafe_code`, and weakening a workspace-wide `forbid` so an example can count
bytes is a bad trade against `dhat`, whose entire job is to do that unsafe part
once and correctly. `dhat` is a dev-dependency; it never reaches a consumer.

**Shrink inside `slate_tuple::encode`.** One place instead of two, and it would
catch every caller. Rejected because most callers are not storing: `encode`
also runs for scan bounds and for per-row predicate comparisons, which are
dropped immediately. A shrink there is a realloc on the read path buying
nothing. `MemoryTransaction::put` is the one place a buffer becomes *stored*.

**Size the encoder's buffer exactly instead of shrinking after.** Strictly
better in principle — no realloc at all — and it needs a size pass over the
values before encoding them, so it trades a realloc for a second traversal and
a second place that must agree with the encoder about how many bytes each value
takes. That second place drifting from the first is a corruption bug, not a
size regression. Not worth it for 88 bytes when `shrink_to_fit` measured free.

**Change the representation: interned strings, a column store, `Box<[u8]>`
instead of `Bytes`.** The largest single component is 128 bytes a row of inline
`Bytes` headers, 32 bytes each for two entries. `Box<[u8]>` would halve that.
Rejected as out of scope for a measurement task and too large to do without a
reason beyond bytes — `Bytes` is what makes a snapshot O(1) to clone, which is
what makes `begin` O(1), which was a finding in its own right (§16).

## Evidence

Counted with `dhat`, which reports the bytes each allocation *requested*, so
the figures are properties of the program rather than of the machine. 100,000
trips, release build, staged so each figure is a difference between readings:

| | total | per row |
|---|---:|---:|
| source rows, before the store exists | 45.2 MB | 474 |
| + `insert_many` | 91.9 MB | 964 |
| + `analyze` | 92.0 MB | 964 |
| **− the source rows: the store itself** | **46.8 MB** | **491** |

**474 of the 1,290 was the source `Vec<Row>`**, which the old harness held for
the whole run and RSS duly counted — `Value` enums and a `String` each. The
store is **491 bytes a row over 86 bytes of keys and values: 5.7×, not 12×.**

Against the floor:

| | per row |
|---|---:|
| keys and values, logical | 86 |
| the same pairs in a bare `BTreeMap<Bytes, Bytes>` | 219 |
| — of which inline `Bytes` headers | 128 |
| the same pairs, allocated the way the writer allocates them | 369 |
| the store | 491 |

**A hypothesis, tested and rejected.** `Shared::history` keeps a `BTreeSet` of
every key a commit wrote, and a bulk load puts 200,000 keys in one. It was the
leading suspect and it is worth **two allocations a row and no measurable
bytes** — the `Bytes` in the set share the map's buffers — and `trim_history`
drops it on the next transaction drop regardless.

**What was worth 88 bytes.** The 219→369 gap is capacity slack:
`slate_tuple::encode` reserves nine bytes a value, `keys` a header plus the
same, and `Bytes::from(Vec)` adopts the vector's **capacity**, not its length,
for the life of the entry. Shrinking at `put` takes the store to **403 bytes a
row**, 4.7×.

It also made the load faster, which was not the expected direction — two
prebuilt binaries, alternating runs, ranges non-overlapping:

| | 100,000 rows |
|---|---:|
| without the shrink | 0.29 s every run |
| with it | 0.26–0.28 s |

Smaller blocks and less memory touched, presumably; that is an explanation and
not a measurement. What matters is that nothing was traded away.

**Not the SlateDB path.** `slatedb`'s `put` takes `AsRef<[u8]>` and copies into
its own batch, so it never adopts a caller's capacity. This is `MemoryStore`'s
alone — the store the browser runs on, and the one §7b measured.

**A test that did not test what it said.** The first guard turned a stored
`Bytes` back into a `Vec` and asserted `capacity() == len()`. It passed with
the shrink removed: `Vec::from(Bytes)` reuses the allocation only when the
`Bytes` uniquely owns it, and a read out of the store never does, so it copies
and the capacity always equals the length. Capacity is not observable through
`Bytes` at all. The real guard is `slate-slatedb/tests/footprint.rs`, a dhat
ceiling of 450 bytes a row, which passes at 403 and fails at 487 with the
shrink removed — mutation-tested both ways. What stayed in
`slate-kernel/tests/footprint.rs` is the half that could corrupt something:
shrinking reallocates, and the bytes, their order, and the tombstone that
matches them must survive it.

## A flake this surfaced, and fixed

Running `slate-kernel`, `slate-slatedb`, `slate-wasm` and `slate-orm` at once
failed `the_clock_stops_before_the_rows_are_rendered`, which had passed alone
and in CI. It reproduces on demand with four spinning CPUs: the pair inverted
2.6× — 38.1 ms against 14.8 — against a threshold of 1.15.

The flaw was in the sampling, not the threshold. The test took the best of
seven runs of one statement and then the best of seven of the other, so
whichever batch happened to land in a bad patch lost, and widening the
threshold enough to survive that would have taken it past 260% — well past the
26% the mutation is worth, which is to say it would catch nothing.

It now alternates the two statements and keeps each side's minimum *kernel*
reading, so both go through the same weather and contention, which only ever
adds, is minimised out. Green twice under the load that reliably broke it, and
still fails on the render mutation (20.9 against 15.5).

Worth recording because I wrote in yesterday's entry that a check passing 2% of
the time is worse than none, and then shipped a check that fails under
`cargo test --workspace` on a busy runner. It passed CI once, which is exactly
how this class of thing gets through.

## What this does not do

**403 is still 4.7× the data, and the biggest remaining piece is not slack.**
128 bytes a row is four `Bytes` structs inline in the map's nodes. Getting
under that means changing the representation, and `Bytes` is load-bearing:
`Arc<BTreeMap<Bytes, Bytes>>` is what makes `begin` O(1). Nothing here attempts
it.

**Nothing is attributed between 369 and 491.** The remaining ~120 bytes a row
is the snapshot `Arc`, the `RecordStore`, the statistics, and the error in my
reconstruction of the writer's capacity guess. It was not chased.

**One sample, one schema.** Eleven columns, one secondary index, one string
column. A table of ten `i64`s would have a different ratio and nothing here
predicts it.

**The ceiling in the test is a ceiling, not a target.** It will not notice a
regression from 403 to 449. A tighter bound would fail on a schema change; this
one is set to catch the class of mistake that produced it.

**The example's own timing is not a benchmark.** Under `--features dhat-heap`
it reports 27k rows/s against 338k without, because the counting allocator
dominates. The table above comes from runs with the feature off, and the
example says so.
