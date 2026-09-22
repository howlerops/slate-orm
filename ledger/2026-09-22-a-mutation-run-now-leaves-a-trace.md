# A defect in `mutate.py` is only as expensive as the runs it silently ruined, and that number was unknowable: specs are written per run, passed on stdin, and stored nowhere.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #287 (F6g)
- **Touches:** `scripts/{mutate.py,test_mutate.py}`, `ledger/README.md`, new `ledger/mutations/`
- **Kind:** closing the gap #285's audit actually found, rather than the one it first claimed

## What changed

Every `mutate.py` run writes one JSON file under `ledger/mutations/`:

```json
{ "at": "…", "file": "scripts/check_cost_prose.py", "dialect": "python",
  "command": ["python3", "scripts/test_check_cost_prose.py"],
  "commit": "11ff82f", "outcome": "clean",
  "cases": [{"name": "…", "verdict": "caught", "caught_by": ["…"], …}] }
```

**Including the runs that cannot score.** A `-q` run records
`"outcome": "baseline-unreadable"` and its command, which is precisely the
question #285 could not answer.

## Why

#285 found that `cargo test -q` suppresses the per-test lines the `rust`
dialect matches, so every mutation scored as a survivor. The fix took minutes.
The unanswerable part was *which earlier runs that had spoiled* — and the entry
written at the time guessed "entries #241 onward", was audited an hour later,
and had to be withdrawn. The guess was wrong and the mechanism for checking it
did not exist.

It does now. The next defect in this script has a blast radius that can be
read off a directory listing.

## Alternatives rejected

**An appended log.** `ledger/README.md` already states why not, for entries:
several agents work here at once and a shared file conflicts on every commit.
A mutation log would conflict harder — runs are more frequent than commits.

**Record only successful runs.** This is the tempting default and it would
have reproduced the exact failure. The `-q` run *scored nothing*; a record
written on success would have left the same silence behind. The record is
written in a `finally`, and there is a test whose entire job is the
unscoreable case.

**Have the author paste a block into the ledger entry.** `mutate.py`'s own
docstring argues against this at length about its dialect list: a list a tool
asks a human to keep in sync is a list that goes stale. The same applies to
evidence.

**Gitignore the records.** They would die with the container, which makes them
useless for the one purpose they exist for — auditing runs made in *past*
sessions.

**An opt-out flag.** Exploratory runs are exactly the ones that go
unrecorded by hand, so an opt-out reintroduces the gap for the cases most
likely to need it. The `MUTATE_RECORDS` environment variable redirects the
destination — which is how the test suite avoids writing dozens of fake runs
into the repository — but nothing turns recording off.

## Evidence

Four mutations against the recording itself, **all caught**: dropping the
unscoreable outcome, dropping the `finally`, dropping `caught_by`, and dropping
the command.

Two new cases in `test_mutate.py`, and the second is the one that matters — a
run whose baseline output the dialect cannot read must still produce a record
saying so. It reuses `QUIET_CARGO_FAKE`, the `-q` stand-in #285 already left
here, rather than a second copy of the same output.

Demonstrated end to end before any test was written: a fake printing exactly
what `cargo test -q` prints gave `outcome: baseline-unreadable`, with the
command, and zero cases.

**Two bugs in my own test code, both caught by the suite.** `FAKE` speaks the
rust dialect, so declaring `"dialect": "python"` made the run report nothing.
And `ok = (a and b and c)` yields the *last operand*, not a bool — `caught_by`
is a list, so `sum()` over the results died. A truthy chain is not a predicate;
it is `bool(...)` now.

The harness was changed to point `MUTATE_RECORDS` at a per-subject directory
for every case, not only the ones that ask. Without that, running the suite
would write dozens of records about `/tmp` subjects into `ledger/mutations/` —
a fixture dirtying the tree it tests, which is #281 pointed the other way.

`sh scripts/check.sh`: **47 passed, all of them.** Two real records are in this
commit, from runs made while building it.

## What this does not do

**It does not backfill.** Every run before this commit is still unrecorded and
unauditable. The `-q` question remains permanently unanswerable for them; what
changed is that the next one is answerable.

**Nothing checks that a ledger entry's mutation claims match a record.** An
entry can still say "four caught, none survived" with no run behind it, or
cite a run whose record says otherwise. The records make that *checkable* and
nothing checks it.

**A record is written by the script, so a hand-run mutation still leaves
none** — which is the behaviour `CLAUDE.md` already discourages, now with one
more reason.

**The directory grows without bound**, one file per run, committed. At this
session's rate that is roughly ten files a day. Nothing prunes them, and
whether that becomes a problem is a question for whoever meets it first.
