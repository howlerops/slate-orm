# The real-tree mutations run again now, and the roster says which guards have none

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-a-scope-claim-that-understates-reads-as-exhaustive.md`
- **Touches:** `scripts/mutations.json`, `scripts/run_mutations.py`,
  `scripts/check_mutations_roster.py`,
  `scripts/test_check_mutations_roster.py`, `scripts/check.sh`,
  `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** process

## What changed

`scripts/mutations.json` holds every real-tree mutation this session found:
seventeen suites, one per guard, each a change to a file that guard watches and
the reason it must be refused. `python3 scripts/run_mutations.py` runs them all
— **17 suites clean, 0 with findings**, in about a minute.

`scripts/check_mutations_roster.py` checks the roster is complete: every
`scripts/check_*.py` has a suite or a written sentence saying why it has none.
Eleven have the sentence. It runs in `check.sh` and in `ci.yml`, because it
reads files and changes nothing; the runner breaks the tree and puts it back,
so it deliberately does not.

## Why

Three entries in a row recorded the same gap and it got worse each time:

> **Nothing makes the real-tree mutations run again.** The five above are
> recorded and were run once… the next change to a client's renamed-column test
> will not re-run them.

Eleven by the second entry, seventeen by the fourth, all of them history in
`ledger/mutations/` and none of them a check. That matters more here than for
an ordinary test, because what these mutations establish is the one thing a
guard's fixture tests cannot: that the guard is *pointed at the real tree at
all*. `check_toolchain_pins.py` passed its own ten fixture cases while its
roster was its own docstring.

The second half is the survey's other outcome. Eleven guards still have no
real-tree mutation, and "not yet" was living in a ledger caveat where it
degrades into a number nobody re-derives. `unmutated` puts it in the roster,
one sentence per guard, checked from both sides: a guard in neither list fails,
and a guard in both fails.

## Alternatives rejected

**Run the mutations in CI.** They break the working tree between the patch and
the restore, which is fine in a container CI throws away. Rejected for a
different reason: it is a minute of wall clock on every push to catch a class
of defect that changes when a *guard* changes, not when the code it watches
does. A scheduled run, or a run after touching `scripts/`, is the right cadence
and `run_mutations.py` takes a guard name so that is one command.

**One roster file per guard.** Seventeen small files, no shared-file conflicts
between sessions — the reason `ledger/` is one file per entry. Rejected: the
completeness rule has to read all of them anyway, and a roster whose point is
"every guard is in here" reads worst as a directory you have to list.

**Skip `unmutated` and just fail on the eleven.** Honest, and it would have
stopped this entry landing. The eleven reasons are real — two of them are
"mutating this file races another session's work", one is "the mutation would
put a deliberate falsehood in the ledger for the length of a run" — and a rule
with no way to say that is a rule that gets switched off.

**Put the mutations in `mutate.py` itself.** It takes one spec on stdin and
restores in a `finally`; a roster of seventeen is a caller's job, and keeping
it out leaves that script's six protections unentangled with a file format.

## Evidence

`python3 scripts/run_mutations.py` reports **`17 suites clean, 0 with
findings`**. Each names the guard's own refusal, so the run is readable as a
list of what each guard is actually for:

```
check_renamed_column: python's renamed-column test no longer declares an old
  spelling: no `Column\("comment", ValueType\.STR\)` between that test and the next one.
check_handlers: service.rs:2654: `join` calls `join_from_proto` with no
  authorisation in the 4 lines above it.
check_secret_types: `Credentials` at crates/slate-slatedb/src/s3.rs:57 derives
  `Debug` and holds a secret.
check_toolchain_pins: clients/go/scripts/generate_proto.py runs a pinned
  `go install` (…) and never sets `GOTOOLCHAIN`, while .github/workflows/ci.yml
  pins `go-version`.
```

The seventeen records, one per suite:

- `ledger/mutations/20260929T084515-clients-python-tests-test-fixture-py.json`
- `ledger/mutations/20260929T084515-docs-correctness-md.json`
- `ledger/mutations/20260929T084516-clients-go-scripts-generate-proto-py.json`
- `ledger/mutations/20260929T084521-docs-performance-md.json`
- `ledger/mutations/20260929T084530-scripts-check-cost-prose-py.json`
- `ledger/mutations/20260929T084534-crates-slate-server-src-service-rs.json`
- `ledger/mutations/20260929T084536-crates-slate-kernel-src-record-rs.json`
- `ledger/mutations/20260929T084537-crates-slate-slatedb-src-s3-rs.json`
- `ledger/mutations/20260929T084539-clients-go-slate-client-go.json`
- `ledger/mutations/20260929T084539-crates-slate-kernel-cargo-toml.json`
- `ledger/mutations/20260929T084540-cargo-toml.json`
- `ledger/mutations/20260929T084541-scripts-test-mutate-py.json`
- `ledger/mutations/20260929T084543-readme-md.json`
- `ledger/mutations/20260929T084547-clients-typescript-proto-slate-v1-records-proto.json`
- `ledger/mutations/20260929T084548-crates-slate-server-tests-multi-rs.json`
- `ledger/mutations/20260929T084552-clients-go-slate-client-go.json`
- `ledger/mutations/20260929T084553-docs-correctness-md.json`

`python3 scripts/check_mutations_roster.py` reports `ok    29 guards, 17 with a
real-tree mutation, 11 with a written reason for having none`.
`python3 scripts/test_check_mutations_roster.py` reports `12 passed, 0 failed`
over a temp tree — including the headline case (a guard with neither), both
kinds of roster rot (a suite for a guard that is gone, a guard with both a
suite and a reason), and the two anchor rules.

**The anchor rule caught something while being written.** The roster's
`check_proto_copies` suite anchored on `message NotTheSameName {`, which occurs
zero times in the proto — a name invented while drafting rather than read out
of the file. `scripts/mutate.py` would have errored on it, but only when
somebody ran that suite; the static rule said so immediately, which is the
argument for having a static rule at all. That is the first of the six lies
`mutate.py`'s docstring lists, caught by a check rather than by a reader.

`sh scripts/check.sh` reports `85 passed, all of them`.

## What this does not do

**Eleven guards still have no mutation; the roster records that, it does not
fix it.** `check_build_output` has a real reason — its subject is the *absence*
of a tracked artifact, which `mutate.py`'s replace-text-in-one-file patch
cannot express. The other ten say "deferred" with a specific obstacle, and
seven of those obstacles are "this session did not want to break that file
beside its other work", which is a scheduling excuse, not a technical one.

**Nothing runs `run_mutations.py` on a schedule.** It exists and is one
command; no workflow, no cron, no reminder calls it. The caveat this entry
closes was "nothing re-runs them", and what is true now is "one command
re-runs them" — weaker than the sentence it replaces if nobody types it. A
`workflow_dispatch` job, or a weekly `schedule:` trigger, is the shape that
would finish it.

**A suite's `why` is unchecked prose.** `check_mutations_roster.py` requires it
to be non-empty and can say nothing about whether the mutation breaks the
property the guard exists for rather than something incidental. Two mutations
were rejected during the survey for exactly that — renaming a function's
declaration while its call sites keep the old name, and lowering a Go version
under a guard whose invariant is about the pin existing — and both were caught
by reading, not by a rule.

**The eleven reasons will rot.** Nothing re-reads them. A guard whose obstacle
is gone keeps its sentence, and the honest count of "guards with no mutation
for a reason that is still true" is unavailable. A `reviewed` date per entry,
as `docs/caveat-status.json` carries, is the obvious fix and is not here.
