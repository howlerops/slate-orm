# The formatting job was red for two commits

- **Date:** 2026-09-17
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-server/tests/returning_cap.rs`
- **Kind:** fix

## What changed

Six lines of `rustfmt` output in one test file. No semantics: the same nine
tests pass before and after, and the diff is entirely where a struct literal
wraps inside a nested call.

## Why

CI run 113 failed one job — `cargo fmt --all -- --check` — on the commit that
added that file, and nobody looked. Run 114 was queued against the same
unformatted file, so it was going to fail identically for a second time, which
is the part that makes this worth an entry rather than a quiet fix.

The failure is exactly the one `CLAUDE.md` warns about in its own words: *"Use
`cargo fmt -p <your-crates>`, and actually run it: CI checks `--all`, and a
session that formatted nothing turned that job red on nothing but line
breaks."* `cargo fmt -p slate-server` **was** run. It was run before the test
file had finished being written, and nothing re-ran it afterwards. Knowing the
rule and having a note about it is not the same as having a check, and the
two commits in between were reported here as green on the strength of the
local suites rather than on CI.

The immediate lesson is about ordering, not about knowing: `cargo fmt --all --
--check` is read-only, takes about four seconds, and belongs *after* the last
edit rather than in the middle of the work. That is now how this branch
finishes a commit.

## Alternatives rejected

**Put `cargo fmt --all -- --check` in the pre-commit hook.** The obvious
structural fix, and the hook refuses it on its own terms. Its header says why:
*"Deliberately a `sh` script with no dependencies: a hook that needs a
toolchain to run is a hook that stops running the first time somebody commits
from a container that does not have it."* A formatting check needs `cargo` and
a `rustfmt` component. Adding it would trade a job that goes red — visibly,
with a diff — for a hook that either silently skips the check wherever cargo is
absent or blocks commits in containers that were never meant to build Rust.
Neither is better than reading CI.

**Amend the two commits so history is clean.** Both are pushed to a branch that
other sessions fetch, and rewriting shared history to hide a four-second
formatting slip is a much larger risk than the slip. A forward fix leaves the
failure legible, which is the point of the ledger.

**Run `cargo fmt --all` rather than `-p slate-server`.** Same output here,
since the check named exactly one file and it is mine. Still `-p`: `--all`
reformats files other sessions have open, and `CLAUDE.md` asks for `-p`
specifically because that has bitten this repository before. The read-only
`--check` is the `--all` form that is safe to run, and it is the one that now
runs last.

## Evidence

Before: `cargo fmt --all -- --check` reports one file, `returning_cap.rs:346`.
After: it reports nothing and exits 0. `cargo test -p slate-server --test
returning_cap` passes nine tests, the same nine as before, so the reflow
changed no behaviour.

The failing job is run 113's `formatting`, step `cargo fmt --all -- --check`,
the only one of that run's sixteen jobs that did not succeed — every other job,
including `clippy, test`, the three client suites, conformance and the deployed
stack, was green on the same commit.

## What this does not do

**Nothing prevents the next one.** The hook is the wrong place, for the reason
above, and no other guard was added: this rests on `cargo fmt --all -- --check`
being run last, by a person who remembers to. That is a weaker guarantee than
this repository usually accepts, and it is stated plainly rather than dressed
up — the fallback is that CI catches it in four seconds and somebody reads the
result, which is the part that failed here.

**It says nothing about run 114.** That run was still in flight when this was
written, against the unformatted file, so its `formatting` job is expected to
have failed too. This commit is what makes the next run green, not that one.
