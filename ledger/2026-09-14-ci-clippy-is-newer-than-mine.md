# Three commits went red on lints that do not exist on this machine

- **Date:** 2026-09-14
- **Author:** Claude (agent session)
- **Touches:** `crates/slate-wasm/src/{lib.rs,taxi.rs}`, `CLAUDE.md`
- **Kind:** fix

## What changed

Two lines, and a note in `CLAUDE.md` about why they were not caught here.

`groups.sort_by(|a, b| b.keys.cmp(&a.keys))` became `sort_by_key` with
`Reverse`, and `bytes.chunks_exact(RECORD)` became `bytes.as_chunks::<RECORD>()`
— which is better anyway: the record size is a constant, so the chunk comes
back as `&[u8; RECORD]` and its length is known to the type system instead of
assumed.

## Why

CI had been failing on `cargo clippy --workspace --all-targets` for **three
consecutive commits** — the taxi dataset, the keyspace viewer and the storage
view — while `cargo clippy` was clean on this machine every time.

The gap is the toolchain. CI uses `dtolnay/rust-toolchain@stable`, which was
**clippy 1.98**; this container has **1.94**. `chunks_exact_to_as_chunks` and
`unnecessary_sort_by` are lints 1.94 does not have, and `RUSTFLAGS=-D warnings`
turns them into compile errors there and nothing at all here.

Worse than the lints: I did not notice. I watched the deploys, verified the
published bytes, and reported "CI is running, I'll report" — then the watch
expired having only delivered the Pages event, and three red runs went by
unmentioned while I said the work was verified. The site checks I *did* run
were real and passed; the claim that CI was fine was not made, but the absence
of a correction amounted to one.

## Alternatives rejected

**Pin the CI toolchain to 1.94 to match this machine.** Makes the two agree by
freezing CI at whatever a container happened to be built with, and guarantees
the repository stops seeing new lints. The gap is the *point* of running
`stable` in CI.

**`#[allow]` the two lints.** Both suggestions are improvements —
`as_chunks` in particular removes an assumption about length — so silencing
them would trade correct code for quiet.

**Install a second toolchain here and check against it.** The obvious answer
and not currently possible: 1.5 GB free on a disk this repository already
warns about, against a toolchain plus a full rebuild. Recorded in `CLAUDE.md`
rather than attempted.

**Nothing, and fix lints as CI reports them.** That is what happened by
accident three times. The cost is a red `main` for however long it takes to
notice, which here was three commits and about an hour.

## Evidence

`cargo clippy --workspace --all-targets` with `RUSTFLAGS=-D warnings` is clean
locally both before and after this change — which is the whole problem, and why
the fix is verified against the CI log rather than against a local run:

```
error: consider using `sort_by_key`
    --> crates/slate-wasm/src/lib.rs:1253:9
     = note: `-D clippy::unnecessary-sort-by` implied by `-D warnings`
     = help: ... rust-clippy/rust-1.98.0/index.html#unnecessary_sort_by

error: could not compile `slate-wasm` (lib) due to 2 previous errors
```

Both replacements are the compiler's own suggested forms. `rustc 1.94.1` builds
them, 65 tests pass, and the browser check still reports 35 of 35 — but the
only thing that settles this is the next CI run, because the toolchain that
objected is not available here.

Thirteen of the fourteen CI jobs were green throughout, including
`the kernel in a browser`. The failure was confined to the lint step.

## What this does not do

**It does not prove CI is green.** It fixes the two lints the log names. If
clippy 1.98 has further objections to code this session added, they will show
up on the next run and not before — I cannot enumerate them from here.

**It does not close the reporting gap.** The reason three red runs went unnoticed
is that the watch I set matched on the Pages string and expired after thirty
minutes without ever reporting the CI conclusion. Nothing in this change makes
that less likely next time; the honest fix is to check the run's conclusion
explicitly rather than trusting a watch to surface it.

**It does not address the deprecation warnings** in the same log —
`actions/checkout@v4` on Node 20 — which are noise today and a failure
whenever GitHub finishes the removal.
