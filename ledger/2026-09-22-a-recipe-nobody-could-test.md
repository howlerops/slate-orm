# The disk-reclaim recipe was a snippet to paste, so it could report "freed 0.00 GB" from the wrong directory and nothing could tell that from an already-clean tree.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #286 (F6f)
- **Touches:** `scripts/reclaim.py` (new), `scripts/test_reclaim.py` (new), `scripts/check.sh`, `.github/workflows/ci.yml`, `CLAUDE.md`, `ledger/README.md`
- **Kind:** turning a pasted snippet into a file that can be tested

## What changed

`scripts/reclaim.py`, replacing the `python3 -c "..."` block in
`ledger/README.md`. It keeps the newest build of each target, drops superseded
copies, removes `incremental/` and `examples/`, and **refuses** to touch
`target/*/build`. `--dry-run` reports without deleting. `CLAUDE.md` and
`ledger/README.md` point at it.

## Why

The snippet was pasted five times in one session — ENOSPC is the most common
interruption in this container — and twice from the wrong working directory. It
opens with `cd target/debug/deps`, and from anywhere else it finds nothing and
prints `freed 0.00 GB`, which is **the same output as a tree with nothing to
free**. That is the "a skip is green" failure in `CLAUDE.md`, in the one tool
reached for when a build has already failed and judgement is worst.

Nothing tested it, either, and it had a real defect in its history: it keyed on
filename alone and skipped any name containing a dot, so it dedupped the test
binaries — a handful of files — and left every `.rlib` and `.rmeta`, which is
most of what is on disk. On a tree it had just "cleaned", regrouping by name
**and extension** freed 6.5 GB more. That correction lived as a warning
paragraph telling the reader to check they had the right version. It is a test
case now.

## Alternatives rejected

**Leave it a snippet and fix the `cd`.** One line, no new file. It does not
address the part that matters: a snippet cannot be run by CI, so the next
regression in it is found the same way the last one was — by someone noticing
their disk did not get emptier.

**A shell script.** The grouping logic is a dict of lists keyed on a regex
match; in `sh` that is `awk`, and the extension-in-the-key bug is exactly the
kind of thing `awk` makes easy to get wrong twice. The repository already runs
Python guards in CI, so this costs no new dependency.

**Call `cargo clean`.** It is the supported tool and it is wrong here: it
removes everything, including the build-script outputs under `build/` that must
survive, and the next build then pays full cost. The whole point is dropping
only what a newer build has already superseded.

**Have it run automatically before big builds.** Tempting, and a foot-gun: a
sweep concurrent with another agent's link step deletes artifacts that build is
about to read. Manual, with `--dry-run`, is the honest interface for a shared
disk.

## Evidence

**Seven mutations, six caught, one recorded as an expected survivor**, and the
round of mutations before that is the useful part of this entry.

Three survived the first time, and two of them were the *same* finding:
`PROTECTED`/`protected()` was **dead code**. Nothing in `DISPOSABLE` is
protected, so the refusal never ran — deleting the check changed no verdict,
and adding `build` to the sweep list changed no verdict either, because each
was covered by the other. A defence in depth where neither layer is exercised
is two unverified claims, not two defences. `sweep` now takes `disposable` as a
parameter, and the case sweeps **with `build` in the list** and requires the
build-script output to survive anyway. Both mutations are caught now; the third
— adding `build` to the default list — is recorded as an expected survivor,
because with the refusal proven it genuinely is harmless.

The third survivor was a bad test of mine: "a tie is broken the same way twice"
called `superseded()` twice in one process and compared. `os.scandir` order is
stable within a run, so it would have passed with no tiebreak at all. It now
asserts the survivor is the greatest path, which is the documented rule.

**Measured on the real tree:** `--dry-run` reported 2.79 GB across 3,518 files,
and the run freed it.

`python3 scripts/test_reclaim.py`: 7 passed, 0 failed.
`sh scripts/check.sh`: **47 passed, all of them.** `test_check_sh.py` accounts
for the new `ci.yml` step: 88 steps, all accounted for.

## What this does not do

**It does not detect ENOSPC or run itself.** It is still something a person or
agent has to think to run, after a confusing linker error. The `CLAUDE.md` note
is still the thing that connects `Bus error` to this script.

**It does not know what another build is using.** It keeps the newest artifact
per target, which is a heuristic: a build in flight against an older one loses
its input. That has not happened here, and the alternative — parsing cargo's
fingerprints — is far more machinery than this is worth.

**It only sweeps `target/`.** The other things that fill this disk — a stale
`node_modules`, downloaded fixtures under `site/data/`, an S3 scratch directory
— are untouched and undiscovered by it.

**`--dry-run`'s byte count can exceed what a real run frees**, because a file
can vanish between the two. The run prints the filesystem's own before/after
for exactly that reason.
