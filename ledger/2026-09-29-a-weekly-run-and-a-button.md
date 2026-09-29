# A weekly run and a button, and the half of it that cannot fire yet

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-every-guard-now-has-one.md`
- **Touches:** `.github/workflows/mutations.yml`, `docs/caveat-status.json`
- **Kind:** process

## What changed

`.github/workflows/mutations.yml` runs `scripts/check_mutations_roster.py`,
then `scripts/run_mutations.py`, then `git diff --exit-code`. On
`workflow_dispatch`, and weekly on a cron.

## Why

The previous entry named this as the only thing left between the survey and a
check: twenty-seven suites, one command, "worth exactly as much as the number
of times somebody types it". Nobody types it. The guards' *own* failure mode is
being green for a reason nobody looked at, and a set of mutations that runs
when a person remembers is the same shape.

Weekly rather than per-push because the defect class moves with the guards, not
with the code they watch. A guard is edited a few times a month; the tree it
reads is edited a hundred times. Running the whole roster on every push
buys a faster signal on the rare change and pays a minute on every common one,
beside twenty-one other jobs.

## Alternatives rejected

**Put it in `ci.yml` as a twenty-second job.** It breaks the working tree
between each patch and its restore, which is fine in a container CI throws
away, and the timing argument above is the real one. A `paths:` filter on
`scripts/**` was the obvious middle — rejected because `CLAUDE.md` records that
a `paths:` filter that does not match is silent, and because a mutation can go
stale from the *other* side: an anchor moves when the watched file is
reformatted, and that push does not touch `scripts/`.

**`schedule:` only, no button.** Would have been a check that never fires:
GitHub runs scheduled workflows from the default branch alone, and this file is
on a feature branch. That is the exact trap `ci.yml`'s own header describes —
it was marked active and had run zero times for months. `workflow_dispatch`
works on any branch from the moment this lands.

**A cron on the hour.** `0 7 * * 1` queues behind everyone else's and GitHub
drops scheduled runs under load. For a weekly job that is a missed week nobody
notices. `11 7 * * 1` is the same advice `CLAUDE.md` gives about scheduled
times, applied here.

## Evidence

The file parses and declares what it should: `name Mutations | on ['schedule',
'workflow_dispatch'] | jobs ['mutations']`, five steps.

`python3 scripts/check_toolchain_pins.py` still reports `5 pinned go-version
across 1 workflow(s)` — the new workflow pins no language version, so it adds
no `GOTOOLCHAIN` obligation. `python3 scripts/test_check_sh.py` reports `124
steps, 6 blocks and 2 env vars, all accounted for`, unchanged: that guard reads
`ci.yml`, and this is a different file. `sh scripts/check.sh` reports `85
passed, all of them`.

**Not run.** The workflow has never executed. Its steps have each been run by
hand in this container — `check_mutations_roster.py` on every commit today,
and `run_mutations.py` end to end over the whole roster, twice, both recorded
in `ledger/2026-09-29-every-guard-now-has-one.md` — but the workflow as a
workflow is untested, which is the thing its own header says is worth
distrusting. The first `workflow_dispatch` is the test.

> **Tried, and refused, within the hour.** This session attempted the dispatch
> — `POST /repos/howlerops/slate-orm/actions/workflows/mutations.yml/dispatches`
> — and got `403 Resource not accessible by integration`. The token this
> container holds can read runs and logs and cannot start one. So the button
> works for a person on any branch and does not work for the agent that wrote
> it, which is a sharper version of the caveat below rather than a softer one:
> the workflow is untested *and* the party most likely to test it cannot.

## What this does not do

**It has never fired, and the scheduled half cannot until this is on `main`.**
Written into the file rather than only here, because the file is what somebody
reads when they wonder why no run exists. The honest state is: a button that
works and a schedule that is waiting on a merge.

**Nothing checks this workflow's steps the way `test_check_sh.py` checks
`ci.yml`'s.** That guard's rule — every step is in `scripts/check.sh` or
written down as one it cannot run — is scoped to one file, and a second
workflow is outside it. Both steps here happen to be accounted for
(`mutations-roster` is in `check.sh`; `run_mutations.py` is deliberately not),
but by coincidence rather than by a rule.

**A weekly cadence means a broken guard can be broken for a week.** The ones
the survey found had been broken for days to weeks already, so this is an
improvement of the same order rather than a different one. Per-push would be
better and costs a minute on every push; that trade is the entry's subject and
could be revisited if the survey ever finds something a week old that mattered.

**`git diff --exit-code` is a belt, not a brace.** It catches a mutation left
behind by a killed run *within the same job*. A run killed by the runner itself
leaves nothing to check, which is what `mutate.py`'s in-flight marker is for,
and that marker is only read by the next `mutate.py` on the same checkout —
never the case in CI, where every run starts clean.
