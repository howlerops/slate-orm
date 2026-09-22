# 82 recorded measurement tables in this repository; two record the cargo features they were taken under, and both are the write-up of the defect that cost nine tasks. Now every measuring program prints its build, and a guard fails if one stops.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #280 (F5z)
- **Touches:** `crates/slate-kernel/{build.rs,src/build.rs,src/lib.rs}`, `crates/slate-slatedb/{build.rs,src/stamp.rs,src/lib.rs,Cargo.toml}`, `crates/slate-tuple/Cargo.toml`, 15 examples and 2 benches across four crates, `crates/slate-serverd/src/main.rs`, `scripts/{check_build_stamp.py,test_check_build_stamp.py,check.sh}`, `.github/workflows/ci.yml`, `docs/performance.md`, `site/data/make-trips.py`
- **Kind:** the general defect behind #278 and #279, closed for new measurements

## What changed

A program that measures something prints one line before its first number:

```
build: slate-slatedb 0.0.1 | features aws, cache | off dhat-heap | release (opt-level 3, debug false) | x86_64-unknown-linux-gnu | slatedb 0.16.0
```

Seventeen programs print it — every example, both criterion benches, the
ClickBench runner, and `slate-serverd` on stderr. `check_build_stamp.py` fails
if a cargo feature is added without the stamp reporting it, or if any program
under `crates/` neither prints the line nor is recorded as measuring nothing.

Two conditions shout rather than inform: a **debug** build, and `cache` off.

## Why

#278 answered why `POINT_READ_COST` was 3.0 — SlateDB's block cache compiled
out — and #279 found the guard from #278 had been widened along the wrong
axis. Both entries closed with the same open item: **no measurement in this
repository records the build that produced it.** Everything here is one
instance of that.

Two surveys, run before writing anything:

- **`docs/` and `site/`**: ~82 recorded tables and fenced output blocks. About
  46 carry some provenance — fixture size, run count, machine load, cold
  versus warm. **Six** state release versus debug. **Two** state cargo
  features, and both are the write-up of finding 8 itself. **None** records a
  commit, though `docs/clickbench.md` praises another project for recording a
  binary's SHA256 with every result.
- **`crates/`**: 26 programs produce measurements. **None** printed any build
  fact. Every header is hand-rolled; there was no shared banner to add one to.

Three things that survey turned up are worth more than the count:

1. **`scripts/run_examples.sh` builds and runs `debug` binaries**, while every
   example's doc comment and every command in `docs/performance.md` say
   `cargo run --release`. Both produce a plausible table, and nothing in
   either transcript said which you were reading.
2. **`cache_probe`'s whole question is "does this build have a block cache?"**
   — a cargo feature — and it answers by *measuring*, because there was no way
   to just report it.
3. **`row_footprint` prints `0 B` for every allocation stage** without
   `--features dhat-heap`: a full table of zeroes indistinguishable from a
   program that allocates nothing.

## Alternatives rejected

**A convention in `CLAUDE.md` and nothing else.** Free, and it is what the
last three tasks amounted to. The 3.0 figure survived nine tasks of people who
would all have agreed with the convention. A line a program prints is
recorded by the act of running it; a line a person is supposed to add is the
thing that did not happen 80 times out of 82.

**`vergen` or `built`.** Both want a `.git` and a build-dependency. What was
missing is four strings cargo already sets in every build script's
environment, and a dependency to read those would cost every consumer of the
kernel a compile. Rejected for scope, not for quality.

**A `build.rs` in each crate that measures something.** Four copies of six
lines. A profile belongs to the *cargo invocation*, not to a crate, so the
generic half lives in `slate-kernel` — which everything above it already
depends on — and `slate-slatedb` adds only what is genuinely its own: its
feature set and its resolved `slatedb`. `slate-tuple` sits *below* the kernel
and reaches the stamp through a **dev-dependency cycle**, which cargo permits
and which nothing shipped carries; the alternative was that fourth build
script in the workspace's bottom crate.

**Put the stamp in `slate-tuple` so everything can reach it.** One build
script, no cycle — and build identity declared by a tuple codec. Rejected on
where a reader would look for it.

**A roster of the programs that must print it.** Anything added later is
silently absent, which is how `run_examples.sh` came to have unchecked floors
(#274). Inverted: *every* example, bench and binary must print the line or
appear in `NOT_A_MEASUREMENT` with a reason, so a new benchmark fails the
check until somebody decides which it is.

**Record the commit too.** Every measurement here is written up in a `ledger/`
entry committed with its change, so the code version is already recoverable,
and reading `git` in a build script breaks release tarballs. Stated in the
module rather than left for the next reader to wonder about.

**Retro-fit provenance onto the 82 existing tables.** Not possible and not
honest: the build behind a table from three weeks ago is not recoverable, and
inventing a plausible one is worse than the gap. `docs/performance.md` now
says which tables predate the stamp and what *is* known about them.

## Evidence

**The stamp, from a real run** rather than from reading the code —
`flatten_cost`, built the way `run_examples.sh` builds it:

```
build: debug (opt-level 0, debug true) | x86_64-unknown-linux-gnu
!! this is a debug build: wall-clock numbers below are not comparable with
   anything recorded in docs/, which is measured at --release.
grouped join with a computed column over 500 rows: min 6.6 ms, median 6.7 ms
```

That is finding 1 above, caught at the only moment it matters. `scan_tuning`
prints the full slatedb line, features and resolved `slatedb 0.16.0` included.

**Verified rather than asserted, on the way past:** #278's entry claimed 1.0
"is right for the build that ships". `default = ["aws", "cache"]`, and
`cargo tree -p slate-serverd -e features` shows `slatedb feature "foyer"` ←
`slate-slatedb feature "cache"` ← `default`. The claim holds; it had not been
checked. A unit test now fails if that default ever loses `cache`, because the
planner constant depends on it.

**Fifteen mutations, fifteen caught**, each by a named test: seven on the
guard (each half of the feature-set comparison, the `default` exclusion, the
`announce_to` spelling, the unstamped-program arm, the stale-exemption arm),
five on the Rust (the profile predicate, the target in the line, the honesty
of the `cache` bit, the lock-read version, the load-bearing filter) and its
inverse — `.filter(|_| true)`, which is caught too, so the filter is a filter
rather than a pass-through.

**One survivor, and it was the important one.** *A load-bearing feature that
is off stops warning* survived: my test had **re-implemented** the filter over
`LOAD_BEARING` instead of calling it, because a test binary is one build and
this one has `cache` on, so the cacheless arm is unreachable from inside it.
The test was checking its own copy — #212's finding, in new code, found the
same way. `missing(&["cache"])` is now a function the test and the stamp
share.

**A design flaw found by running it rather than reading it.** With `dhat-heap`
in `LOAD_BEARING`, `scan_tuning` printed a warning about `row_footprint`'s
allocation columns above a table of scan timings. A warning that fires where
it cannot apply teaches a reader to skip the line — and the line it teaches
them to skip is the `cache` one. `LOAD_BEARING` is now for features whose
absence changes what *any* measurement means; `row_footprint` says its own
piece next to its own zeroes.

**Two stale figures, found by the survey and fixed:**

- `docs/performance.md` still said *"a point read costs about 3"* in
  present-tense guidance on how to read the whole page. Stale since #269. It
  survived #277's eight-file sweep and #279's widening because
  `check_cost_prose.py` reads `crates/` and not `docs/`.
- `site/data/make-trips.py` still justified its 100,000-row sample with *"1.3
  KB a row"* and *"~130 MB"*, both withdrawn by §7c in favour of **403 bytes a
  row**. The corrected figure changes the argument — the month is ~1.2 GB, not
  ~3.8 GB, against a tab's ~2 GB — and **the sample size was not revisited**,
  which the comment now says rather than implying the old reasoning still
  holds.

`sh scripts/check.sh`: **44 passed, all of them** (42 before; two new steps).
`cargo clippy --workspace --all-targets`: clean, after two `indexing_slicing`
warnings in the new tests that CI's `-D warnings` would have failed on.
`python3 scripts/test_check_build_stamp.py`: 13 passed, 0 failed.
`sh scripts/run_examples.sh slate-kernel --smoke`: 4 passed, 0 failed.
Unit tests: kernel 9, slate-slatedb 12, slate-tuple 2, all passing.

## What this does not do

**It does not stamp a single existing table.** Everything recorded before
today is still a number whose build is unknown, and that cannot be fixed
retroactively. The convention applies from here.

**It does not record the commit, the machine or the wall-clock date.** Three
of the four things a reader might want; the ledger carries the first, the
harnesses that care print the second, and nothing prints the third.

**`docs/` prose is still unguarded.** The stale "costs about 3" above is
evidence that this costs something. Widening `check_cost_prose.py` to `docs/`
is a task of its own — `correctness.md` narrates these numbers historically at
length, and #279 is a fresh lesson in what a hasty widening does.

**The stamp's `slatedb` version is the only dependency version it carries.**
`foyer`, `object_store` and `tokio` all move under a measurement too. One was
chosen because one was the suspect; the rest is a lock file nobody reads.

**Nothing checks that a *recorded* table carries the line.** A guard could
require a `build:` line near each table in `docs/`; this one only makes sure
the programs emit it. Somebody pasting a table without its header is
unguarded, which is the same shape as the defect one level up.

**`clients/python/testserver` is outside the swept tree**, as is anything
measuring under `examples/` (`batchbench` prints its TSV header from the
shell). The guard walks `crates/` only, and says so.
