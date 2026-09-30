# `mutate.py --help` lists the adapters beside it, derived rather than written down

- **Date:** 2026-09-29
- **Author:** Claude Code, task K4
- **Touches:** `scripts/mutate.py`, `scripts/test_mutate.py`
- **Kind:** fix

## What changed

`python3 scripts/mutate.py --help` now prints an **Adapters** section under the
dialect table: every `scripts/mutate_*.py` beside it, with its docstring's first
line, globbed rather than listed. `scripts/test_mutate.py` gains
`case_help_lists_every_adapter`, which asserts in both directions — the help
names exactly the files that exist.

## Why

`ledger/2026-09-29-the-dialect-mutate-py-was-missing.md` added
`scripts/mutate_guard.py` and recorded the hole in the same entry:

> **It is a runner, not a dialect, so `mutate.py --help` does not list it.** …
> A session looking for "can I mutate against a guard" will find the answer in
> this entry and in `mutate_guard.py`'s docstring, and nowhere the tool prints
> — which is exactly the staleness `mutate.py`'s docstring argues against, in
> the one place it cannot fix itself.

That docstring's argument is unusually well evidenced, because the thing it
warns about already happened here: the hand-written dialect list stopped at
`pytest` while `node` and `go` were added under it, and a session read it,
concluded `node` was unsupported, and **wrote a throwaway harness
reimplementing four protections against a dialect that had been here for
weeks**. The fix then was to generate the list from the table. `mutate_guard.py`
put the same failure one level out: a capability that exists, works, and is
printed nowhere.

A glob rather than a constant, for the reason the dialect list is generated:
a hand-written `ADAPTERS = ("mutate_guard.py",)` would be correct today and
would go stale on the second adapter, which is the exact history above.

## Alternatives rejected

**A constant naming `mutate_guard.py`, with the test comparing it to the
directory.** This is what the roster guards in `scripts/` do, and the idiom is
deliberate there — "a list you are forced to edit is a list that stays true"
buys a *reason* per entry, which is worth the maintenance. There is no reason
to write here: an adapter's docstring already says what it is for, and the only
fact the list adds is that the file exists, which the filesystem knows better.

**Printing every `scripts/*.py`.** The help would then name `caveats.py` and
twenty-nine guards, none of which is a thing to pass as `command`. The
`mutate_` prefix is the convention that already exists, and the test holds the
directory to it from both sides.

**Leaving it to the entry.** That is the state this closes, and the entry that
created it argued against itself: the answer lived in a ledger file and in a
docstring nobody prints.

**Teaching `--help` to import each adapter and print its `--help`.** Richer and
worse: importing a program to describe it runs its module-level code, and the
first line of a docstring is what a reader needs to decide whether to open the
file.

## Evidence

- Before: `python3 scripts/mutate.py --help` ended at the five-dialect table.
  After, it ends with `mutate_guard.py  Run repository guards and report in a
  shape scripts/mutate.py can read.`
- `python3 scripts/test_mutate.py`: **75 passed, 0 failed** (74 before).
- **Mutation**, `scripts/mutate.py`, record
  `ledger/mutations/20260929T135542-scripts-mutate-py.json`: two cases, two
  caught, both by `--help lists exactly the adapters beside it`.
  - the glob changed to `mutates_*.py`, so the list goes empty and the section
    is skipped → caught. This is the direction that matters: a silently empty
    list is the failure the whole entry is about.
  - an entry appended for a file that does not exist → caught, which is the
    other direction and is why the case compares sets rather than checking
    that each file is mentioned.
  Both are changes: one alters the glob pattern, the other appends a tuple.

## What this does not do

**One adapter exists, so the list has one row.** Everything above is about the
shape rather than the content, and a one-row generated list and a one-row
hand-written list print the same thing today. The difference only shows on the
second adapter, which is precisely when the hand-written one would have been
wrong — but that is an argument, not a measurement, and this entry has not got
a second adapter to demonstrate it on.

**The docstring's first line is whatever the adapter's author wrote.** Nothing
holds it to a length or a shape, so a 200-character first line would wrap
badly. `mutate_guard.py`'s fits; a rule for the next one is not written.

**`--help` still does not say which dialect an adapter targets.**
`mutate_guard.py` reports in the `python` dialect, and a reader has to open it
to learn that. Printing the pairing means a declared attribute in each adapter,
which is a convention with one participant.
