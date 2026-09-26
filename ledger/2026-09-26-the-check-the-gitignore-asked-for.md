# `.gitignore` wrote "if this happens a third time, the answer is a check rather than another comment." The check exists now, before the third time. A mutation showed one of its tests was asserting something no code path could get wrong.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/check_build_output.py` (new), `scripts/test_check_build_output.py` (new), `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** a guard, and batch seven of the re-triage pass

## What changed

`scripts/check_build_output.py` asserts two things: no tracked file lives under
a `node_modules/`, `dist/`, `dist-test/`, `build/` or `target/` directory, and
every tracked `package.json`'s directory has its `node_modules` and `dist`
ignored. Over this tree: **0 tracked build files, 5 package roots, all
covered.** Ten cases in `scripts/test_check_build_output.py` drive it over real
`git init` repositories. Both run in `scripts/check.sh` (now 59 checks) and in
CI. Batch seven of the reading pass is stamped alongside, nine still true.

## Why

Two places asked for exactly this, independently, and neither could act on its
own request. `.gitignore`:

> the cost is that every new example must add its own line, and nothing
> enforces that. If this happens a third time, the answer is a check rather
> than another comment.

and `ledger/2026-09-18-untrack-the-benchmarks-build-output.md`:

> **Nothing prevents the fourth example doing this again.** The comment says
> what to do if it happens a third time; it does not stop the third time.

It has happened twice: half a megabyte of `dist/` and a Go binary committed,
noticed later, untracked in a follow-up, still in the pack forever. Writing the
check before the third time rather than after is the only interesting thing
about it, and it is only possible because the caveat tracker brought the
request back.

**Two checks, because the failure has two moments.** The tracked-file check
catches the commit that does it, which is the last moment it is cheap — after
that the bytes are in the pack and removing them means rewriting a pushed
branch. The ignore-coverage check catches the moment before: a new example
whose output nothing ignores yet, merely *waiting* to be committed. A tree can
pass the first and fail the second, and that gap is precisely what the
`.gitignore` comment is about.

## Alternatives rejected

**A blanket `**/node_modules/` in `.gitignore`.** One line instead of a script,
and `.gitignore` already rejects it for a stated reason: it would also hide a
`node_modules` somebody genuinely meant to vendor. This check enforces the
choice that comment made rather than arguing with it.

**Check only that nothing is tracked.** Half the value and the easier half. The
failure it misses is the one that was actually predicted — the *next* example,
not this one — and a tree with an unignored output directory is one `git add
-A` away from the expensive failure.

**Derive the build-directory names by shape** rather than from a list. Anything
matching `^(out|build|dist|target|\.next)$`, say. It would catch `out/` and
`.next/`, which `BUILD_DIRS` does not, and it would also catch a source
directory legitimately named `build` — and this repository has no such
directory *today*, which is a fact about today. A named list that must be
edited is the roster failure mode, and it is accepted here for the reason it is
accepted elsewhere: a list you are forced to edit is a list that stays true,
and guessing at directory names by shape is not.

**Write the test against `slate-orm`.** It would assert that today's tree is
clean, which is also what a check that does nothing asserts. The cases need a
real repository, because the guard asks git what is tracked and what is
ignored — faking either would test a mock. `git init` in a temporary directory
is cheap and exercises the same `ls-files` and `check-ignore` as the real run,
including `check-ignore`'s exit-code-1-means-*not*-ignored convention, which is
the easiest thing here to get backwards and has its own named function and its
own mutation.

## Evidence

**A mutation found a test that could not fail.** Six were run; the first
survived:

```
!! the last path segment is included, so a file named dist counts: SURVIVED
```

`path.split("/")[:-1]` drops the filename before matching, and the case meant
to pin that used `app/src/dist.ts` — whose last segment is `dist.ts`, not
`dist`. Including the filename therefore changed nothing, and the case was
asserting something no code path could get wrong. Replaced with `app/scripts/
build`, a file whose own name *is* a build directory's name, which a shell
script with no extension plausibly is. The mutation is caught now, by that
case alone.

This is the `CLAUDE.md` rule about equivalent mutations arriving from the other
direction: not "the mutation was not a change", but "the mutation was a change
that the fixture could not see". Same symptom, and the fix is in the test
rather than in the tool.

**All six, after the fix:**

| mutation | caught by |
|---|---|
| the filename is included in the directory match | `a file whose own name is a build directory's name is not one` |
| `check-ignore`'s exit code read backwards | 4 cases |
| `target` dropped from `BUILD_DIRS` | `a tracked file under target/ is reported` |
| `dist` dropped from `PACKAGE_OUTPUT` | `a package root with node_modules ignored but not dist is reported` |
| package roots read from disk, so vendored ones count | 4 cases |
| the tracked-under-build check forced to pass | 3 cases |

The mutations were re-run **after** `ruff format` rewrote the file — it
collapsed the `committed` comprehension onto one line — because a moved anchor
is the first of the five ways a mutation run lies, and the one this repository
has met most often.

**`scripts/check.sh`: 59 passed, all of them** (57 before). `ty` and `ruff`
run against `/tmp/ci-env`, the way CI does: clean, and `ruff format` applied.

**Batch seven, nine still true:**

| caveat | checked against |
|---|---|
| a page's row count is not bounded by `limit` | unchanged; `max_returned_rows` is still write-only |
| no per-level predicate, ordering or limit | `message Relation` is `table`, `foreign_key`, `direction` |
| nothing measures a page's cost | no page timing in `docs/performance.md` |
| Go's `Transact` cannot be used as a method value | `clients/go/slate/transact.go:72` is still `func Transact[T any](` |
| nothing measures a retry under contention | no contention measurement anywhere |
| no server-side attribution for batching | no per-arm instrumentation in `examples/batchbench` |
| nothing makes the frontend units part of a pre-push run | `scripts/check.sh:134` runs `npm run typecheck` for the demo, not `npm test` |
| the cross-column check shape is fixture-only | `clients/python/tests/test_details.py:248` builds it by hand |
| only N4's *Stops at* was re-read | `docs/orm-comparison.md` has **eight** `*Stops at.*` lines; one was checked |

**Rate.** Six closures over 70 reads today — 1 in 11.7, against the 1-in-8
prior from the earlier sample of eighteen. Two of today's six were closed by
writing the guard the caveat asked for rather than by finding the caveat
already stale, which is a different thing and is counted separately: **four
found stale, two closed by doing the work.**

## What this does not do

**`out/`, `.next/` and `bin/` are not in `BUILD_DIRS`.** The list is the names
this repository's toolchains write. A new toolchain is a line here, and nothing
reminds anyone to add it — the same roster weakness the alternatives section
accepts on purpose.

**It says nothing about the history.** The half-megabyte the 2026-09-18 entry
untracked is still in the pack and this does not change that; that caveat
(`the repository still has the artifacts in its history`) is separate and
remains open.

**Ignore coverage is checked for JavaScript packages only.** A new Rust crate
with a stray `target/` is caught by the tracked-file check after the fact and
not by the coverage check before it, because a `Cargo.toml` says nothing about
where its output lands — the workspace's single `target/` is already ignored at
the root. A crate built out-of-tree would slip through the second check.

**The two checks can disagree about what counts.** `BUILD_DIRS` has five names
and `PACKAGE_OUTPUT` has two, and nothing ties them together. That is
deliberate — `target` is cargo's and a `package.json` says nothing about it —
and it does mean a reader must read both lists to know what is covered.
