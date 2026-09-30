# The one roster in `scripts/` that was checked in neither direction

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `scripts/check_toolchain_pins.py`,
  `scripts/test_check_toolchain_pins.py`, `docs/caveat-status.json`,
  `scripts/check_closed_caveats.py`
- **Kind:** fix

## What changed

`stale_skips()` in `check_toolchain_pins.py` holds `NOT_AN_INSTALLER` to the
tree: every name in it must be a readable file that still carries a `go install`
example. A stale entry is reported and stops the rest of the guard, because the
installer set was computed with the wrong exclusions.

Two cases in the guard's own suite, and `run()` now seeds the two skipped files
into every synthetic tree so the other twelve stay about what they were about.
Sixteen cases, up from fourteen.

## Why

`ledger/2026-09-29-a-guard-whose-roster-was-itself.md` named it and left it
`open`:

> **The self-skip is by filename.** `NOT_AN_INSTALLER` names two paths. A guard
> renamed or a second example file added gets no warning — the skip does not
> check that the file it names still carries an example, which is the same
> roster-rot this repository checks for in `EXTERNAL`, `WITNESS` and `PROVES`
> and does not check here.

That is a list of three rosters that already do this and one that did not, which
is as close as this repository gets to a defect being pre-diagnosed. **A skip
that excuses nothing is worse than no skip**: a reader takes the name for a live
exemption, and if the file later grows a *real* installer the skip hides it —
the guard would then be silent about exactly the file it was written to watch.

The caveat's other half — "a second example file added gets no warning" — needs
no rule and gets none. A new file carrying an example and no entry here is
already reported as an installer, which is the guard asking to be told. That is
what happened to this file and its test when the rule was first written, and it
is the behaviour that gave the never-fires halves something to bite on.

## Alternatives rejected

**Check that the skipped file still parses as a guard**, rather than that it
carries an example. Narrower and wrong: the exemption is not "this is a guard",
it is "the `go install` in here is documentation". A guard that stopped
documenting the pattern should lose the skip even if it is still a guard.

**Report a stale skip and carry on to the installer loop.** Cheaper and it
misreports: `installers()` applies the skip list before matching, so a stale
entry means the set the loop iterates was built from the wrong exclusions.
Reporting installers against a tree the guard has misread is how a real finding
and an artefact arrive in the same list. The never-fires halves already return
early for the same reason.

**Give each case in the test its own copy of the two skipped files.** Explicit,
and it would have put four lines of fixture into twelve cases that are not about
the skip list. Seeding in `run()` with a per-case override (`None` means "and
not this one") keeps each case's dict to the thing it is testing — the shape the
two new cases then use to make the rule fire.

**Drop `NOT_AN_INSTALLER` and exclude by content instead**, e.g. skipping a file
whose `go install` is inside a string constant. That is a parser, and the
never-fires note in this guard already argues the general case: a narrow rule
with a loud failing half beats a parser nobody can debug. It would also be
wrong here — the examples in these two files are in ordinary code position.

## Evidence

`python3 scripts/test_check_toolchain_pins.py`: **16 passed, 0 failed**, up from
14. `python3 scripts/check_toolchain_pins.py` against the real tree: ok, 5
pinned `go-version` across 1 workflow, 2 pinned `go install` targets in 1 file.

Three mutations, record
[`ledger/mutations/20260929T205854-scripts-check-toolchain-pins-py.json`](mutations/20260929T205854-scripts-check-toolchain-pins-py.json):

| mutation | outcome |
| --- | --- |
| the rot rule stops noticing a skipped file that is gone | caught, `a skipped file that is gone is reported` |
| the rot rule stops noticing a skipped file with no example left | caught, `a skipped file that no longer carries an example is reported` |
| a stale skip no longer stops the rest of the guard running | caught, both |

The third is the one worth having. It leaves `stale_skips()` intact and called,
and only removes the early return — which is the change somebody would make
while "tidying up", and which turns a hard stop into a computation whose result
is thrown away. Both new cases catch it, because with the early return gone the
report is the installer loop's output and not the rot message.

Each mutation is a change to behaviour rather than to spelling: the first
inserts a `continue` before the append, the second makes the condition
constantly false, the third deletes a control-flow edge. None is the
`&x.clone()` shape.

## What this does not do

**It does not check the skip list against what the file's example is *for*.**
`GO_INSTALL.search` says an example is present, not that it is the pattern this
guard matches for the reason the comment gives. A file that grew an unrelated
`go install` in a docstring would keep its skip alive on the wrong evidence.

**`PINNED` still matches any `@v<digit>` token**, which is the sibling caveat in
the same entry and is untouched here. A file containing a `go install` and an
unrelated version-pinned string reads as a pinned Go installer; nothing in the
tree does today.

**Two files is a roster small enough that rot was unlikely**, and that is the
honest size of this finding: the rule is right, the class is real and has bitten
three other rosters, and the specific list it now guards has two entries that
have never moved. It is cheap insurance, not a caught bug.

**Nothing checks that the never-fires halves still fire.** They are tested
against synthetic trees here, which is the same thing the seeded fixture now
guarantees cannot happen by accident — and a seeded fixture is a precondition
somebody could satisfy wrongly.
