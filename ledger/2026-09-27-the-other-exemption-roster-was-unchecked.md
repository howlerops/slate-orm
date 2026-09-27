# `FIXTURES` exempts seven whole files and nothing checked that any of them still had a citation to exempt. The commit that gave `NOT_A_FILE` a never-fires half recorded this as the gap it left.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_cited_docs.py`, `scripts/test_check_cited_docs.py`, `docs/caveat-status.json`
- **Kind:** correctness

## What changed

`check_cited_docs.py` reports a `FIXTURES` row whose file is gone, and a row
whose file no longer contains any `docs/…` or `ledger/…` path at all. Both
rosters in that file now have a never-fires half.

`check` and `source_files` take `fixtures` as an argument for the same reason
`check` already took `roster`: these rows name real files, so over a fixture
tree every one would report.

## Why

`ledger/2026-09-27-a-ledger-entry-could-cite-anything.md` gave `NOT_A_FILE` a
never-fires half and wrote down what it left:

> **`FIXTURES` still has no never-fires half.** `NOT_A_FILE` reports a row
> whose citation is gone; the older whole-file roster does not, so a file
> listed there that stopped containing a fixture path keeps its exemption
> forever. It hides less than a path-level row does — the file is still named
> and still has to be justified — which is why it is recorded rather than
> fixed here.

"Hides less" was the argument for deferring and it is the wrong way round. A
`NOT_A_FILE` row exempts one string in one file; a `FIXTURES` row exempts
**every citation in the file, forever**, including ones written after the
exemption. A test file that grows a real `ledger/…` reference in a comment
next year gets it unchecked, and the reason in the roster — "writes a
temporary tree containing `docs/d.md`" — still reads as true.

## Alternatives rejected

**Delete `FIXTURES` and roster each fixture path in `NOT_A_FILE`.** The
narrowest thing, and it would mean enumerating every invented path each test
writes — `test_check_cited_docs.py` alone writes a dozen and they change with
every case added. A roster somebody must edit to add a test case is one that
gets a glob added to it. The two rosters exist because the two situations are
different, which is now spelled in the code.

**Check that the file still contains the *specific* path the reason names.**
Sharper, and it would have to parse a sentence: the reasons say things like
"`docs/d.md` and `ledger/e.md`" and "invented `docs/…` paths". Extracting a
path from prose to check the prose is the shape that makes a guard's own
correctness the thing in doubt.

## Evidence

**Two ways a row goes dead, and the second is the quiet one.** A file that is
gone fails loudly the moment anybody looks; a file that is still there,
still readable, and has stopped containing any citation is a row whose reason
still sounds right. Both have a named case.

**The roster is clean today.** All seven rows point at files that exist and
still contain a citation-shaped string, so the new rule fires on nothing — a
guard against the next edit, not a fix for a present defect. Stated plainly
because the previous two guards in this file each found something and this one
did not.

**The *other* half of the file fired on this entry, in the run that added it.**
Quoting a roster reason back — "writes a temporary tree containing `docs/d.md`
and `ledger/e.md`" — put two unresolvable paths in prose, and
`check_cited_docs.py` failed `check.sh` on them. That is the third time in two
days a session has been caught by a guard it was in the middle of writing, and
the pair is now rostered in `NOT_A_FILE` with that as the reason. It is not
evidence for the new rule; it is evidence that keying the roster on
`(file, citation)` rather than on the file was right, because this entry's
other citations are claims and are still checked.

**Mutations.** Three runs, six cases, no survivors:
`ledger/mutations/20260927T011253-scripts-check-cited-docs-py.json` (four
cases, three caught and one that could not be scored),
`ledger/mutations/20260927T011307-scripts-check-cited-docs-py.json` (that one
again, with a single-line anchor, which made no difference — the cause was the
mutation, not the anchor), and
`ledger/mutations/20260927T011339-scripts-check-cited-docs-py.json` (the
observable version of it: caught).

| mutation | outcome |
| --- | --- |
| the missing-file branch reports nothing | caught |
| a row whose file lost its citation is not reported | caught |
| the fixtures roster falls back to the real one over a fixture tree | caught, 4 cases |
| a named fixture file is no longer excluded from the scan | caught, 3 cases |

**One mutation could not be scored, and `mutate.py` said so rather than
calling it a survivor.** `if not path.is_file():` → `... and False:` makes the
next line read a file that is not there, so the suite dies on an uncaught
`FileNotFoundError` before printing its summary. That is the second of the six
lies the script's docstring lists — "nothing runs" reading the same as "nothing
failed" — and it exited non-zero with `NOTHING RAN` instead. The `continue`
under that branch is load-bearing for exactly that reason, so the mutation that
*is* observable deletes the branch's body and keeps the `continue`; that one is
the first row above.

**Suites.** `scripts/test_check_cited_docs.py` 23 passed 0 failed (was 20).
`sh scripts/check.sh` 69 of 69.

## What this does not do

**It does not check that a `FIXTURES` reason is true.** The row says why the
file's paths are fixtures; the guard checks only that there are still paths.
A file whose reason has become wrong — it stopped writing a temporary tree and
now cites a real entry — passes both halves. Same limitation `NOT_A_FILE` has
and the same one every roster here carries.

**It fires on nothing today, so its own never-fires half is the only evidence
it works.** Four mutations and three fixture cases, no real-tree finding. That
is a weaker kind of evidence than the two guards before it produced, and it is
the honest description of a guard written against a clean roster.
