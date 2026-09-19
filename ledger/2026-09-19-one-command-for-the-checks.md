# One command runs every static check in CI, and a guard fails if it stops listing one

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `scripts/check.sh`, `scripts/test_check_sh.py`,
  `.github/workflows/ci.yml`, `CLAUDE.md`
- **Kind:** process

## What changed

`sh scripts/check.sh` runs nineteen checks — `cargo fmt --check`, two clippy
invocations, `cargo doc`, both `ty` runs, both `ruff` runs, `gofmt` and
`go vet` in two Go trees, three TypeScript typecheckers, the workspace guard,
the codegen tests and the pre-commit suite — from their six different working
directories, reports at the end rather than stopping at the first failure, and
prints what it deliberately does not cover. `scripts/test_check_sh.py` reads
`ci.yml` and fails unless every step there is either in the script's list or in
an `ELSEWHERE` table with a reason it cannot be. CI runs the guard.

## Why

Three commits in one afternoon went red on checks that existed, were cheap, and
were run from the wrong place or not at all:

- `ruff check .` at the repository root passed while `ruff check .` in
  `clients/python` failed. Two runs, two configurations, disjoint trees.
- `pytest tests/test_details.py` passed while the suite failed, on a change to
  a module every other test imports.
- `cargo clippy -p <one crate>` passed while `--workspace --all-targets`
  failed.

`CLAUDE.md` described all three, accurately, in prose. Prose is not a command.
The only thing a session reliably does is run something, so the fix is
something to run.

Writing it immediately found a nineteenth check nobody had noticed: CI runs
`cargo clippy -p slate-orm --features json --all-targets`, a feature-gated
invocation that `--workspace --all-targets` does not cover, and the first draft
of the script omitted it. The guard caught that, before the script had ever
been committed.

## Alternatives rejected

**A `Makefile`.** More conventional, and it buys incremental targets, which is
the wrong shape here: every check is cheap, the expensive part is remembering
they exist, and a `make` that skips a target because nothing changed is exactly
the "a check that never fires" failure this repository has met twice. A shell
script that always runs everything says what it did.

**A pre-commit hook.** The hook already exists and is already the thing agents
bypass with `--no-verify` when it blocks a recovery. Adding two minutes of
clippy to it would make bypassing it routine, and a bypassed hook is a hook
that protects nothing. This is a thing you run, not a thing that runs you.

**Stop at the first failure.** Shorter, and it wastes a session's time: fix
one, re-run, pay for clippy again to learn about the second. Running everything
costs the same as running everything once.

**Include the suites.** The tempting version, because the third failure above
was a suite and not a lint. It would need a built `slate-serverd`, a built
`slate-testserver`, three `npm ci`s and a browser, take twenty minutes, and be
run approximately never. The line is drawn at "needs no binary, no browser, no
container, no network", and what falls outside it is printed at the end of
every run rather than left implicit. `CLAUDE.md` now also says, in its own
sentence, that running one file of a suite is not running the suite.

**A guard that allowlists command prefixes** (`cargo clippy`, `ruff`, …) rather
than requiring every step to be classified. Much less to write and it has the
defect the allowlist form always has: a CI step of a *new* kind is invisible to
it. Requiring a decision per step is the `EXPECTED_REFUSALS` pattern the
conformance runner uses, and it is here for the same reason — a list you are
forced to edit is a list that stays true.

**No guard at all.** The script's value is entirely in its list being current,
and nothing else in this repository keeps a hand-maintained list current. The
demo's table list is on its fifth copy and needed a test to stop drifting.

## Evidence

`sh scripts/check.sh` is 19 passed on a clean tree, and
`python3 scripts/test_check_sh.py` reports 60 steps and 5 blocks all accounted
for. Five mutations, all caught:

| mutation | result |
| --- | --- |
| a listed check replaced with `false` | caught — reported as `FAILED: workspace-guard`, and the other eighteen still ran |
| a check removed from the script's list | caught — the guard named it |
| a new step added to `ci.yml` | caught — the guard named the command |
| a stale `ELSEWHERE` entry for a step that does not exist | caught — the guard named it |
| the script's `LIST` block renamed | caught — the guard raises rather than silently finding nothing |

The last is the one worth having: a guard that parses a file and finds nothing
reports success, which is how "check them all" tests pass while checking none.

`gofmt -l` needed a wrapper because it prints the files it would rewrite and
exits zero either way — a check on its status alone would have been permanently
green. Verified by running it against a deliberately misformatted file.

## What this does not do

**It does not run the suites, and the suites are where the defects are.** The
closing summary names the nine it skips. A session that changes a client and
runs only this has run a spell-checker.

**The guard compares strings.** `cargo clippy --workspace --all-targets` in
`ci.yml` and in the script must match character for character; reordering the
flags in one fails the guard with a confusing message. That is annoying and it
is the cost of not writing a shell-command parser, which would have its own
disagreements.

**Multi-line `run: |` blocks are matched by their `name:`.** Three exist and
none is a static check. Renaming one fails the guard, which is the right
direction, but the guard cannot see *what* such a block does — so a static
check added inside an existing block would pass unnoticed.

**Nothing runs the script for you.** It is not in the hook, for the reason
above, and CI does not run it either — CI runs the real steps. If a session
does not type it, it does not happen, and that is the same weakness the prose
had. What is different is that it is now one thing to remember instead of
nineteen.
