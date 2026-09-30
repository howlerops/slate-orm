# The file the guard is named after carried the stale count, twice

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `scripts/check.sh`, `scripts/test_check_sh.py`, `CLAUDE.md`
- **Kind:** fix

## What changed

Three corrections and one widened rule, found by asking whether the previous
commit had left anything stale.

**`scripts/check.sh`'s header was wrong in two numbers.** It said *"`ci.yml` is
seventeen jobs and the static checks in it are spread across six of them"*.
`ci.yml` is **twenty-three** jobs and **eleven** of them hold a check the
script also runs. Both corrected.

**`job_count_matches()` now reads `check.sh` too**, not only `CLAUDE.md`, and
holds both of its numbers: the job count, and the derived count of jobs holding
a static check. `COUNTED` grew downward to cover the second, which lives lower
than the first.

**CLAUDE.md described half a guard.** It said `test_check_sh.py` "fails if a
step is added to `ci.yml` and neither listed in the script nor written down as
one it cannot run" — true this morning and half the story since the previous
commit, which added the reverse direction.

## Why

The previous entry closed a caveat about `test_check_sh.py` reading one
direction. It did not ask what *else* in the repository described that guard,
and CLAUDE.md did — a sentence that became half-true the moment the code
changed. That is the rule this repository states as **stale documentation is
worse than none, because it is read as current**, and the answer to "is
everything updated?" is only ever found by looking.

The two numbers are worse, and funnier. `job_count_matches()` exists because
CLAUDE.md's job count went stale twice — it said *seventeen* for as long as it
did, then *twenty-one* against twenty-two. The function was written to stop
that recurring. **It was reading one file, and the other file carrying the same
stale number was `check.sh`, which the function is named after.** Nobody had
looked, because a guard about a number feels like it covers the number.

The count it carried was *seventeen* — the same wrong figure, from the same
era, surviving in the file next door to the check that exists to prevent it.

The second number is the sharper one. "Spread across six of them" is not a
count of jobs, it is a count of *jobs holding a static check*, so the existing
rule could not have caught it even pointed at the right file. It is eleven,
derived from `script_checks()` rather than counted by hand, so the sentence and
the roster cannot disagree about what is being counted.

## Alternatives rejected

**Fix the numbers and not the rule.** Ten seconds, and it is the move that
produced this situation: the numbers were fixed by hand at least twice before,
and each time the fixing stopped at the file somebody happened to be reading.
A third hand-correction with no rule is a promise to do it a fourth time.

**Extend the rule to every file, by globbing for "N jobs" across the tree.**
The general version, and rejected on false positives: "jobs" is an ordinary
word. `check.sh` alone says *"the jobs that find the interesting failures"*,
and the first draft of this reported `'the'` as a number it did not recognise.
A guard that cries wolf on English is one people stop reading, which is the
failure `check_cited_tests.py` measured at 87% and rejected. The rule is
anchored on the claim — `` `ci.yml` is N jobs `` — in the two files that make
it.

**Give the spread count its own word map rather than extending `COUNTED`.**
Cleaner in principle: two quantities, two ranges, two lists. Rejected because a
second map of number words goes stale on its own schedule and for its own
reasons, and the thing being avoided here is exactly a second list nobody
maintains. One map, with a comment saying it serves two counts.

**Leave the spread sentence out of the guard and just write "several".** Honest
and unfalsifiable, which is the trade: "eleven" tells a reader how scattered
the checks are, which is the sentence's whole job. A number worth writing is a
number worth holding.

## Evidence

Both counts derived rather than eyeballed. `A_JOB` over `ci.yml` returns **23**.
`jobs_holding_a_static_check()` — `script_checks()` intersected with each job's
`- run:` lines, plus `SPELLED_IN_CI`'s values — returns **11**, from `demo`,
`fmt`, `frontend`, `go`, `hooks`, `layout`, `python`, `quickstarts`, `rust`,
`scripts` and `typescript`.

`python3 scripts/test_check_sh.py`: **9 passed, 0 failed**, `148 steps, 16
blocks and 2 env vars, all accounted for`.

`sh scripts/check.sh`: exit 0, **87 passed, all of them**.
`python3 site/check/docs.py`: the docs site holds together.

Three mutations against `scripts/check.sh`, all caught, record
`ledger/mutations/20260929T194649-scripts-check-sh.json`:

| mutation | outcome |
| --- | --- |
| the job count drifts back to seventeen | caught |
| the spread count drifts back to six | caught |
| the sentence carrying both is rewritten to say neither | caught |

The third is the one that matters. A rule keyed on a sentence goes blind when
somebody rewrites the sentence, and it goes blind *quietly* — the count it was
holding simply stops being found. Both halves report "the sentence moved — put
this back on it" rather than passing, which is the difference between a guard
that failed and a guard that stopped existing.

A fourth possibility was checked and is not a mutation: whether extending
`COUNTED` down to `one` makes CLAUDE.md's looser loop fire on ordinary prose.
`grep -on "[a-z-]* jobs" CLAUDE.md` returns three matches, two of which are the
count itself and one of which has no word before it. No false positive today.

## What this does not do

**CLAUDE.md's half is still a loop over every word before "jobs", not an
anchored match.** It reports any unrecognised word, which is right for that
file today and would be wrong the moment somebody writes "the four jobs that
build binaries". Extending `COUNTED` downward made that more likely, not less —
the words `four` and `six` now resolve to numbers and would be compared against
23. The two halves of this function are asymmetric and only one of them was
hardened.

**Nothing derives the prose from the count.** Both files still *state* a number
that a rule compares; neither generates it. A reader editing the sentence has
to get the word right, and the guard tells them afterwards rather than the
editor telling them at the time.

**Only two files were swept.** The question that started this was "what else
describes the guard", and the answer was found by grepping for `test_check_sh`
and for `seventeen`. A third file describing it in words that match neither
pattern is exactly as stale as these two were, and nothing looked.

**The eleven is a count of jobs, not of coverage.** A job holding one of the
script's checks counts the same as `scripts`, which holds seventy-one. The
sentence says how *scattered* the checks are and not how much of CI the script
reproduces, and a reader could take it for the second.
