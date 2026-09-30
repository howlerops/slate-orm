# The dialect `mutate.py` was missing, and the five checks it could not score

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-a-roster-of-test-names-is-weaker-and-worth-having.md`
- **Touches:** `scripts/mutate_guard.py`, `scripts/test_mutate_guard.py`,
  `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** process

## What changed

`scripts/mutate_guard.py` runs one or more `scripts/check_*.py` guards and
reports in the `ok    name` / `FAIL  name` / `N passed, M failed` shape that
`scripts/mutate.py`'s `python` dialect already reads. A mutation aimed at the
*real tree* and judged by a guard now goes through that script like any other,
with the restore in a `finally`, the exactly-once anchor check, the
reported-suite count and the JSON spec that never reaches a shell.

The five real-tree mutations the previous entry had to run by hand were
re-run through it, and are on record.

## Why

The previous entry left this:

> **The three real-tree mutations were hand-run.** `scripts/mutate.py` cannot
> read this guard's output, so the protections in that script … did not apply
> to the three checks that matter most. Giving `mutate.py` a dialect for a
> guard that prints `ok    …` on success and problems on stderr would fix it
> for every guard in `scripts/`, and is not done here.

It matters because a guard's fixture tests and the guard itself answer
different questions, and only one of them is checkable without this. The
fixture tests prove the *rules* refuse what they should, over a tree the test
builds. Only the guard, run against the real tree with the real tree broken,
proves the rules are pointed at anything. The distinction is not academic:
`check_renamed_column.py`'s first draft passed all fifteen of its fixture cases
while its main rule was matching a line in an unrelated test, and mutating the
real client files is what said so. There are twenty-seven guards and that check
was available for none of them.

## Alternatives rejected

**A sixth entry in `mutate.py`'s `DIALECTS` table.** The obvious fix, and the
wrong one. The twenty-seven guards agree on an exit code and on nothing else:
seventeen open with `ok    `, six print a bare count, four lead with their
subject. A pattern matching all of them matches almost any line — and a report
pattern that over-matches is the one lie `mutate.py` cannot catch, which its
own `scripts/test_mutate.py` exists to guard against. A runner keeps the
loose part outside the table and leaves the table exact.

**Pass the guard's exit code straight through.** Simplest, and it would have
made every syntax-breaking mutation score as *caught*: a mutation that stops
the file parsing makes the guard exit non-zero, which under a naive reading is
indistinguishable from the guard refusing. A false catch is silent, and a run
of ten cases would come back clean having judged none of them. Hence the three
suppressions below, which are the whole content of this script.

**Make every guard print a uniform line.** Twenty-seven edits, each changing a
summary a person reads, to serve a tool. The summaries are good — they say what
was checked and how much of it — and flattening them to `1 passed, 0 failed`
would lose that. The adapter goes in one file instead.

## Evidence

`python3 scripts/test_mutate_guard.py` reports `11 passed, 0 failed` and takes
13.0s, almost all of it the last case: it runs all twenty-seven real guards and
asserts every one prints something and exits 0, which is exactly what the
"a silent pass is not a pass" rule assumes about them.

**The five real-tree mutations, now properly run**, each caught, each naming the
client and the pattern:

- `ledger/mutations/20260929T075229-clients-python-tests-test-fixture-py.json`
  — 2 cases: the declaration changed to the current spelling, and the test
  renamed away.
- `ledger/mutations/20260929T075243-clients-go-slate-schema-test-go.json`
  — 2 cases: the declaration changed, and the catalog no longer calling
  `category` a previous name.
- `ledger/mutations/20260929T075244-clients-typescript-test-schema-test-ts.json`
  — 1 case: the declaration changed.

**The runner's own rules**:
`ledger/mutations/20260929T075509-scripts-mutate-guard-py.json`, 10 cases,
**no survivors**. The three suppressions are each mutated directly, and each
mutation turns four or more cases green-that-should-be-red:

- dropping the traceback check makes a guard that raised report `0 passed, 1
  failed` — a crash scored as a catch;
- accepting exit 2 does the same for a guard that exited confused;
- accepting a silent exit 0 does it for a guard that checked nothing;
- returning 1 rather than 70 from the "no verdict" branch puts the result line
  back, which is the failure in its most direct form.

Also caught: reading only stdout (the guards that refuse on stderr would say
nothing), counting a refusal as a pass, exiting 0 on a refusal, running with no
guard named, and handing a missing path to the interpreter.

**One case here failed on its own message rather than on behaviour, and the
fix is worth recording.** `test_mutate_guard.py` first asserted the absence of
the report line with the substring `" passed, "` — which occurs in this
runner's own explanation that "a guard that could not run is not a guard that
passed, so no result line is printed". Six cases failed on the prose. The
pattern is now imported from `mutate.DIALECTS["python"][1]` rather than copied,
so the test asserts against the exact regex `mutate.py` will use and cannot
drift from it. That is the same offence as a guard passing on a word, met
inside the test written to stop it.

`python3 scripts/test_check_sh.py` reports `120 steps, 6 blocks and 2 env vars,
all accounted for`.

## What this does not do

**It is a runner, not a dialect, so `mutate.py --help` does not list it.** The
generated dialect list is `mutate.py`'s own table, and this is not in it. A
session looking for "can I mutate against a guard" will find the answer in this
entry and in `mutate_guard.py`'s docstring, and nowhere the tool prints — which
is exactly the staleness `mutate.py`'s docstring argues against, in the one
place it cannot fix itself.

**Nothing makes the real-tree mutations run again.** The five above are
recorded and were run once. They are not in CI and there is no roster saying
which guard should be mutated against which file, so the next change to a
client's renamed-column test will not re-run them. A `scripts/mutations.json`
naming the standing mutation suites, run on a schedule rather than per push,
is the shape that would fix it.

**Only `check_renamed_column.py` was actually mutated against.** The runner
works for all twenty-seven — the last test case runs all of them — but
twenty-six have never had a real-tree mutation aimed at them, and at least one
of those is likely to be in the state `check_renamed_column.py` was in: green
for a reason that has nothing to do with what it checks. That is a survey, not
a fix, and it is not done.

**The fixture guards are written into `scripts/` and removed in a `finally`.**
They have to be, because `mutate_guard.py` resolves its arguments against the
repository root it computes from its own location. A killed run leaves
`scripts/passes.py` and five siblings behind — the same hazard `mutate.py`
solved with an in-flight marker, not solved here.
