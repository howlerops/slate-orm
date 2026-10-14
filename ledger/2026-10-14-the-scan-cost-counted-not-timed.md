# The 0.17 scan cost, counted in instructions: 7% was the clock, the work grew 1–3%, and a per-row tracing span is the part that can be named

- **Date:** 2026-10-14
- **Author:** Claude Code (session: "Let's do the last one", meaning profile the 7%)
- **Touches:** `crates/slate-headbench/examples/scan_count.rs` (new),
  `scripts/run_examples.sh` (the headbench floor, 5 → 6),
  `docs/performance.md` (§8f corrected, §8g new),
  `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** performance

## What changed

§8f had recorded a full scan as about 7% slower on SlateDB 0.17. Its caveat
said that was located but not explained. It is now explained as far as this
machine allows, and part of the original claim is withdrawn:
- §8f carries a correction at its top: the 7% was the clock, and it is
  withdrawn.
- §8g records the measurement that replaced it.
- `scan_count`, the instrument behind §8g, is a committed example. It joins
  `slate-headbench`'s five, and `run_examples.sh`'s floor follows it to six.

## Why

The owner asked for the last open item, the unprofiled 7%. Re-measuring
first showed the number could not be trusted. A day later, at load 15–19, the
same two binaries gave 23.0 against 23.2 ms, and interleaved runs gave
±300% spreads. On a machine with performance and efficiency cores, shared
with other work, a scan's speed depends on where it is scheduled. Profiling a
difference that might not exist would have explained noise.

Instructions retired (`/usr/bin/time -l`) count the work regardless of load.
They showed the work did grow, by 3.3% between the registry builds that
actually ship. That made it worth attributing, and it turned out to be about
half the clock's claim.

## Alternatives rejected

- **Profile the 7% straight away.** The profile would have found where time
  goes, but not whether there was any more of it. Five runs on the clock had
  the two versions overlapping. The first real job was to find a measure the
  machine's load could not move.
- **Quiet the machine.** The load came from other people's work: a container
  VM, a browser, another project's agent. None of it was this session's to
  stop. Counting instructions makes quiet unnecessary.
- **Trust the profile's per-function differences.** After the span, no
  function moved by more than ~0.6% of samples, against sampling noise of
  about ±0.15% per function. The inclusive view was dominated by call paths
  that had moved between versions without getting slower: `read_blocks` rose
  from 0.6% to 2.1% while its own source is identical in both versions. Hence
  the counterfactual and the bisect, which measure work directly.
- **Bisect all 39 commits.** 14 points, chosen around every commit whose
  title touches the read path, then 5 runs at each boundary. More points
  would not help, because the steps left are smaller than the difference
  between two builds of identical source.
- **Patch SlateDB in this repository to drop the span.** A `[patch]` on a
  fork, for about 1% of a scan's CPU over an in-memory store, would be a fork
  to maintain for a cost that a network round trip dwarfs. The fix belongs
  upstream: skip `.instrument()` when the span is disabled.

## Evidence

All counts are instructions per full scan of 20,000 rows, from `scan_count`:
the difference between `SCANS=200` and `SCANS=0`, divided by 200.

- **Registry builds:** 0.16 is 328.1M–331.2M (in its own tree) and
  329.3M–333.8M (in 0.17's tree, with 0.17's lockfile). 0.17 is
  340.1M–341.7M.
- **Dependencies ruled out:** 0.16 with foyer 0.22.6 and object_store 0.14.2
  is 329.6M–331.0M.
- **Counterfactual:** 0.17 with `DbIterator::next_entry`'s two `.instrument()`
  calls removed is 337.0M–338.7M. The build used a private copy of the
  registry source, linked via `[patch]` in a throwaway worktree, and
  `cargo tree` and the binary's strings confirmed the copy was the one
  linked.
- **Profile:** Instruments' Time Profiler, 300 and then 1,500 scans a side,
  symbolicated with `atos` against `dsymutil` output. The tracing
  `Instrumented` and `Span` drops appear only in 0.17's samples.
- **Bisect:** SlateDB built from git at 14 first-parent commits, all built
  identically. The lowest mode at each point:
  - v0.16.0: 334.0M
  - #8: 330.6M
  - #9, the tracing commit #2048: 334.0M, so +3.4M
  - #18, range filters #2076: 334.8M, against 334.9M before it
  - #26, the descending fix #2067: 336.2M, against 337.7M before it
  - #38: 335.9M
- **Build effect:** registry 0.17.0 is byte-identical to git v0.17.0, and #38
  differs from it only in its version strings. Yet #38 built from a path
  costs 335.9M–338.8M, and 0.17.0 from the registry costs 340.2M.
- **Wall clock, both days:** recorded in §8f and §8g.
- `scan_count` gained `slate_slatedb::announce()` after these counts were
  taken, because `check_build_stamp.py` requires a measuring program to print
  what built it. It runs once per process, in both the `SCANS=0` and the
  `SCANS=200` run, so it cancels out of the difference.
- `scripts/run_examples.sh slate-headbench --smoke`: 6 passed, 0 failed.
  This used a stand-in `timeout` on the PATH, because macOS has no GNU
  `timeout`; CI has the real one. `check_examples_roster.py` reports every
  floor exact.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- **The new witness:** renaming §8g's heading was caught by
  `check_closed_caveats: FAIL every witness is still in the tree`.

## What this does not do

**About two thirds of the gap is not attributed.** Between registry builds
it is 3.3%; built identically from git, the code moved only 0.6%. The
difference comes from how identical source compiles, which this measurement
cannot take apart. §8g says so rather than share it out among commits.

**`scan_count` was not mutation-tested.** It is an instrument whose output a
person reads. Its one assertion, that a scan returns every row, is the same
check `cache_probe` makes.

**Nothing was reported upstream.** The per-row span is upstream's to change.
Opening an issue on SlateDB's repository is an outward-facing action, and
whether to take it is the owner's call.

**aarch64 only.** The release binaries and the image are x86_64 and aarch64
Linux, and neither was counted.
