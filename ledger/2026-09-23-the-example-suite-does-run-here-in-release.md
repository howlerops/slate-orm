# I recorded an unverifiable mutation because "nine debug example binaries exhaust the disk". They do. The same nine are 131 MB at `--release`, and the script already had both knobs needed to run them.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #298 (F6r)
- **Touches:** `CLAUDE.md`
- **Kind:** correcting an exemption I wrote three hours ago, by doing the thing it said could not be done

## Supersedes

[`ledger/mutations/20260923T132547-crates-slate-slatedb-examples-loader-cliff-rs.json`](mutations/20260923T132547-crates-slate-slatedb-examples-loader-cliff-rs.json)
records an `expect_survivor` whose stated reason — "on this container it cannot
be built: nine debug example binaries exhaust the disk" — is false as written.
The record stays as run, per `ledger/README.md`; this entry is the correction it
links forward to. The claim is true only of the default profile.

## What changed

The doc comment on `assert_wrote_something` in `loader_cliff.rs` said the same
thing in the source itself, ending "a guard whose only witness is a suite you
cannot run is a guard nobody has seen fire". It now says which profile, and
names the mutation that shows the guard firing. `run_examples.sh` called these
"eight binaries" in two comments; there are nine.

`CLAUDE.md` now carries the recipe:

```sh
CARGO_INCREMENTAL=0 cargo build --release -p slate-slatedb --examples
SKIP_BUILD=1 BINARIES_DIR=$PWD/target/release/examples \
    sh scripts/run_examples.sh slate-slatedb --smoke
```

**9 passed, 0 failed**, `loader_cliff` among them.

The debug build really does not fit — three attempts today, the last with `df`
reporting **8.0K free** and `LLVM ERROR: IO failure on output stream`. What was
wrong is the conclusion I drew from that. Debug carries `-C debuginfo=2`; the
same nine examples are **131 MB** at `--release`. And `SKIP_BUILD` and
`BINARIES_DIR` have been in the script since #274, for its own tests.

## Why

#293 added an empty-store assertion to `loader_cliff` whose only witness is
this suite, and recorded the mutation as `expect_survivor` with the reason
"the runner that does reach it cannot be built on this container". That reason
is now known to be false as stated: the *default profile* cannot be built here.
The suite can.

An `expect_survivor` is a claim that something cannot be checked, written into
a record other people will trust. Getting one wrong is worse than a missing
test, because it closes the question.

## Alternatives rejected

**Add a profile flag to `run_examples.sh`.** Drafted, then abandoned on reading
the script: `SKIP_BUILD` and `BINARIES_DIR` already do it, and a third knob
overlapping two existing ones is how a script grows a way of doing everything
twice. Reading the file I was about to edit is what caught it.

**Leave the exemption and note the correction in passing.** The record is the
thing people read; the entry beside it is not. Running the mutation for real is
the only thing that replaces a claim about what cannot be run.

**Make CI use release too.** CI has room and the debug profile catches
`debug_assertions` that release compiles out. Nothing here is a reason to
change what CI does, only what this container can do.

## Evidence

The decisive one: breaking the directory walk so a real store reads as empty is
**caught** — `loader_cliff, exit 101` — in
[`ledger/mutations/20260923T150044-crates-slate-slatedb-examples-loader-cliff-rs.json`](mutations/20260923T150044-crates-slate-slatedb-examples-loader-cliff-rs.json).
The assertion is wired up and does fire through the smoke suite, which is what
#293 could not show.

Deleting the *call* still survives, in that record and in
[`20260923T150403`](mutations/20260923T150403-crates-slate-slatedb-examples-loader-cliff-rs.json),
and the real reason is better than the one I gave: at 2,000 rows the store is
healthy, so an assertion that would have passed is invisible whether it runs or
not. That is true of every passing assertion in every suite, and it is not a
missing test.

Also run locally, since CI is unavailable: `cargo test -p slate-slatedb --test
s3` (16 passed), `cargo test -p slate-orm --no-fail-fast` (122 across 18
targets), `cargo test -p slate-derive` (4), `cargo test -p slate-wasm --test
timing` (6, three times). `sh scripts/check.sh`: **48 passed, all of them.**

## What this does not do

**It does not make the whole suite runnable.** `slate-headbench`,
`slate-kernel` and `slate-orm` also have examples, and only `slate-slatedb`'s
nine were built and run this way. The other three crates' floors are 5, 4 and
1; whether those fit in debug here is untested.

**It does not close the gap it was meant to.** The call site remains
unmutatable-in-the-useful-sense for the reason above, so "does `main` still
call the assertion" has no witness. A test that loads zero rows would give it
one, and would also be a test asserting that an example fails, which is a
different thing to maintain.

**131 MB and 1.4 GB are one measurement each**, on one crate, on this
container, today. The ratio is the point, not the numbers.
