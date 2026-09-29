# Fourteen checks ran only on a laptop, and the rule that would have said so read one direction

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `.github/workflows/ci.yml`, `scripts/test_check_sh.py`,
  `scripts/check_closed_caveats.py`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

**Fourteen steps that `scripts/check.sh` runs were in no workflow at all.** They
are in `ci.yml` now:

- nine repository guards — `check_client_identity.py`,
  `check_mutation_claims.py`, `check_retired_claims.py`, `check_write_paths.py`,
  their four test suites, and `test_free_ports.py` — in the `scripts` job;
- `gofmt -l .` in `clients/go` and in `examples/explorer/backends/go`, wrapped
  because it exits zero whatever it finds;
- `tsc --noEmit` in `clients/typescript` and in
  `examples/explorer/backends/node`;
- `go vet ./...` in `examples/explorer/backends/go`.

And `scripts/test_check_sh.py` now reads the roster **both** ways. It held every
`ci.yml` step to the script and nothing held the script to CI; `ONLY_LOCAL` is
the mirror of `ELSEWHERE`, and it is empty, because after this change there is
nothing to put in it.

Two caveats close on it, including one from a different entry that turned out to
be stale by reading.

## Why

`ledger/2026-09-29-everything-this-repository-ships-can-now-be-published.md`
recorded the gap precisely and did not measure it:

> `test_check_sh.py` reads one direction — every workflow step is in the script
> or rostered as one it cannot run — and nothing reads the other. Found the same
> way the line above was: `check_versions.py` was added to `check.sh` and not to
> `ci.yml`.

One step found by hand reads as an oversight. Fourteen is a different thing, and
it is the failure CLAUDE.md names in its own words: **a check that never fires
is a check nobody has debugged.** These fired, but only when somebody ran
`check.sh`, which is a habit rather than a gate — nothing on a push consulted
them, and a branch could go green with any of the fourteen red.

**The sharpest case is `check_mutation_claims.py`.** It caught a real defect
earlier the same day: a ledger entry counting mutations and citing no run. It
caught it locally, because CI was not running it. Had that entry been written by
a session that skipped `check.sh`, it would have merged with its evidence
uncheckable and nothing would have said so.

`gofmt` is the second: **no job checked Go formatting at all.** The `fmt` job is
Rust, `go vet` does not format, and the only `gofmt` in the repository was the
shell function in `check.sh`.

The reason the comment in `main` gave was true and had stopped being
proportionate. It said the script running checks CI does not is "allowed and is
the point of having it", and named four — the two `tsc` runs, `gofmt`, the
demo's `go vet`. All four are in the list above: the sentence describing the
allowance was also the complete inventory of it, and then nine more arrived
under the same words without anybody re-reading them.

## Alternatives rejected

**Leave the five toolchain checks local and wire only the nine guards.** What
the old comment would have advised, and defensible for `tsc`: `npm test`
compiles what the tests import, so the marginal coverage is types in unexercised
exports. Rejected on `gofmt`, which is not marginal — it is the only formatting
check Go has anywhere, and a misformatted client would reach `main` green today.
Once the wrapper is written for one directory the second is a copy, and leaving
`tsc` out while adding `gofmt` would need a reason neither has.

**Make `ONLY_LOCAL` a blanket allowance with a note, rather than a roster.**
Half the work and none of the value: an unbounded allowance is what let fourteen
accumulate behind four names. The roster is empty and stays honest for the
reason `ELSEWHERE` does — a list you are forced to edit is a list that stays
true, which is the `EXPECTED_REFUSALS` idiom already used in three places here.

**Match CI against `check.sh` on a substring, so the `gofmt` wrapper needs no
entry.** Tempting: `test -z "$(gofmt -l .)" || …` contains `gofmt -l .`. Rejected
because the same rule would let a CI step running `go vet ./... --some-flag`
satisfy a script step running `go vet ./...`, which is a different check. That
is the false-positive shape `check_cited_tests.py` measured at 87% and rejected,
so the two spellings are written out in `SPELLED_IN_CI` — two entries, both
`gofmt`, and the next one has to be added deliberately.

**Move `gofmt` into the `fmt` job beside `cargo fmt`.** Reads better — one job
for formatting — and costs a Go toolchain in a job that installs none, to check
files the `go` job has already checked out. Put where the toolchain is instead.

## Evidence

The count was derived, not eyeballed: `script_checks()` against
`workflow_steps()` over all five workflows returned **fourteen** pairs in the
script and in no workflow. Each of the nine Python ones was then confirmed by
name — `grep -c` over `.github/workflows/*.yml` answered **0** for all nine.

`python3 scripts/test_check_sh.py`: **9 passed, 0 failed**, from 8 checks before,
reporting `148 steps, 16 blocks and 2 env vars, all accounted for`.

`sh scripts/check.sh`: exit 0, **87 passed, all of them**.

Three mutations against `ci.yml`, judged by the guard, all caught by the new
check and by it alone:

| mutation | caught by |
| --- | --- |
| a guard dropped from CI, still in `check.sh` | `every check.sh step runs in a workflow or is in ONLY_LOCAL` |
| the client's `tsc --noEmit` dropped from CI | the same |
| the Go client's `gofmt` wrapper dropped from CI | the same |

Records: `ledger/mutations/20260929T193148-github-workflows-ci-yml.json` is the
scoring run, `restored_clean: true`. The one before it,
`ledger/mutations/20260929T193133-github-workflows-ci-yml.json`, is
`baseline-reported-nothing` — the first spec pointed `mutate_guard.py` at
`test_check_sh.py` rather than `scripts/test_check_sh.py`, and the adapter
refused to call a guard it could not find rather than scoring three survivors.
That is the second of `mutate.py`'s six lies being caught by the thing written
to catch it, and it is left in the record because a run that scored nothing is
worth as much to a later reader as one that did.

The third is the one worth having: it deletes the *CI spelling* of a step whose
`check.sh` spelling differs, so it also exercises `SPELLED_IN_CI` — a mapping
that silently stopped matching would make the rule blind to exactly the two
steps it was written around, and would look like a pass.

**The teardown caveat was stale, and CI said so.**
`2026-09-29-the-teardown-test-asked-for-the-wrong-mode.md` :: *"It has never run
in CI"* — the demo job's last step is `python3 scripts/test_run_teardown.py`,
unconditional, and run 501's `three SDKs, one database` job reports that step as
`success`. Checked against the run's step list rather than the job's conclusion,
because a job passes whether a step ran or was skipped.

## What this does not do

**It does not prove the fourteen were passing.** They pass here and `check.sh` is
green, but nine of them have never run on CI's kernel, CI's Python or CI's Go,
and the toolchain-gap notes in CLAUDE.md are four separate instances of a check
that was green here and red there. The first CI run after this change is the one
that says; if it is red, that is the finding rather than a regression.

**`ONLY_LOCAL` being empty is a fact about today.** It is the right shape and it
has no entries, so nothing exercises the branch that reads a reason out of it.
The stale-roster half is tested by `SPELLED_IN_CI`, which does have entries; the
`ONLY_LOCAL` half is a rule with no data.

**Nothing checks the third direction.** A guard that exists in `scripts/` and is
in neither `check.sh` nor a workflow is invisible to both rules — they compare
two lists to each other and never to the directory. That is how a guard written
and never wired would hide, and `check_mutations_roster.py` is the closest thing
to a rule about it.

**The `gofmt` wrapper is duplicated.** `check.sh` has a shell function and
`ci.yml` has a one-liner, and `SPELLED_IN_CI` records that they correspond
rather than that they agree. A reader changing one has to change the other, and
the only thing that would notice is the exact-string match going stale.
