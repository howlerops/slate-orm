#!/usr/bin/env python3
"""Tests for the parts of `conformance.py` that need no adapters.

The runner itself needs three SDKs and a server, which is why it had no tests:
everything it does is I/O, and the one piece that is not — turning a list of
findings into `N passed, M failed` — was arithmetic nobody could reach without
standing the whole thing up. It was also wrong.

That is the shape `CLAUDE.md` names: a check whose own correctness is taken on
trust because exercising it is expensive. The fix is to make the piece
reachable rather than to test the whole runner, so this file tests `tally` and
the `EXPECTED_REFUSALS`/`MUST_DIFFER`/`EXPECTED_ACCESS` rosters, and leaves the
I/O to the run.

Run directly: `python3 examples/explorer/conformance/test_conformance.py`.
"""

from __future__ import annotations

import pathlib
import sys
from collections.abc import Callable

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
    for quiet, loud, _field in conformance.MUST_DIFFER:
        for entry in (quiet, loud):
            checks.append((
                f"MUST_DIFFER names a case that exists: {entry!r}",
                entry in names,
                f"{entry!r} is not in CASES",
            ))
    for entry in sorted(conformance.EXPECTED_ACCESS, key=str):
        checks.append((
            f"EXPECTED_ACCESS names a case that exists: {entry!r}",
            entry in names,
            f"{entry!r} is not in CASES",
        ))
    # The never-fires half: rosters that emptied would make every check above
    # vacuous and this file would print `ok` over nothing.
    checks.append((
        "the rosters are not empty",
        bool(conformance.EXPECTED_REFUSALS)
        and bool(conformance.MUST_DIFFER)
        and bool(conformance.EXPECTED_ACCESS),
        "one of EXPECTED_REFUSALS, MUST_DIFFER or EXPECTED_ACCESS is empty, so "
        "the checks above looped over nothing",
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
    quiet, loud, field = conformance.MUST_DIFFER[0]
    checks = []

    same = {quiet: {field: []}, loud: {field: []}}
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

    differed = {quiet: {field: [1]}, loud: {field: [2]}}
    checks.append((
        "a pair that differed is not reported",
        not any(quiet in str(f.lines) and loud in str(f.lines)
                for f in conformance.must_differ_findings(differed)),
        "a pair with different answers produced a finding",
    ))

    # Differing everywhere *except* the field the pair is about, which is the
    # hole naming the field closes: this passed before, because the two
    # answers were not byte-identical.
    elsewhere = {
        quiet: {field: [1], "servedBy": "writer"},
        loud: {field: [1], "servedBy": "replica"},
    }
    checks.append((
        "a pair that differed somewhere else is still reported",
        any(quiet in str(f.lines) and loud in str(f.lines)
            for f in conformance.must_differ_findings(elsewhere)),
        f"two answers agreeing on {field!r} and differing elsewhere passed",
    ))

    # And the field going missing is its own finding, not a silent pass: two
    # answers with no `field` at all would compare `None` to `None`.
    gone = {quiet: {"servedBy": "writer"}, loud: {"servedBy": "replica"}}
    checks.append((
        "a pair whose field is absent from both answers is reported",
        any("is about" in line
            for f in conformance.must_differ_findings(gone) for line in f.lines),
        f"neither answer carried {field!r} and no finding said so",
    ))

    missing = conformance.must_differ_findings({quiet: {}})
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


def silent() -> list[tuple[str, bool, str]]:
    """A case that produced neither an agreement nor a finding is a finding.

    The gap this closes: `tally` defines passing as "not named by a finding",
    so a case the loop skipped counted as a pass and *raised* the reported
    number. Four checks, because the wrong fix has three tempting shapes — a
    silent case that is not reported, one reported and still counted as
    passing, and a real pass reported as silent.
    """
    names = ["alpha", "beta", "gamma"]
    checks = []

    skipped = conformance.silent_cases(
        names, [conformance.Finding("alpha", ["FAIL  alpha: nope"])], {"beta": "{}"}
    )
    checks.append((
        "a case with neither an agreement nor a finding is reported",
        [f.case for f in skipped] == ["gamma"],
        f"expected gamma alone, got {[f.case for f in skipped]}",
    ))
    checks.append((
        "and it says it was skipped rather than that it failed",
        bool(skipped) and any("skipped, not passed" in line for line in skipped[0].lines),
        f"the message does not say so: {skipped and skipped[0].lines}",
    ))
    checks.append((
        "so the tally stops counting it as a pass",
        # One case failed, one agreed, one was silent: two failures and one
        # pass, where before this the silent one made it two passes.
        conformance.tally(
            len(names),
            [conformance.Finding("alpha", ["FAIL  alpha: nope"]), *skipped],
        ) == (1, 2),
        f"got {conformance.tally(len(names), [conformance.Finding('alpha', ['x']), *skipped])}",
    ))
    checks.append((
        "and a run where every case reached a verdict reports nothing",
        conformance.silent_cases(
            names,
            [conformance.Finding("alpha", ["FAIL  alpha: nope"])],
            {"beta": "{}", "gamma": "{}"},
        ) == [],
        "a case that agreed was reported as silent",
    ))
    checks.append((
        "a pair finding, which is about no case, does not fill a case's slot",
        [f.case for f in conformance.silent_cases(
            names, [conformance.Finding(None, ["FAIL  a pair"])], {}
        )] == names,
        "a finding about no case was read as a verdict for one",
    ))
    return checks


def access() -> list[tuple[str, bool, str]]:
    """`access_findings` reads the roster in both directions.

    The roster exists because `MUST_DIFFER` compares two plans to each other
    and so cannot see the server renaming both. These are the four things it
    has to do, and none of them is reachable from a run without standing the
    stack up and editing a plan's text — so they are tested here, where an
    answer is a dict.
    """
    name = "a search for 'the' by index"
    checks = []

    def answers(**edits: object) -> dict[str, object]:
        """Every rostered case answering what the roster says, plus `edits`.

        All of them, because a roster line whose case is absent is itself a
        finding — correctly, in a real run — so a one-entry input would report
        the other eleven as stale and bury what is being tested.
        """
        every: dict[str, object] = {
            case: {"access": access, "rows": []}
            for case, access in conformance.EXPECTED_ACCESS.items()
        }
        every.update(edits)
        return every

    checks.append((
        "a run where every plan matches the roster is not reported",
        conformance.access_findings(answers()) == [],
        f"a matching run produced findings: {conformance.access_findings(answers())}",
    ))

    renamed = conformance.access_findings(answers(**{name: {"access": "Seq Scan"}}))
    checks.append((
        "a renamed plan is reported",
        any("access path changed" in line for f in renamed for line in f.lines),
        f"a plan text the roster does not hold passed: {renamed}",
    ))
    checks.append((
        "and is blamed on the case, which did not pass",
        bool(renamed) and all(f.case == name for f in renamed),
        f"expected every finding blamed on {name!r}: {[f.case for f in renamed]}",
    ))

    # An answer carrying a plan that nobody rostered. The `None` for this one
    # is deliberate: it is what a `reader`'s search returns, so a case going
    # from no plan to a plan is exactly the change this arm catches.
    unrostered = conformance.access_findings(
        answers(**{"a case nobody rostered": {"access": None}})
    )
    checks.append((
        "a case reporting an access path that is not in the roster is reported",
        any("nothing has looked at it" in line
            for f in unrostered for line in f.lines),
        f"an unrostered plan passed: {unrostered}",
    ))

    # The other direction: the roster is not empty and the run produced no
    # answer carrying a plan, so every line is stale.
    stale = conformance.access_findings(answers(**{name: {"rows": []}}))
    checks.append((
        "a roster line for a case that reported no plan is reported",
        any("the roster is stale" in line for f in stale for line in f.lines),
        f"a stale roster line passed: {stale}",
    ))
    checks.append((
        "and is blamed on no case, since the case itself may have passed",
        bool(stale) and all(f.case is None for f in stale if "stale" in str(f.lines)),
        f"a stale-roster finding named a case: {[f.case for f in stale]}",
    ))
    return checks


def harness() -> list[tuple[str, bool, str]]:
    """`run` turns a group that raises into a failing check, not into silence.

    Written because a mutation emptying that branch SURVIVED: nothing on a
    clean tree raises, so the arm exists only for a tree somebody has broken —
    which is precisely the tree `scripts/mutate.py` builds, and precisely when
    the difference between "no test failed" and "the harness died" decides
    whether a mutation scores.
    """
    def boom() -> list[tuple[str, bool, str]]:
        raise KeyError("a case the roster does not hold")

    found = run("boom", boom)
    return [
        (
            "a check group that raises becomes one failing check",
            len(found) == 1 and found[0][1] is False,
            f"expected one failing check, got {found}",
        ),
        (
            "and the check says which group and what it raised",
            bool(found)
            and "boom" in found[0][0]
            and "KeyError" in found[0][2],
            f"the failure does not name the group and the error: {found}",
        ),
        (
            "a group that does not raise is passed through untouched",
            run("silent", silent) == silent(),
            "a clean group's checks were changed on the way through",
        ),
    ]


#: Every check group, paired with its name.
#:
#: The name is written out rather than read off `__name__`: a
#: `Callable[[], ...]` annotation is not a function to a type checker, which
#: is right — the annotation admits any callable, and only functions carry
#: `__name__`. CI's `ty` says so and this container's does too.
GROUPS: list[tuple[str, Callable[[], list[tuple[str, bool, str]]]]] = [
    ("rosters", rosters),
    ("pairs", pairs),
    ("silent", silent),
    ("access", access),
    ("harness", harness),
]


def run(
    name: str, group: Callable[[], list[tuple[str, bool, str]]]
) -> list[tuple[str, bool, str]]:
    """A group's checks, or one failing check saying it raised.

    A group builds its checks by *calling* the function under test, so a
    defect that makes it raise takes the whole file down with a traceback and
    no summary line — and `scripts/mutate.py` reads the summary line. "No test
    failed" and "the harness died" then look identical, which is the second of
    the six ways a mutation run lies, and two mutations against
    `access_findings` and `must_differ_findings` hit it: deleting either
    guard turns the next line into a `KeyError` rather than a wrong answer.

    Caught here rather than defended against in the runner, because a guard
    whose removal crashes the test harness is a guard the harness cannot
    score, and adding a redundant `.get` to the runner to make it scoreable
    would be dead code standing in for a test.
    """
    try:
        return group()
    except Exception as error:  # noqa: BLE001 - the group is the thing under test
        return [(
            f"the {name} checks build without raising",
            False,
            f"{type(error).__name__}: {error}",
        )]


def main() -> int:
    failed = 0
    for name, (cases, findings), expected in CASES:
        got = conformance.tally(cases, findings)
        ok = got == expected
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {expected}, got {got}")

    checks = [check for name, group in GROUPS for check in run(name, group)]
    for name, ok, why in checks:
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        {why}")

    total = len(CASES) + len(checks)
    print()
    print(f"{total - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
