# `mutate.py` supported two dialects I wrote scratch harnesses around, because its `--help` had not been updated.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #261 (F5g)
- **Touches:** `scripts/{mutate.py,test_mutate.py}`, `examples/explorer/conformance/conformance.py`, two ledger entries
- **Kind:** fix

## What changed

`mutate.py --help` now prints the dialect table instead of a sentence that
listed three of five, and `scripts/test_mutate.py` asserts the printed list is
exactly the table's, patterns included.

The three-SDK conformance runner prints `FAIL  <what>` per finding and a
closing `N passed, M failed` beside its own summary line, which is the house
style `mutate.py`'s existing `python` dialect already reads. No new dialect.

Two ledger entries from earlier today are corrected: one claimed `mutate.py`
refused a run it was never given.

## Why

Twice in one session a mutation run could not be scored, and twice I wrote a
scratch harness reimplementing the four protections `mutate.py` exists to
provide. One of those two times was avoidable and my fault in an instructive
way: the script has had a `node` dialect for weeks, and its docstring stops at
`pytest`. I read the docstring, concluded the suite was unsupported, and wrote
the harness — then recorded in a ledger entry that "it refused the run rather
than guessing, which is correct", which is a claim about a run that never
happened.

So the defect is not that `mutate.py` lacked a dialect. It is that **the list
of what it supports was maintained by hand in prose**, and a hand-maintained
list of a tool's capabilities is the one thing certain to be wrong at the
moment someone needs it. `DIALECTS` had five entries and the docstring named
three; both additions that went unmentioned were made by people who tested
their dialect and did not think to edit a sentence twenty lines up.

The conformance runner is the other half. That refusal *was* real — its output
matched nothing the script knew — and the fix is the one `test_codegen.py`'s
entry already settled for the identical problem: print the house format, do not
add a reader. A runner that decides whether three SDKs agree, and which nobody
can mutation-test, is a runner whose own correctness is taken on trust.

## Alternatives rejected

**Add a `conformance` dialect.** The obvious move and the wrong one twice over.
It grows the table for a one-off format, and the format was the one-off: every
other suite here prints `FAIL name` and `N passed, M failed`. Worse, the
failure pattern would have had to exclude `run.sh`'s startup chatter —
`starting the head node on 127.0.0.1:…` is an unindented line that any
"top-level line in the failure block" regex matches — so the dialect would have
been the kind of regex that is correct until the preamble changes.

**Add a TAP dialect.** It was already there. That is the entry's point.

**Leave the docstring and just add the two missing names.** The same fix the
last two dialect authors did not make, for the same reason, with the same
half-life. A list that has gone stale twice will go stale a third time.

**Keep the runner's `130 cases: …` line only.** It says something the counts do
not — that these are *cases compared across three SDKs* rather than tests — so
it stays, with the machine-readable line beside it rather than instead of it.

## Evidence

The frontend mutations, re-run through `mutate.py` proper rather than the
scratch harness: `{"dialect": "node", "command": ["npm", "--prefix",
"examples/explorer/web", "test"]}` catches both, each named
`every view the UI offers reads a table, with that table's columns`. The
`--prefix` matters and is the reason the first attempt failed — `mutate.py`
runs from the repository root by design, so a suite that lives in a
subdirectory is named rather than `cd`-ed to.

The conformance mutation, likewise: the Node client's view declaring
`{ ...BOOKS }` without the view's name is caught, and the script names all
three disagreeing cases and the must-differ pair that stopped being
comparable — a better report than the scratch harness gave.

`scripts/test_mutate.py`: 18 passed, one new. Five mutations against the help
listing, all caught after the case was fixed twice:

- dropping `go` from the loop → caught (missing `['go']`);
- printing a `tap` row the table does not hold → caught (invented `['tap']`);
- printing `^` for every pattern → **survived the first version**, because it
  only asserted the pattern was non-empty. A help text listing five names and
  five wrong patterns is worse than one listing nothing, since it reads as
  specific. The case now compares each printed pattern against
  `DIALECTS[name][1].pattern`;
- printing the *failure* pattern where the report marker belongs → caught by
  that same comparison;
- and the first draft of the case parsed "an indented line with a space in it",
  which matched the docstring's own JSON example and made `{"name":` a
  dialect. It could never have asserted the equality it claimed to. Anchored on
  `^    \w+\s+\^` now.

`sh scripts/check.sh`: 34 passed. `python3 site/check/docs.py`: green.
`examples/explorer/run.sh --conformance`: 130 cases, the three SDKs agree, and
the new summary line reads `130 passed, 0 failed`.

## What this does not do

**It does not check that a dialect's patterns are right**, only that `--help`
shows the ones the table holds. A dialect whose report marker matches nothing
is still possible; the existing per-dialect cases are what guard that, and each
was added after a real misreading rather than in advance.

**It does not give `mutate.py` a working directory.** A suite in a
subdirectory has to be named from the root — `npm --prefix …`, `go test ./…/…`
— which works for every suite here and is why nobody has needed one. A spec
with `cwd` would be three lines; it is not here because adding an option for a
case that has not come up is how options accumulate.

**The conformance runner's `passed` count is cases minus findings**, and a
single case can produce more than one finding only through the must-differ
pairs, which are not cases. So the count is right today and would quietly drift
if a case ever appended two findings. Nothing asserts it.

**Two scratch harnesses were written and are not in the repository.** They
lived in the session's scratchpad and are gone. Neither was wrong — both
implemented the four protections faithfully — but a protection reimplemented
per session is a protection nobody maintains, which is the argument this entry
is about, turned on itself.
