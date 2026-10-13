# SlateDB 0.17, which fixes a descending scan that came back out of order on compacted data, and a test that shows it did

- **Date:** 2026-10-13
- **Author:** Claude Code (session: "Let's do a final pass at ensuring we are up to date with slate db proper")
- **Touches:** the five manifests that name `slatedb` (`slate-slatedb`,
  `slate-server`, `slate-serverd`, `slate-headbench`, `examples/helpdesk`),
  `Cargo.toml` (`rust-version`), `Cargo.lock`,
  `crates/slate-headbench/examples/cache_probe.rs`,
  `crates/slate-slatedb/tests/descending.rs`
- **Kind:** fix

## What changed

Every crate now asks for `slatedb = "0.17"`, which resolved to 0.17.0, released
2026-09-29. The workspace was on 0.16.0. It brings foyer 0.22.3 → 0.22.6 and
object_store 0.14.1 → 0.14.2. Foyer 0.22.6 declares Rust 1.91, so the
workspace's `rust-version` goes from 1.90 to 1.91, which is what cargo's
resolver needed to pick it.

One API changed under us. `DbBuilder::with_db_cache` now takes a
`db_cache_id`, so that one cache can serve several databases. Only the
`cache_probe` benchmark example calls it, and it gives every arm a fresh cache
over one database, so it passes a constant. The `DbCache` trait gained
`fetch_*` methods that report hit or miss. They have default implementations
built on the `get_*` methods the probe overrides, so its counters still see
every lookup.

A new test, `tests/descending.rs`, fills a database with small SSTs, waits
for the compactor to produce a sorted run of more than one SST, and scans it
in descending order through `SlateStore`. It reads once through a transaction
(the path the record layer's queries take) and once through a snapshot (the
path a replica serves).

## Why

**On 0.16, a descending scan over compacted data returned keys out of order.**
Inside a sorted run that spans several SSTs, SlateDB walked each SST
backwards but walked the SSTs forwards (slatedb#2067, fixed in 0.17). This
crate maps `ScanOrder::Descending` straight onto SlateDB's, and the kernel
issues one for `.order(ScanOrder::Descending)` and for a `Desc` order that an
index can serve. Every client reaches that through the wire `ScanOrder`. So a
descending query over a table large enough to have been compacted could come
back in the wrong order, with no error.

Nothing here caught it. Every existing descending scan in the
`slate-slatedb` suite reads five rows from a memtable: no compaction, so no
sorted run, so the broken path never ran.

The owner asked for the version to be current. A dependency bump argued from
release notes is a hypothesis, so this one carries the test that fails
without it.

## Alternatives rejected

- **Stay on 0.16 and work around the bug in `SlateStore`.** For example,
  scan ascending and reverse in memory. That turns a streaming iterator into a
  buffered one, and every `LIMIT` over a descending index into a whole-range
  read. The fix exists upstream, so a workaround would cost all of that to
  avoid a minor-version bump.
- **Keep `rust-version = "1.90"` by pinning foyer to 0.22.3.** slatedb 0.17
  requires the newer foyer, so the only way to keep 1.90 is to stay on
  slatedb 0.16, which is the option above. And nothing builds on 1.90
  anyway: CI uses current stable, the `Dockerfile` uses `rust:1-bookworm`,
  and this machine has 1.94.1. The declared number is what the resolver
  reads, and 1.90 would have made it refuse the upgrade.
- **Test the fix against `slatedb` directly rather than through
  `SlateStore`.** That would show SlateDB's bug, which upstream's own test
  already does. What needed showing is what this crate's callers see. Both
  read paths are covered, because a transaction and a snapshot are different
  SlateDB types.
- **Wait for L0 to be empty before scanning,** as the first draft did. The
  size-tiered scheduler leaves a final small L0 SST that has nothing its size
  to merge with, so the wait timed out at 30s with a 7-SST run already built.
  The scan merges L0 in either way, and the bug is in the run.

## Evidence

**The new test, on both versions,** three runs each:
- **0.16.0:** run in a `git worktree` of `HEAD` with only the test file
  copied in. Failed 3 of 3. The first wrong key was at position 1426, 1672
  and 1508 of 2000; every time it was `k000141` where a larger key belonged.
  The position moves because the compaction layout depends on timing.
- **0.17.0:** passed 3 of 3, in 3.1 s, 7.1 s and 7.1 s, most of which is
  waiting for the compactor.

So the bump changes what a caller sees, and the test detects that change in
both directions.

**The rest:**
- `cargo test -p slate-slatedb -p slate-server -p slate-serverd -p
  slate-helpdesk --no-fail-fast` on 0.17: 49 test binaries, 787 passed, 0
  failed. The suite includes the S3, replica, handover, restart and
  storage-crash tests.
- `cargo check --workspace --all-targets --all-features` and `cargo clippy
  --workspace --all-targets -- -D warnings`: clean. The one compile error was
  the `with_db_cache` signature above.
- `scripts/check.sh`: 98 of 99. The failure is `example-runner-guard`, which
  needs GNU `timeout` and fails here before and after this change.
- `examples/explorer/run.sh --conformance`, with the head node rebuilt on
  0.17: "147 cases: the 4 adapters agree on all of them", 147 passed, 0
  failed.

## What this does not do

**v0.2.0 still ships SlateDB 0.16, and so does the live explorer.** The
published binaries, the GHCR image and the Cloudflare head nodes were all
built before this change. They carry the descending-scan bug until the next
release, and a tag is the owner's decision. The live explorer's tables are
small enough that they have probably never been compacted. That is a guess,
not something checked.

**`rust-version = "1.91"` is declared, not tested.** Nothing builds the
workspace on 1.91 exactly, just as nothing built it on 1.90. The number is
right in the sense that matters to cargo, which is resolution, and it is not
a promise that 1.91 compiles the tree.

**The rest of 0.17 is unmeasured here.** Upstream's notes include changes
that bear on numbers in `docs/performance.md`: point lookups probe sources
concurrently, range scans use filters, `CachedObjectStore` was rewritten, and
L0 flush streams. None was benchmarked. Those tables carry a `build:` line
naming `slatedb 0.16.0`, so they read as measurements of that version and are
not wrong, but they are no longer measurements of the current tree.
