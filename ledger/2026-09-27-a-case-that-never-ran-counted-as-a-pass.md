# The conformance runner's passed count went *up* when a case was skipped

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-21-the-conformance-runners-own-arithmetic.md`
- **Touches:** `examples/explorer/conformance/conformance.py`,
  `examples/explorer/conformance/test_conformance.py`
- **Kind:** fix

## What changed

`silent_cases(names, findings, agreed_by_name)` returns a `Finding` for every
case in `CASES` that produced neither an agreement nor a finding, and `main`
extends the findings with it before the `MUST_DIFFER` pass. Five checks in
`test_conformance.py`, taking it from 52 to 57.

## Why

`tally` defines a pass as "not named by a finding". That is right for the
defect it was written to fix — a case with two findings is one failure — and it
has a second consequence the entry recorded and did not close: a case that the
loop *skipped* produces no finding, so it is counted as passing.

Not merely uncaught. It is uncaught in the direction that makes the report more
confident: drop a case from the comparison and `130 cases: the three SDKs agree
on all of them` still prints, with one of the hundred and thirty never sent to
an adapter. Every other failure mode here makes the number worse.

The fix is positive rather than a subtraction inside `tally`: a silent case
becomes a finding, so it is *named* in the output the way every other failure
is. Making the arithmetic right while leaving the report silent would be the
same class of quiet as the bug.

## Alternatives rejected

**Subtract the silent cases inside `tally`.** Two lines instead of twenty, and
the printed count would be correct. Rejected because the count is not what a
person reads when something is wrong — the `FAIL` lines are — and a run
reporting `128 passed, 2 failed` with two failures printed and a third
unexplained is worse than the bug, which at least did not lie about how many
lines to expect.

**Assert `len(agreed_by_name) + len(broken) == len(CASES)` and raise.** A
tighter statement of the same thing and it fails as an exception rather than as
a finding, so the run stops at the first skipped case instead of reporting
every problem it found. The runner's whole shape is collect-then-report.

**Mark each case as attempted at the top of the loop.** The obvious
instrumentation, and it is the same bug one line up: a `continue` placed before
the mark skips the mark too, and a `continue` placed after it marks a case that
never ran. Defining "reached a verdict" as "produced an agreement or a finding"
cannot be fooled that way, because both are things the loop has to actually
construct.

## Evidence

- `python3 examples/explorer/conformance/test_conformance.py` — 57 passed, 0
  failed (52 before).
- Four mutations via `scripts/mutate.py`, recorded as
  `ledger/mutations/20260927T024745-examples-explorer-conformance-conformance-py.json`:
  - treating every name as having a verdict → caught by four checks;
  - blaming the silent finding on no case → caught by three, including *so the
    tally stops counting it as a pass*, which is the point;
  - **two recorded as expected survivors, with reasons.** Dropping the
    `is not None` filter from the verdict set is *equivalent*, not uncaught:
    the set is only ever asked `name not in verdict` for a name out of `CASES`,
    and no case is called `None`. Discarding `silent_cases`' return value in
    `main` survives because the unit suite calls `silent_cases` directly and
    nothing that fits in `check.sh` runs `main` — that needs three adapters and
    a server.

The first of those two is the class `CLAUDE.md` warns about: *first check the
mutation was a change*. It looks exactly like a missing test and writing one
for it would have been writing a test that cannot fail.

## What this does not do

**`main` is still untested, and that is where the call lives.** The check is
exercised through `silent_cases` and never through the loop that feeds it,
which is the same shape `must_differ_findings` was in before its own test — and
that gap let a mutation through last time. Closing it needs three SDK adapters
and a head node, which is `run.sh --conformance` and not anything `check.sh`
can reach; the expected-survivor entry says so rather than leaving it implied.

**A case can still be wrong without being silent.** Nothing here checks that a
case's `path` and `body` actually exercise what its name says — the runner
compares three adapters against each other, so three adapters agreeing on the
answer to a question nobody meant to ask is a pass, and always was.

**The `is not None` filter is now redundant in one of the two places it
appears.** It stays for symmetry with `tally`, where it is load-bearing. That
is a judgement about which way a future edit is more likely to go wrong, and it
is recorded rather than argued: two adjacent lines reading differently is an
invitation to make one match the other, and the wrong direction there is a real
defect.
