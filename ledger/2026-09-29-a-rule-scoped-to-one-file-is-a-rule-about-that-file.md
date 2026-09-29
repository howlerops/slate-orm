# The step roster read one workflow of five, and nine steps were accounted for by coincidence

- **Date:** 2026-09-29
- **Author:** Claude Code, task K3
- **Touches:** `scripts/test_check_sh.py`
- **Kind:** test

## What changed

`scripts/test_check_sh.py` reads **every** workflow under `.github/workflows/`
rather than `ci.yml` alone. Nine steps and named blocks became visible and none
of them had a reason; each now has one. Two new reported checks cover the
stitching, one new fixture exercises it, and the three never-fires `assert`s
are reported checks instead of assertions.

## Why

`ledger/2026-09-29-a-weekly-run-and-a-button.md` added `mutations.yml` and
wrote down the hole in the same breath:

> Nothing checks this workflow's steps the way `test_check_sh.py` checks
> `ci.yml`'s. […] Both steps here happen to be accounted for
> (`mutations-roster` is in `check.sh`; `run_mutations.py` is deliberately not),
> **but by coincidence rather than by a rule.**

Widening the glob turned that into a measurement. Nine things were outside the
rule:

| where | what |
|---|---|
| `mutations.yml` | `python3 scripts/run_mutations.py`, `git diff --exit-code` |
| `release.yml` | `ls -l dist` |
| `pages.yml` | *The pages exist*, *If that failed, here is why and what to do* |
| `release-build.yml` | *Cross-compilation toolchain*, *Build*, *It starts, and validates a configuration*, *Name it after its target* |

The entry's own claim was right — `check_mutations_roster.py` is in `check.sh`
and passed — and it was right about one of the nine. The step count the guard
reports went from 105 to **130**.

Two of the three findings below came out of mutating the widened guard, which
is why the change is larger than a glob.

**A never-fires `assert` scores as a suite that never started.** Narrowing the
glob back to `ci.yml` should be caught by `assert len(found) > 1`. It was not
caught; `scripts/mutate.py` reported **NOTHING RAN**, because an
`AssertionError` kills the run before the `N passed, M failed` line the
`python` dialect reads. A guard correctly refusing and a suite failing to start
produce the same empty output — which is lie number two in `mutate.py`'s own
docstring, met inside the file whose entire job is to stop a check going quiet.
The three assertions are now entries in a `blind` list reported as *the parser
still sees the workflows*, and the same mutation now dies by name.

**A defence against a shape the tree does not have is untested by
construction.** Joining several files needs a sentinel between them, or a
`- run:` at the end of one workflow takes the `working-directory:` at the start
of the next. Deleting the sentinel survived everything, because no workflow
here ends that way. `workflow_steps()` now takes its files as an argument so a
fixture can supply the shape, and the mutation dies.

The first draft of that fixture **also** left the mutation surviving: its
second file began `jobs:` with the `working-directory:` three lines down, which
is a picture of the shape rather than the shape. Worth recording, because it is
the same error as an equivalent mutation and reads the same way — a case that
looks like coverage and is not.

## Alternatives rejected

**A separate guard for the other four workflows.** Two rosters drift apart, and
the reason `ELSEWHERE` works is that it is one list somebody is forced to edit.
A second list is a second thing to forget.

**Excluding `release-build.yml` and `pages.yml` as "not checks".** They *are*
not static checks, which is why all six of their blocks are `ELSEWHERE` with a
reason. But the rule is "every step is accounted for", not "every step is a
check" — a build step arriving in `ci.yml` gets a reason today, and there is no
principle that makes the same step in another file exempt.

**Keeping the assertions and teaching `mutate.py` to read a traceback.** A
runner that treats a crash as a pass is a real gap and
`ledger/2026-09-29-the-dialect-mutate-py-was-missing.md` is about exactly that
class. But this file is not a guard invoked as a subprocess — it is a suite, and
a suite that crashes rather than reporting has a simpler fix: report. Teaching
the runner would leave every other suite free to crash.

**Leaving the sentinel untested and recording an `expect_survivor`.**
`expect_survivor` takes the reason survival is *correct*, and here it is not:
the code is not redundant, the case was simply missing. CLAUDE.md's rule is to
write the test rather than hide it, and the test cost five lines.

## Evidence

- **Nine unaccounted steps and blocks**, listed above, from the first run after
  the widening: `2 passed, 2 failed`, naming all nine. With reasons added,
  `6 passed, 0 failed` over **130 steps, 12 blocks and 2 env vars**.
- **Mutation, against the guard**, `scripts/mutate.py`, record
  `ledger/mutations/20260929T134606-scripts-test-check-sh-py.json`: three
  cases, three caught.
  - narrowing the glob to `ci.yml` → *the parser still sees the workflows*;
  - deleting the sentinel between files → *several workflows are stitched
    together without bleeding into each other*;
  - reading only `files[:1]` → the same check, and *no ELSEWHERE entry names a
    step the workflows have dropped*.
  Each was a change: the glob returns one path instead of five, the sentinel
  line is removed, the slice drops every file after the first.
- **Mutation, against the real tree**, record
  `ledger/mutations/20260929T134411-github-workflows-mutations-yml.json`:
  adding `--strict` to `mutations.yml`'s roster step → caught by *every
  workflow step is in check.sh or in ELSEWHERE*. This is the case that proves
  the widening points at something: before it, a step added to `mutations.yml`
  was invisible.
- **The two findings above are in the records, not reconstructed.**
  `ledger/mutations/20260929T134411-scripts-test-check-sh-py.json` scores the
  glob mutation `nothing-ran` and the sentinel mutation `survived`;
  `ledger/mutations/20260929T134529-scripts-test-check-sh-py.json` has the glob
  mutation `caught` — the assertions were reported checks by then — and the
  sentinel still `survived`, against the fixture that did not reach the bug.
- `python3 scripts/check_guard_scope.py`: 29 guards, 3 making a scope claim,
  every claim matching.

## What this does not do

**It reads `- run:` and `- name:`, not YAML.** A step written as a mapping the
regexes do not expect is invisible, exactly as before; the widening multiplies
whatever the parser misses by five files instead of one. Parsing the workflows
properly needs a YAML dependency `check.sh` does not have and the
`scripts` job does not install.

**`environment_matches` is still `ci.yml` only.** It compares `check.sh`'s
exports against one workflow's `env:` block, and no other workflow has one
today. If `mutations.yml` grows a workflow-level `env:`, nothing will notice —
a hole of exactly the shape this entry closed, one level down.

**The never-fires branch is exercised by one mutation, not by a case.** Nothing
here fails if `blind` stops reporting: disabling it on a healthy tree changes
no answer, and a fourth case doing exactly that survived —
`ledger/mutations/20260929T134550-scripts-test-check-sh-py.json`. It is a
survivor for the honest reason, so it was dropped rather than filed under
`expect_survivor`: a single mutation cannot show a guard fires when its trigger
is absent. What covers the branch is the glob mutation dying by that check's
name, which is a mutation run rather than a test in the file.

**Six of the nine new reasons are for blocks in workflows nobody here can
run.** `release-build.yml` cross-compiles and `pages.yml` deploys; the reasons
are read off the workflow, not from having watched the step. They say what the
step is for, which is what a reader needs, and not that it works.
