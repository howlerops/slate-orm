# The restore check said "did not come back clean" whether the tree was dirty or unscorable, and printed an empty list either way

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-26-a-cached-go-run-is-not-a-run.md`
- **Touches:** `scripts/mutate.py`, `scripts/test_mutate.py`
- **Kind:** fix

## What changed

`restore_report(failures, reported, status)` returns the line to print and
whether the tree came back clean, and `apply` prints it. Three outcomes rather
than two:

| | printed | `restored_clean` |
|---|---|---|
| a named failing test | `the tree did not come back clean: [...]` | `false` |
| nothing reported | `the restore could not be scored: nothing reported …` | `null` |
| an exit this dialect cannot read | `the restore could not be scored: the command exited N …` | `null` |
| clean | `restored: N suites reported, none failing` | `true` |

All three of the first rows are still problems and still exit non-zero. Four
checks in `test_mutate.py`, taking it from 70 to 74.

## Why

The caveat is exact about the defect:

> When the restore verification also cannot score, the run prints "the tree did
> not come back clean" with an empty failure list, which reads as an unclean
> tree and means "I could not confirm either way".

An empty list after that sentence is the worst of both readings: it asserts a
dirty tree and shows no evidence for one. And the reader of that line is
deciding whether to trust every result above it, which is exactly when the
difference between *this is broken* and *I could not tell* matters.

`None` rather than `false` in the record for the same reason. A later reader of
`ledger/mutations/*.json` has no way to tell a run that came back dirty from one
that could not be scored if both say `false`, and the second is the one that
invalidates the results above it.

## Alternatives rejected

**Reword the one line to hedge** — "the tree may not have come back clean". One
edit and no new function, and it makes the *dirty* case, which is the
unambiguous one, read as a maybe. The three states are distinguishable; saying
so is cheaper than blurring all of them.

**Keep `false` in the record and distinguish only in the prose.** The prose is
read once, by whoever ran it; the record is what
`ledger/2026-09-27-the-guard-now-reads-the-runner-it-depends-on.md` reads
later. Recording the weaker fact is the class of quiet this whole file is
about.

**Leave it in `apply` and test through a real mutation run.** The dirty arm is
reachable that way. The two unscorable arms are not: reaching them needs a
command that scores its baseline and *then* stops reporting, which cannot be
arranged here on purpose — and an untestable branch about an unreadable result
is precisely the shape `unreadable` itself was written to catch. Splitting out
a pure function is what makes all four states assertable.

## Evidence

- `python3 scripts/test_mutate.py` — 74 passed, 0 failed (70 before).
- Four mutations via `scripts/mutate.py`, recorded as
  `ledger/mutations/20260927T025229-scripts-mutate-py.json`:
  - reporting a dirty tree as unscorable → caught by *a named failure is a
    dirty tree*;
  - treating "nothing reported" as clean → caught by *nothing reporting cannot
    be scored*;
  - treating an unreadable exit as clean → caught by *an unreadable exit cannot
    be scored*;
  - **one expected survivor**: making an unscorable restore stop counting as a
    problem. `test_mutate.py` calls `restore_report` directly and never reaches
    the line in `apply` that counts its answer, for the same reason the arms
    themselves cannot be driven.
- The three-outcome table above was produced by calling `restore_report` with
  each of the four input shapes, not by reading the branches.

## What this does not do

**`apply`'s use of the answer is still untested**, which the expected survivor
records rather than hides. The function decides correctly and nothing checks
that its caller acts on the decision; a `problems += clean is False` would let
an unscorable restore exit zero, and only a reader would notice.

**Nothing reads `restored_clean` yet.** It has been in every record since
records existed and no guard or script consumes it, so widening it from
`bool` to `bool | None` costs nothing today and might cost a future reader who
assumed two states. Written down here because a JSON field nobody reads is one
whose shape nobody is defending.

**The wording is still one person's judgement.** "Could not be scored" is
clearer than what it replaces, which is a low bar; whether it is clear to
somebody meeting it at two in the morning after a killed run is not something a
test can answer.
