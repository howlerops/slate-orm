# A ledger of why, enforced by a pre-commit hook

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5), at the repository owner's request
- **Touches:** `ledger/`, `.githooks/pre-commit`, `CLAUDE.md`, `.claude/settings.json`
- **Kind:** process

## What changed

A `ledger/` directory with one markdown file per change, a `pre-commit` hook
that refuses a commit touching anything outside it without an entry, a
`CLAUDE.md` telling every agent the rule and the standards it sits inside, and
a `SessionStart` hook that points `core.hooksPath` at `.githooks` so a fresh
clone is covered without anybody remembering to run anything.

## Why

The reasoning behind a change is the expensive part and the first thing to
decay. A diff says what a line became; it does not say what the alternatives
were, which one was measured, or which invariant the change was protecting.
Commit messages in this repository already carry a lot of that, but they are
found only by someone who already knows which commit to look at.

The sharper reason is that this repository is worked on by several agents at
once, so "ask whoever wrote it" is not available even in principle. In the
session that prompted this, four workers ran in parallel and produced twelve
defect fixes between them; reconstructing which finding drove which change
already meant reading across four separate reports.

## Alternatives rejected

**A single append-only `LEDGER.md`.** The obvious shape, and unusable here:
with several agents committing concurrently it conflicts on every change. Every
worker appends at the end of the same file, which is the worst case for a
textual merge. One file per entry never conflicts.

**Sequential numbering (`0001-`, `0002-`).** Conflicts the same way as soon as
two entries are written in the same hour — both workers pick the next number.
A dated slug collides only when two people name the same change the same thing
on the same day, which is a signal rather than a merge problem.

**A hook that only checks a file exists.** Rejected because it teaches the rule
can be satisfied without thinking, and a rubber-stamp entry is worse than none:
it looks like the reasoning was recorded. The hook checks the five sections are
present, that each has something under it, and that the template's own prose
has been replaced. It cannot check whether what was written is true — that
limit is stated in `CLAUDE.md` rather than pretended away.

**No escape hatch.** Rejected on the grounds that a check with no bypass gets
switched off permanently the first time it blocks something urgent, and then
there is no check at all. `--no-verify` works, is documented, and is paired
with the expectation that the entry gets written afterwards.

**A hook in `.git/hooks`.** Not versioned, so it would exist only on the
machine that created it. `core.hooksPath` pointing at a tracked directory is
the shareable form, wired automatically at session start.

**Requiring an entry for merges and reverts.** Exempted. A merge's reasoning
belongs to the commits being merged, and requiring a fresh entry to revert
something is paperwork in front of a recovery.

## Evidence

The hook was tested against all five paths rather than assumed: a code change
with no entry is refused; an entry still carrying the template prose is refused
and says so by filename; an entry with an empty `## Alternatives rejected` is
refused and names the section; a complete entry is accepted; a commit touching
only `ledger/` is accepted with no entry. The script is `sh` with no
dependencies and passes `sh -n`.

## What this does not do

It cannot tell whether an entry is honest or useful — only that it is shaped
like one and is not the template. The failure mode this leaves open is a
well-formed entry that says nothing, and no hook can close it.

It does not backfill. The thirty commits already on this branch carry their
reasoning in their messages and are not being restated here; the ledger starts
now. `2026-09-13-defects-found-by-review.md` indexes the twelve defects from
that session, because "where did this come from" is exactly the question the
ledger exists for and those were spread across four separate agent reports.

It enforces nothing on a push, only on a local commit. A commit made elsewhere
with `--no-verify`, or by a tool that skips hooks, arrives unchecked.
