# The two never-fires halves of the GOTOOLCHAIN guard had no case holding them to firing

- **Date:** 2026-09-30
- **Author:** Claude Opus 5
- **Touches:** `scripts/test_check_toolchain_pins.py`
- **Kind:** process

## What changed

`check_toolchain_pins.py` opens with two never-fires halves: if no workflow
pins `go-version:`, or if nothing in the tree runs a pinned `go install`, the
forward rule below would be checking an empty set, so each reports instead of
passing. Two synthetic cases covered them and nothing held those cases to
existing. The suite now derives the roster of halves by running the guard over
an **empty** tree — a tree with nothing in it violates every such precondition
by construction — and, for each report that comes back, requires some case
above to be *about* it: an expectation drawn from that report's text, which
that case actually observed. A third check asserts the derived roster is not
itself empty.

## Why

The caveat is
`ledger/2026-09-29-the-skip-list-that-excused-nothing.md`: the seeded fixture
that gives every synthetic tree the two skip-listed files is a precondition
each case inherits, so a case could satisfy it wrongly and a half would stop
firing with the suite still green. That is the repository's oldest recurring
shape — a check that never fires is a check nobody has debugged — applied to
the very code written to catch it.

## Alternatives rejected

**A roster of case names**, the idiom in
`ledger/2026-09-29-a-roster-of-test-names-is-weaker-and-worth-having.md`. It
holds names to existing, not branches to firing: a case renamed, or edited
until its tree no longer reaches the branch, still passes.

**Naming the two reports as constants in the guard** and iterating those. It
was written and reverted. The tuple is hand-maintained, so a third half added
later is absent from it and untested — the same rot, one level along, and
nothing would say so. Deriving from the empty tree costs a fifteenth guard
run, which is milliseconds, and grows on its own.

**Requiring each half to be produced anywhere in the run** was the first
version. It is satisfied by the empty-tree case alone, so it checks nothing:
the claim worth making is that some case *isolates* each half.

## Evidence

`python3 scripts/test_check_toolchain_pins.py`: 19 passed, 0 failed (was 16).
Three mutations, via `scripts/mutate.py`, across two runs:
`ledger/mutations/20260930T010415-scripts-test-check-toolchain-pins-py.json`
(the surviving one, and the roster) and
`ledger/mutations/20260930T010434-scripts-test-check-toolchain-pins-py.json`
(the deletion, re-run as a real deletion, and the retargeting):

| mutation | outcome |
| --- | --- |
| delete the whole case targeting the `go install` half | caught, by the new check naming that half |
| that case's expectation edited to `"is reported"`, which is in neither report | caught, twice: the case itself, and the new check |
| `never_fires()` reads a tree where both preconditions hold | caught, by "an empty tree names at least one half" |

A fourth was run first and **survived**: broadening that case's expectation
from ``"nothing in this repository runs `go install"`` to ``"go install"``.
That is not a change to the property — a case asking for ``"go install"`` is
still about that half and still observes it — but it does weaken the case's own
assertion, and nothing catches that. See below.

## What this does not do

Substring matching is the whole harness's idiom: a case's expectation is
`in`-tested against the report, so an expectation broadened to a common
fragment still passes and still counts as targeting. The surviving mutation
above is that, and it predates this change.

`NO_PINNED_WORKFLOW` is targeted by two cases, so deleting one leaves it
covered. The claim made here is "at least one case is about each half", not
"exactly one", and that is what the check tests.

Nothing holds the guard's *forward* rule — the per-installer report — to being
exercised; only the two halves are covered this way. Ten cases exercise it
today, which is why it was left.
