#!/usr/bin/env python3
"""Tests for the parts of `conformance.py` that need no adapters.

The runner itself needs three SDKs and a server, which is why it had no tests:
everything it does is I/O, and the one piece that is not — turning a list of
findings into `N passed, M failed` — was arithmetic nobody could reach without
standing the whole thing up. It was also wrong.

That is the shape `CLAUDE.md` names: a check whose own correctness is taken on
trust because exercising it is expensive. The fix is to make the piece
reachable rather than to test the whole runner, so this file tests `tally` and
the `EXPECTED_REFUSALS`/`MUST_DIFFER` rosters, and leaves the I/O to the run.

Run directly: `python3 examples/explorer/conformance/test_conformance.py`.
"""

from __future__ import annotations

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import conformance

F = conformance.Finding

CASES: list[tuple[str, tuple[int, list], tuple[int, int]]] = [
    (
        "a clean run passes every case and reports no findings",
        (130, []),
        (130, 0),
    ),
    (
        "one broken case costs exactly one pass",
        (130, [F("a", ["FAIL  a"])]),
        (129, 1),
    ),
    (
        "two findings about one case still cost one pass",
        # The counts diverge here and must: two things went wrong, in one case.
        # The old code counted `FAIL` lines and subtracted them from the case
        # total, so this read `128 passed`, which is one case that never
        # existed.
        (130, [F("a", ["FAIL  a"]), F("a", ["FAIL  a, again"])]),
        (129, 2),
    ),
    (
        "a must-differ finding costs no case at all",
        # The defect this file exists for. A `MUST_DIFFER` pair is a
        # relationship *between* two cases: both can agree across the three
        # SDKs — both pass — while the pair fails because they agree with each
        # *other*. Reported as `129 passed, 1 failed` before, where 130 cases
        # did in fact pass.
        (130, [F(None, ["FAIL  the must-differ pair"])]),
        (130, 1),
    ),
    (
        "a must-differ finding beside the case it mentions counts once",
        (130, [F("a", ["FAIL  a"]), F(None, ["FAIL  the pair ('a', 'b')"])]),
        (129, 2),
    ),
    (
        "every case broken passes none",
        (3, [F("a", ["FAIL  a"]), F("b", ["FAIL  b"]), F("c", ["FAIL  c"])]),
        (0, 3),
    ),
]


def rosters() -> list[tuple[str, bool, str]]:
    """The two rosters name cases that exist.

    Both are lists of case *names*, and a renamed case leaves an entry pointing
    at nothing. `MUST_DIFFER` says so at runtime — it reports a pair it cannot
    compare — but only when the run gets that far, which needs three adapters;
    `EXPECTED_REFUSALS` says nothing at all, and a stale entry there is a case
    that may quietly stop being exercised.
    """
    names = {case[0] for case in conformance.CASES}
    checks = []
    for entry in sorted(conformance.EXPECTED_REFUSALS):
        checks.append((
            f"EXPECTED_REFUSALS names a case that exists: {entry!r}",
            entry in names,
            f"{entry!r} is not in CASES",
        ))
    for quiet, loud in conformance.MUST_DIFFER:
        for entry in (quiet, loud):
            checks.append((
                f"MUST_DIFFER names a case that exists: {entry!r}",
                entry in names,
                f"{entry!r} is not in CASES",
            ))
    # The never-fires half: rosters that emptied would make every check above
    # vacuous and this file would print `ok` over nothing.
    checks.append((
        "the rosters are not empty",
        bool(conformance.EXPECTED_REFUSALS) and bool(conformance.MUST_DIFFER),
        "one of EXPECTED_REFUSALS or MUST_DIFFER is empty, so the checks above "
        "looped over nothing",
    ))
    return checks


def pairs() -> list[tuple[str, bool, str]]:
    """`must_differ_findings` blames the pair and never one of its cases.

    The arithmetic in `tally` was tested before this and the code that feeds it
    was not, so a mutation attributing a pair's finding to one of its cases
    survived: `tally` counted exactly what it was handed, correctly, and what
    it was handed was wrong. A test of a pure function is worth what its inputs
    are worth.
    """
    quiet, loud = conformance.MUST_DIFFER[0]
    checks = []

    same = {quiet: '{"rows": []}', loud: '{"rows": []}'}
    found = conformance.must_differ_findings(same)
    checks.append((
        "a pair that agreed with itself is reported",
        any(quiet in line for f in found for line in f.lines),
        f"no finding mentions {quiet!r}: {found}",
    ))
    checks.append((
        "and is blamed on no case, so both of its cases still pass",
        bool(found) and all(f.case is None for f in found),
        f"a finding named a case: {[f.case for f in found]}",
    ))
    checks.append((
        "so the tally leaves the case count alone",
        conformance.tally(10, [f for f in found if quiet in str(f.lines)]) == (10, 1),
        f"got {conformance.tally(10, found)}",
    ))

    differed = {quiet: '{"rows": [1]}', loud: '{"rows": [2]}'}
    checks.append((
        "a pair that differed is not reported",
        not any(quiet in str(f.lines) and loud in str(f.lines)
                for f in conformance.must_differ_findings(differed)),
        "a pair with different answers produced a finding",
    ))

    missing = conformance.must_differ_findings({quiet: "{}"})
    checks.append((
        "a pair with one case missing is reported as not comparable",
        any("not comparable" in line for f in missing for line in f.lines),
        f"expected a not-comparable finding, got {missing}",
    ))
    checks.append((
        "and that one is blamed on no case either",
        bool(missing) and all(f.case is None for f in missing),
        f"a finding named a case: {[f.case for f in missing]}",
    ))
    return checks


def main() -> int:
    failed = 0
    for name, (cases, findings), expected in CASES:
        got = conformance.tally(cases, findings)
        ok = got == expected
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {expected}, got {got}")

    for name, ok, why in rosters() + pairs():
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        {why}")

    total = len(CASES) + len(rosters()) + len(pairs())
    print()
    print(f"{total - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
