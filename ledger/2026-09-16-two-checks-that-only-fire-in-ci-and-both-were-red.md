# The branch went red on two checks that only fire in CI, and the local run said nothing

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-server/src/service.rs` (a test-module lint allowance)
- **Kind:** fix

## What changed

`#![allow(clippy::expect_used)]` on the `#[cfg(test)] mod tests` added to
`service.rs` with the `Related` handler. One line.

The other half of the same failure — a vendored `records.proto` that had
drifted — is fixed in the TypeScript commit, where it belongs.

## Why

CI run 101, on `28de1ce`, failed two of sixteen jobs:

- **clippy, test.** The workspace bans `expect` outside tests, and
  `RUSTFLAGS=-D warnings` makes it fatal. The new unit tests on `distinct_keys`
  used it. The convention is already in `fingerprint.rs`, three files away, as
  an inner attribute on the test module; this one did not have it. Because
  clippy is step 5 of that job, `cargo test --workspace` and four other steps
  were **skipped** — the whole Rust job is one failure away from running
  nothing.
- **TypeScript client.** `test/proto.test.ts` compares the bundled proto to
  `crates/slate-server/proto` byte for byte. Adding an RPC to the server made
  them differ, and the client resolves its methods from the bundled file at
  runtime, so every test failed with *"this server has no Related method"*.

Neither surfaced locally, and the reason is the same in both cases: **the
commit that broke each of them did not run the check that catches it.** The
wire commit ran the Python suite and the kernel tests, which is where its
changes were, and clippy is a whole-workspace command whose failure was in a
crate that commit did touch. So this is not "a check that never fires" — it is
a check that fires only in CI because nothing ran it here.

## Alternatives rejected

**Return `Result` from the unit tests** rather than allowing `expect`. It is
the lint's intended answer and it is what the rest of this codebase does not
do: `fingerprint.rs` allows both `expect` and `unwrap` in its tests, on the
reading that a panic in a test *is* the failure report. Matching the
neighbouring file beats being locally more correct and globally inconsistent.

**Add clippy to the pre-commit hook**, so this cannot recur. Rejected for the
same reason `cargo fmt` was rejected there two commits ago: the hook is
deliberately dependency-free, runs in a fraction of a second, and a
`command -v cargo` guard would make it skip silently for anyone without a Rust
toolchain — "a skip is green", which this repository has already been bitten by.

**Add the proto copy to the same guard.** `scripts/check_workspace.py` is the
place a cheap structural check like that would live, and copying one file is
already checked by a test that runs in CI. The thing that failed was a habit,
not a missing check.

## Evidence

Before: `cargo clippy --workspace --all-targets` with `RUSTFLAGS=-D warnings`
reports three `expect_used` errors in `crates/slate-server/src/service.rs` at
lines 1852, 1861 and 1870, and `could not compile slate-server (lib test)`.

After: clippy is clean across the workspace, `cargo fmt --all -- --check` is
clean, and `cargo test -p slate-server --no-fail-fast` passes 135 tests across
ten binaries — including the three that could not compile.

CI run 101's job list, for the record: fourteen green, `clippy, test` failed at
step 5 with five steps skipped, `TypeScript client` failed at `npm test`.
`three SDKs, one database` was green, because the node adapter did not call
`Related` yet and so never resolved the missing method.

## What this does not do

Nothing stops the next new test module from repeating it. The lint is
workspace-wide and the allowance is per-module by design, so every test module
that wants `expect` has to say so — which is the lint working, not a gap.

It does not make the local toolchain match CI's. `dtolnay/rust-toolchain@stable`
is newer than this container's, and a lint that exists there and not here is
still invisible until it is pushed. That is recorded in `CLAUDE.md` already and
is not addressed here.
