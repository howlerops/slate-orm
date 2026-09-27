# A verdict written four hours earlier cited a guard that has never existed, and the check written to find it found it on its first run

- **Date:** 2026-09-26
- **Author:** Claude, working from the standing instruction to address every caveat
- **Touches:** `scripts/{check_caveat_citations.py,test_check_caveat_citations.py,check_cited_docs.py,check.sh}`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

`scripts/check_caveat_citations.py` reads `docs/caveat-status.json` and fails
if any verdict's `by` names a path that does not resolve. It is wired into
`scripts/check.sh` (now 61 steps) and into CI, with
`scripts/test_check_caveat_citations.py` — 21 cases over written trees — beside
it.

Its first run reported one defect and two pieces of prose.

**The defect: `scripts/check_unverifiable_claims.py` has never existed.** I
wrote it into a `by` four hours earlier, in batch thirteen of the verdict
sweep, as the guard that "exists to refuse" unverifiable claims. The guard that
does that is `scripts/check_cost_prose.py`, and its `is N% low` rule is the one
I meant. Corrected in the file.

**The prose: `ledger/2026-09-22 #285`**, twice, meaning *the 2026-09-22 entry
for task #285*. That resolves for a person and not for a tool. Both are now
spelled `ledger/2026-09-22-four-dependencies-and-a-tool-that-was-lying.md`,
because a citation nothing can follow is a citation nothing checks.

`2026-09-24-the-ledger-records-762-caveats-and-tracked-none-of-them.md`'s
caveat becomes `narrowed`.

## Why

That caveat named this hole precisely, ten days before it bit:

> **A verdict is a judgement, and nothing checks it.** `closed` must name
> something, and that name is not verified to exist or to say what the verdict
> claims — **`check_cited_docs.py` would catch a dead path in a source file but
> does not read this JSON.** A wrong `closed` is invisible.

It is right about the mechanism and it understates the frequency. **This is the
fourth invented citation in two days**, and the pattern is worth naming because
it is not carelessness about *facts* — it is carelessness about *filenames*,
which is a different failure with a different fix:

| when | what was invented | what caught it |
|---|---|---|
| 2026-09-25 | four `ledger/…` entry filenames, in an entry | a person re-reading, by hand |
| 2026-09-26 | two `ledger/mutations/…` timestamps, in an entry | `check_mutation_claims.py`, within the minute |
| 2026-09-26 | `scripts/check_unverifiable_claims.py`, in a verdict | this, on its first run |

Each was caught by whichever guard happened to read the tree the claim sat in,
and the common factor in all three is a *plausible* filename recalled rather
than looked up. `check_unverifiable_claims.py` is exactly what a guard for
unverifiable claims would be called in this repository; that is why it came out
of my fingers, and it is why reading it back does not catch it. A name that
fits the local convention reads as a name that exists.

The fix that works is not "be careful" — it is that every tree where a filename
can be written now has something that opens it. `docs/caveat-status.json` was
the last one that did not.

## Alternatives rejected

**Widen `check_cited_docs.py` instead of writing a new guard.** It is the
obvious move and it is wrong twice. Its contract is *source files cite
documents* — it reads `.rs`, `.py`, `.go`, `.ts` and matches paths ending
`.md`. This file is JSON, and the citations in it name `.py`, `.rs`, `.json`
and directories as often as `.md`. Widening it to cover both would mean two
suffix rosters, two tree rosters and a docstring explaining which applies
where, for a guard whose value is that it is narrow enough to state in one
sentence. Cost of the split: two files to read instead of one; the guard's
docstring names its sibling.

**Require every `by` to name a file.** It would make every verdict
machine-followable, and it would be actively harmful: 38 of the verdicts
written today say "the caveat itself", because the reasoning genuinely is in
the bullet being judged. Forcing a path there produces a *plausible* filename
where an honest sentence was — which is the exact failure this guard exists to
catch, manufactured on purpose. The guard reports those as invisible rather
than as wrong.

**Check that the cited entry says what the verdict claims.** The caveat asks
for it and it is the larger half, which is why it stays open in the `narrowed`
residual. It means reading the entry and judging it, and that judgement is what
a verdict *is* — a checker that could do it would not need the verdicts. The
same limitation is written into `check_cited_docs.py` and
`check_cited_tests.py` and has been since they were written.

**Match any path-shaped string rather than one rooted at a known tree.** It
would have caught `check_unverifiable_claims.py` even written bare, without
`scripts/`. It would also read `Row::validate`, `a/b` in prose and every
`foo.py` mentioned in passing as citations, and the roster of trees is a list
somebody has to edit — which is the idiom this repository already runs on.
`check_cited_docs.py` made the same choice for the same reason.

## Evidence

**The defect, found by the guard on its first run against the real file:**

```
FAIL  every cited path resolves
      2026-09-25-the-landing-pages-claims-are-checked-now.md: scripts/check_unverifiable_claims.py
      2026-09-22-a-number-that-cannot-say-what-built-it.md: ledger/2026-09-22
      2026-09-22-four-dependencies-and-a-tool-that-was-lying.md: ledger/2026-09-22
```

After the three corrections: `ok 123 citations across 879 verdicts`, all
resolving.

**The guard's own suite:** 21 cases, all passing, over trees the file writes —
including the three parse shapes that would have made it silently weak (a
trailing full stop, a relative path climbing out of the tree, a word merely
ending in a tree name), a missing status file, a file that is not JSON, and
a file whose every `by` is prose.

**Mutation run**, `ledger/mutations/20260926T015308-scripts-check-caveat-citations-py.json`
— **eight cases, eight caught**, each by a named test:

| mutation | the test that failed |
|---|---|
| `if not (root / cited).exists():` → `if False:` | *a citation naming a file that does not exist is reported* (and two more) |
| `counted > 0,` → `True,` | *a file where no verdict cites any path is reported, not passed* |
| drop the trailing `[\w/]` from `CITATION` | *a trailing full stop is not part of the path* |
| drop the leading lookbehind from `CITATION` | *a relative path out of the tree is not a citation* |
| the JSON-parse failure records `True` | *a status file that is not JSON is reported rather than passing empty* |
| the missing-file check records `True` | *a missing status file is reported rather than passing empty* |
| `if hit not in seen:` → `if True:` | *the same path twice is one citation* |
| remove `"ledger"` from `TREES` | *a file whose every citation resolves passes* (and three more) |

Each replacement changes behaviour rather than spelling, checked before running
per `CLAUDE.md`'s note on the equivalent mutation: three are control-flow
inversions, two flip a recorded boolean, two remove a regex guard whose absence
changes what matches, one shortens a roster.

**`check_cited_docs.py` reported the new test file on the first run**, for
`ledger/a.md`, `ledger/b.md` and `ledger/c.md` — fixture paths in written
trees, and the `FIXTURES` roster is where that is declared with a sentence. It
also reported the *guard's own docstring*, which illustrated the two parse
guards with invented `ledger/…` paths. Those were reworded rather than
rostered: a docstring showing a fake path is a citation, the cases belong in
the test file anyway, and adding a second exemption would have been the roster
absorbing a mistake instead of reporting one.

`sh scripts/check.sh`: **61 passed, all of them.**

## What this does not do

**It checks existence, not aptness**, word for word as `check_cited_docs.py`
does. A `by` naming an entry that exists and does not say what the verdict
claims passes. That is the residual of the caveat this narrows, it is the
larger half, and nothing here is a step towards it.

**38 verdicts cite "the caveat itself" and are invisible to it.** They are
honest and unfollowable, and the alternatives section says why making them
followable would be worse. The guard reports a count of citations it *can*
follow and says nothing about the proportion it cannot; a reader comparing 123
citations against 879 verdicts might read the gap as dead weight rather than as
prose.

**Nothing checks a `by` that names a test, a function or a commit.** Several
name `security_probe.rs`'s test functions or a commit's shape, and only the
file half of those is followed. `check_cited_tests.py` does the test-name half
for source files and does not read this JSON either — the same widening, not
done, for the same reason the first alternative gives.

**The fourth invented citation was found by a guard written in the same hour
to look for it.** That is not a demonstration that the class is now covered; it
is a demonstration that this instance is. The three trees where a filename can
be written now each have a guard, and a fifth invented citation will be
somewhere none of them reads.
