# Three proptest seeds saved by a mutation run, labelled as such

- **Date:** 2026-09-21
- **Author:** Claude Opus 5
- **Touches:** `crates/slate-server/tests/wire.proptest-regressions`
- **Kind:** docs

## What changed

Five lines of comment above the three seeds the previous commit's mutation runs
left in `wire.proptest-regressions`, saying they came from a deliberate break
and record no bug — matching the note already sitting above the window seed
from an earlier session, for the same reason.

## Why

A `.proptest-regressions` file reads as a list of defects found. Three of its
six lines are not: proptest persists whatever shrank, and `mutate.py` breaking
`contains` on purpose three times produced three shrunk counterexamples. Left
bare they say "the `contains` conversion has three known failures", which is
the opposite of true — the suite is green and these replay as passing cases.

This is a separate commit only because the pre-commit hook compares the staged
diff against `HEAD`, so amending the commit that carries the ledger entry hides
that entry from the check. Rewriting an already-pushed commit to satisfy a hook
is the wrong trade.

## Alternatives rejected

**Delete the three seeds.** One line instead of six, and it throws away the
coverage: each pins a `Contains` under a shape — a double negation, a `CASE`
guard, an `Or(And(...))` — that `any_expr()`'s recursion reaches only by luck.
The window seed was kept for this reason and these are kept for the same one.

**Say nothing and let the shrink text speak.** It does not: `shrinks to expr =
Not(Not(Contains ...))` is identical whether a real bug or a mutation produced
it. That indistinguishability is the whole problem.

**Amend and force-push.** Rejected: the commit is on the remote, and a hook
whose check cannot see across an amend is not a reason to rewrite shared
history.

## Evidence

`cargo test -p slate-server --test wire`: 30 passed, 0 failed, with all six
seeds replayed before any novel case — which is what says these three record no
bug rather than my saying so.

## What this does not do

Nothing stops the next mutation run adding more unlabelled seeds; noticing is
still a person's job, and this is the second time it has fallen to one. A guard
would have to know which seeds a mutation produced, which means `mutate.py`
snapshotting the file around each case — plausible, not attempted here, and
worth doing if this recurs a third time.
