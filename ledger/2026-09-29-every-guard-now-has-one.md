# The survey finished: twenty-seven of twenty-eight

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-three-more-guards-and-a-substring-that-was-a-use.md`
- **Touches:** `scripts/mutations.json`, `docs/caveat-status.json`
- **Kind:** process

## What changed

The last seven guards got a real-tree mutation: `check_build_stamp`,
`check_demo_surface`, `check_site_claims`, `check_closed_caveats`,
`check_examples_roster`, `check_caveat_citations` and
`check_mutation_claims`. All seven caught theirs first time.

`python3 scripts/run_mutations.py` now reports **27 suites clean, 0 with
findings**. One guard has no mutation, for a reason that is not a schedule:
`check_build_output`'s subject is the *absence* of a tracked artifact, which
`mutate.py`'s replace-text-in-one-file patch cannot express.

## Why

The previous entry said the deferrals had a cost and offered one instance —
`check_site_css` was broken and would have stayed broken. Seven more were
cheap and the honest thing was to finish rather than leave a sentence saying
"not this session" in a file whose whole point is that nobody re-reads such
sentences.

The rate is worth recording plainly, because it is the argument for doing this
at all and it is now a complete count rather than a projection. Twenty-eight
guards, every one mutated against the real tree:

- **five were wrong**, in five distinct ways;
- twenty-three were right.

An 18% defect rate in code whose entire job is to be right about the tree, all
of it invisible to the guards' own fixture tests, all of it green in CI.

## Alternatives rejected

**Stop at twenty, and leave eight sentences.** The roster made "eight guards
have no mutation" a checked fact rather than a number in prose, which was the
point of building it, and there is a real argument that a checked backlog is a
finished piece of work. Rejected because six of the eight sentences said
"deferred for time" and the time was ten minutes.

**Give `check_build_output` a mutation anyway.** Its rule is that no build
output is tracked and every package root ignores its own; a mutation would
have to add a tracked artifact and edit a `.gitignore`, which is two files.
`mutate.py` patches one. Writing a two-file mutation mode for a single suite
is the tail wagging the dog, and its fixture tests cover the rules.

## Evidence

`python3 scripts/run_mutations.py` — **27 suites clean, 0 with findings**.
`python3 scripts/check_mutations_roster.py` — `ok    29 guards, 27 with a
real-tree mutation, 1 with a written reason for having none`. (Twenty-nine
counts `check_mutations_roster.py` itself, which is reflexive and exempt.)

The seven added here, each caught, each naming its own subject:

```
check_build_stamp: `cache` is a feature of slate-slatedb and the stamp does
  not report it. Add it to FEATURES in stamp.rs — a build difference the stamp
  omits is the defect #278 is about.
check_demo_surface: `/api/join` is served by the adapters and called nowhere
  in the UI.
check_examples_roster: `slate-slatedb` has 9 example(s) and a floor of 2.
check_mutation_claims: ledger/2026-09-29-the-mutations-run-again-now.md cites
  ledger/mutations/…-test-fixtures-py.json, which is not there.
```

The seven records:

- `ledger/mutations/20260929T090322-crates-slate-slatedb-src-stamp-rs.json`
- `ledger/mutations/20260929T090323-examples-explorer-web-src-api-ts.json`
- `ledger/mutations/20260929T090323-site-workbench-html.json`
- `ledger/mutations/20260929T090325-scripts-test-check-transport-door-py.json`
- `ledger/mutations/20260929T090329-scripts-run-examples-sh.json`
- `ledger/mutations/20260929T090330-docs-caveat-status-json.json`
- `ledger/mutations/20260929T090333-ledger-2026-09-29-the-mutations-run-again-now-md.json`

**The five defects the survey found, in order:**

1. **A class no guard covered.** A `docs/` page naming a source *file* that had
   been renamed away was seen by nothing — `check_cited_tests.py` matches
   identifiers and `check_cited_docs.py` runs the other way. Now
   `check_cited_files.py`. Measured exposure at the time: 109 distinct paths,
   all resolving, so a guard for next month rather than a fix for today.
2. **A guard whose roster was itself.** `check_toolchain_pins.py` had never
   seen `clients/go/scripts/generate_proto.py`, the one file in the repository
   that installs pinned Go tools, and reported `2 pinned go install target(s)
   in 2 file(s)` about its own docstring and its own test.
3. **A mention counted as a use, twice.** The same guard's `DECIDES` was
   `re.compile(r"GOTOOLCHAIN")`, satisfied by twenty lines of comment
   explaining the setting; and `check_site_css.py` asked whether a class name
   appeared as a *substring* anywhere, so `.bar` was satisfied by "toolbar"
   and `.shell` by the phrase "what a shell produces".
4. **A rule checking half its property.** `check_guard_scope.py` verified that
   every tree a docstring claims to read is one the code reaches, and never the
   reverse — and both guards making a scope claim understated it by one tree.
5. **An exemption marker over-reaching.** No defect in `check_cost_prose.py` at
   all: a `<!-- not a cost-model claim -->` marker covers a paragraph, and one
   paragraph in `docs/performance.md` held a measurement that needs excusing
   beside `POINT_READ_COST = 1.0`, which does not.

Every one was found by breaking the real tree. None was visible from a fixture,
and all twenty-eight guards were green in CI throughout.

`sh scripts/check.sh` reports `85 passed, all of them`.

## What this does not do

**Nothing runs `run_mutations.py` on a schedule.** Unchanged, and now the only
thing standing between this and a check: the suites exist, the roster is
complete, and one command runs them, which is worth exactly as much as the
number of times somebody types it. A `workflow_dispatch` job plus a weekly
`schedule:` is the shape — with the caveat that `schedule:` fires only on the
default branch, so it would not run until this lands on `main`, which is the
"check that never fires" trap `CLAUDE.md` names.

**One mutation per guard is one sample.** `check_handlers` guards fourteen
call sites and this breaks one; `check_site_claims` reads four trees and this
changes one number. A guard can be right about the thing that was mutated and
wrong about its neighbour, which is how `check_cost_prose.py`'s marker hid a
claim the guard could otherwise check.

**The defect rate is five in twenty-eight, not a prediction.** It is this
repository's guards on this day, most of them written in the last three weeks
by sessions working the way this one does. It says nothing about how often the
class recurs, and the obvious next measurement — re-running the survey in a
month and counting how many *new* guards fail — has nobody to take it.

**Four of the five findings are the same failure.** A pattern matching more
than it should: a word in 483 files, a declaration in an unrelated test, a name
in a comment, a substring in a longer word. The fix each time was to narrow the
match and add the case that tells the two apart. Nothing in `scripts/` looks
for the shape, and a lint that did — "a guard whose needle is a bare identifier
with no surrounding syntax" — is the generalisation this entry declines to
build, because four instances is exactly the evidence base that produced the
wrong rule in `ledger/2026-09-29-a-caveat-closed-by-a-line-through-it.md`.
