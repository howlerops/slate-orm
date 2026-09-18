# Untrack the benchmark's build output

- **Date:** 2026-09-18
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `.gitignore`, `examples/batchbench/{go,node}` (untracked, not deleted)
- **Kind:** fix

## What changed

The previous commit added `examples/batchbench` and, with it, 253 files of
build output: `node/node_modules/` (half a million lines), `node/dist/`, and
`go/batchbench`, the binary `go build` writes when nobody passes `-o`. All
three are now ignored and untracked. Nothing on disk is deleted; the benchmark
still runs.

## Why

`.gitignore` names build directories **per directory** — `examples/explorer/backends/node/node_modules/`,
`examples/deployed/node/node_modules/`, and so on — so a new example matches
none of them and its output is tracked by default. The comment above those
lines says exactly why this happens:

> a nested build directory escapes a root-anchored rule, so name each one.

It was right, and it was not enough: naming each one means every new example
must remember to, and the failure is silent. `git add -A` in a directory whose
rules do not exist yet stages everything.

## Alternatives rejected

**A blanket `**/node_modules/` and `**/dist/`.** The obvious fix, and it would
have prevented this. Rejected because it also hides a `node_modules` or a
`dist` somebody genuinely meant to vendor, and the repository would no longer
be able to say so — the per-directory style is what makes "this one is
deliberate" expressible. The cost of that choice is this failure mode, which is
now written down beside the rules rather than left to be rediscovered.

**A pre-commit check that refuses a staged `node_modules` path.** The right
answer if this happens again, and named in the `.gitignore` comment as such.
Not done now because the hook is deliberately dependency-free `sh` and a third
occurrence is the evidence that would justify adding to it — one occurrence is
a mistake, two is a pattern, and this is the first.

**Rewriting the previous commit.** Rejected: the branch is pushed, and a
force-push to fix a file that is merely unwanted rather than harmful trades a
clean history for a broken checkout on anybody who has pulled. The deletion
commit is the honest record of what happened.

## Evidence

`git status` is clean afterwards and `./run.sh --rows 20 --runs 2` still
produces its table, which is the only thing that proves the untracked files
were genuinely build output rather than something the benchmark needed.

The three paths are also the three shapes the two existing examples already
ignore, which is the tell that this was a missing line rather than a new
problem: npm's tree, tsc's output, and a stray `go build` binary.

## What this does not do

**The repository still has the artifacts in its history**, in the previous
commit. They are unreachable from the working tree but they are in the pack,
so a clone still downloads them. Removing them needs a history rewrite on a
pushed branch, which costs more than the half-megabyte is worth.

**Nothing prevents the fourth example doing this again.** The comment says
what to do if it happens a third time; it does not stop the third time.
