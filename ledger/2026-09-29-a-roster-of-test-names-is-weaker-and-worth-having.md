# A roster of renamed-column tests, and the file-wide search it started as

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-the-python-client-had-no-renamed-column-to-declare.md`
- **Touches:** `scripts/check_renamed_column.py`,
  `scripts/test_check_renamed_column.py`, `scripts/check.sh`,
  `.github/workflows/ci.yml`, `scripts/check_closed_caveats.py`,
  `docs/caveat-status.json`
- **Kind:** process

## What changed

A guard that every client under `clients/` has a test declaring a column under
a name the catalog has renamed away. The roster of clients is read off the
directory rather than written out, so a fourth SDK is caught by existing; the
roster of *tests* is written out, three entries, each naming the file, a
pattern matching the test, a pattern matching the old spelling inside that
test's body, and where the catalog says the name is an old one. Two steps went
into `scripts/check.sh`, which now lists 80 — one for the guard, one for its
tests — and the same pair into `ci.yml`'s `guards` job.

## Why

`2026-09-29-the-python-client-had-no-renamed-column-to-declare.md` left it
open, and named the objection to closing it:

> Nothing stops the fourth client arriving without this test. Three suites
> have it now, by hand, and the roster is three files nobody compares. The
> transport-door guard added this morning is the shape that would fix it and
> it checks a different property; extending it to "every client has a rename
> test" would be a roster of test names, which is a weaker thing than a roster
> of doors.

That is right and it is still a roster of test names. A door is a feature: the
guard sees the parameter and sees something walk through it, and a client
without one is broken in a way a reader can point at. A renamed column is not
a feature of any client — the server enumerates every spelling the catalog
accepts and compares, so a client passes by doing nothing at all. There is
nothing in the client to point at. That is exactly why the property is lost
silently, and why the only thing left to roster is the test.

The cost of not having it is on the record twice. `2026-09-14` wrote down that
Go and TypeScript *lacked* this property; both had it, nobody had run it, and
the sentence stood for two weeks. A client that has to do nothing to be correct
is a client nobody checks.

## Alternatives rejected

**Extend `check_transport_door.py` instead of a second guard.** One roster of
`clients/`, one discovery, one never-fires rule. Rejected because the two
guards fail for unrelated reasons and would have shared a green/red bit: a
missing rename test would have turned the transport-door step red, and whoever
read that would go looking at channels. The discovery *is* duplicated, and
deliberately: `MANIFESTS` is copied verbatim with a comment saying why the two
lists must not drift, because two roster guards over the same directory that
disagree about what a client is would let a fourth SDK be caught by one and
skipped by the other — and the one that skipped it would stay green.

**Check the feature rather than the test.** The honest version would start a
server, declare a table under an old spelling from each client, and assert it
is served. That is the conformance runner's job and it needs a built binary, so
it cannot run in the `guards` job, which is the one that costs seconds. A
conformance case would be better and is a larger change; this is the cheap
thing that catches the arriving client, and the entry says so.

**Roster only the test's name, not what it declares.** Half the work, and it
would accept a test renamed to keep the guard quiet with its body gutted. The
`declares` pattern is what makes the roster point at a test that still does
something, and the `renamed` pattern is what stops the *catalog* quietly making
the "old" spelling the current one — at which point the test passes for
nothing.

**Search the whole file for the declaration.** This is what the first draft
did, and it was wrong; see below.

## Evidence

`python3 scripts/check_renamed_column.py` reports
`ok    3 clients, each with a test that declares a renamed column's previous
name and is served`. `python3 scripts/test_check_renamed_column.py` reports
`15 passed, 0 failed` over a temp tree; `python3 scripts/test_check_sh.py`
reports `119 steps, 6 blocks and 2 env vars, all accounted for`.

**The first draft's `declares` rule was checking nothing, and a mutation
against the real tree is what said so.** The fifteen fixture cases all passed
on the first run. Breaking the real clients' tests by hand then found that the
patch could not be applied at all: `Column("comment", ValueType.STR)` occurs
**twice** in `clients/python/tests/test_fixture.py` and
`{ name: "category", type: "string" }` **twice** in
`clients/typescript/test/schema.test.ts`. The second occurrence is a different
test in each case — `test_the_two_spellings_hash_differently`, which is about
hashing, and `a wrong column name is refused`, which is about a name `docs`
never had. The rule searched the file, so cutting the real declaration out of
the real test left it green. Rewritten to search from the test's declaration to
the start of the next one, all three real-tree mutations are now refused, each
naming the client and the pattern:

```
python's renamed-column test no longer declares an old spelling:
  no `Column\("comment", ValueType\.STR\)` between that test and the next one.
typescript's … no `\{ name: "category", type: "string" \}` between that test …
go's … no `\{Name: "category", Type: slate\.TypeString\}` between that test …
```

Those three were run by hand, one line replaced at a time and restored, not
through `scripts/mutate.py`: the guard prints no line any dialect reads as a
suite result, so a run against it would have hit the "no test results at all"
error rather than scoring. That is a real limitation of this evidence — the
`finally`-restore and the exactly-once check were mine to get right, and `git
diff -- clients/` was clean afterwards, which is the only thing checking them.

The guard's own rules went through `scripts/mutate.py` properly:
`ledger/mutations/20260929T074913-scripts-check-renamed-column-py.json`, 17
cases, **no survivors**. That is the run against the file as committed;
`ledger/mutations/20260929T074122-scripts-check-renamed-column-py.json` is the
same 17 cases against the same rules before `ruff format` rewrapped two lines,
re-run because a reformat is exactly what moves an anchor out from under a
patch. Four earlier runs are on record and each ended in a finding rather than
a pass:
`ledger/mutations/20260929T073457-scripts-check-renamed-column-py.json` (12
cases, the draft before body scoping),
`ledger/mutations/20260929T073757-scripts-check-renamed-column-py.json` (12
cases, one survivor — the body's start boundary),
`ledger/mutations/20260929T073937-scripts-check-renamed-column-py.json` (13
cases, one survivor — `test\(` as a boundary), and
`ledger/mutations/20260929T074029-scripts-check-renamed-column-py.json` (4
cases, one survivor — the column-zero anchor). Each survivor bought a case:

- **`text[at.start():]` versus `text[at.end():]`** for where a body begins.
  A real change, and unobservable: no pattern in the roster overlaps a test's
  own declaration line, so both slices behaved identically against every case.
  Resolved by taking the *safer* of the two — starting after the declaration
  means a `declares` pattern cannot be satisfied by the test's own name — and
  writing the case that tells them apart, which points `declares` at
  `RenamedColumn`, a substring of
  `TestARenamedColumnIsAcceptedUnderItsPreviousName`.
- **`test\(` dropped from `BOUNDARY`** survived because the one
  declaration-in-another-test case was written in Python, whose boundary is
  `def `. The case is now generated per client from `PROVES`, so each of the
  three languages' boundary keywords has a case, and dropping any one of them
  is caught.
- **The `^` anchor on `BOUNDARY`** survived because no fixture had an indented
  `def ` inside a test body. That one is a false *refusal* waiting — a helper
  defined inside a Python test is ordinary, and without the anchor it would
  truncate the body and fail a client that is perfectly correct. The case that
  pins it is the only one here that expects exit 0.

One line was deleted rather than tested: `re.M` on the `re.search` in
`missing()`. None of the nine roster patterns uses `^` or `$`, so the flag
could not change an answer — an inert flag, not a protection, and mutating it
would have produced a survivor meaning nothing.

## What this does not do

**It checks that a test exists and what it declares, never that it passes.**
A test can be `t.Skip`-ed, commented out inside its body, or asserting the
wrong thing, and this guard sees a matching pattern and says ok. The client
suites are what run it; this only stops the test going missing.

**`BOUNDARY` is a heuristic, not a parser.** It is the union of six
declaration keywords at column zero. A test whose body contains a top-level
`func ` — a Go closure written flush left, a Python module-level statement
inside a triple-quoted string — would truncate early, and the guard would
refuse a correct client. No case in the tree does, so the failure mode is
unobserved rather than ruled out.

**The three real-tree mutations were hand-run.** `scripts/mutate.py` cannot
read this guard's output, so the protections in that script — the restore in a
`finally`, the reported-suite count, the JSON spec that never reaches a shell
— did not apply to the three checks that matter most. Giving `mutate.py` a
dialect for a guard that prints `ok    …` on success and problems on stderr
would fix it for every guard in `scripts/`, and is not done here.

**The roster of test names is still weaker than a roster of doors, and the
objection the previous entry raised stands.** Nothing here proves a client
*serves* a renamed column; a conformance case against a live server would, and
would cost a built binary in a job that currently needs none.

**`NEXT_TEST` in the test file is a fourth hand-maintained per-client list.**
A fourth client with a `PROVES` entry and no `NEXT_TEST` line raises rather
than silently generating no case — the fixture's exception is reported as exit
70 — but it is one more thing to remember, which is the shape this whole guard
exists to avoid.
