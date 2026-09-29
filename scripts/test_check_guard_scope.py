#!/usr/bin/env python3
"""Tests for `check_guard_scope.py`, over guards this file writes.

Run against the real tree alone, every rule here would pass for as long as the
tree stayed correct — which is the failure the guard exists to prevent, one
level up. So each case writes a `scripts/` directory holding one small guard
with a docstring and a path or two, and the real tree is checked last as the
case that says "and this repository agrees".

Run directly: `python3 scripts/test_check_guard_scope.py`.
"""

from __future__ import annotations

import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_guard_scope as guard

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: A guard that says nothing about scope and reads one tree. Present in every
#: case that is not about an empty tree, so the three never-fires halves do not
#: fire on a fixture that is about something else — and, for the two of them
#: that count claims and paths, so that a case about a *contradiction* is not
#: also a case about an empty set.
SPARE = '"""It reads `docs/`."""\nROOT = 1\nDOCS = ROOT / "docs"\n'


def run(guards: dict[str, str], spare: bool = True) -> list[str]:
    """The real guard over a `scripts/` this writes."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        where = root / "scripts"
        where.mkdir()
        if spare:
            guards = {**guards, "check_spare.py": SPARE}
        for name, body in guards.items():
            (where / name).write_text(body)
        return guard.problems(root)


#: name, the guards to write, the text the report must carry, and whether to
#: include the spare. An empty expectation means nothing is reported.
CASES: list[tuple[str, dict[str, str], str, bool]] = [
    (
        # The other direction, added when a real-tree mutation found it
        # missing: both guards making a scope claim understated it by one
        # tree, and only claimed-but-not-read was checked. A claim that
        # understates reads as exhaustive.
        "a guard that reads a tree its scope claim omits is reported",
        {
            "check_a.py": '"""It reads `docs/`."""\n'
            'ROOT = 1\nDOCS = ROOT / "docs"\nSITE = ROOT / "site"\n'
        },
        "`site/` is not among them",
        True,
    ),
    (
        # And the exemption that keeps the rule from becoming a different one:
        # twenty-five of the twenty-eight guards say nothing about scope, and
        # saying nothing is not saying the wrong thing.
        "a guard with no scope claim at all reads what it likes",
        {"check_a.py": 'ROOT = 1\nDOCS = ROOT / "docs"\nSITE = ROOT / "site"\n'},
        "",
        True,
    ),
    (
        "a guard that says a tree is out of scope and does not read it is clean",
        {
            "check_a.py": '"""`site/` is out of scope.\n\nIt reads `docs/`.\n"""\n'
            'ROOT = 1\nDOCS = ROOT / "docs"\n'
        },
        "",
        True,
    ),
    (
        # The defect, exactly: #283 widened the tree, corrected the paragraph
        # that introduces the scope, and left the one that restates it.
        "a guard that says a tree is out of scope and reads it is reported",
        {
            # `site/`, not `docs/`. The real defect's sentence was about
            # `docs/`, and `scripts/check_retired_claims.py` refuses that exact
            # phrase anywhere in the tree - correctly: a reader grepping for a
            # withdrawn claim should not find it alive in a fixture. The shape
            # is what this case is about, and the shape is the same.
            "check_a.py": '"""`site/` is still out of scope, and deliberately."""\n'
            'ROOT = 1\nSITE = ROOT / "site"\n'
        },
        "check_a.py's docstring says `site/` is out of scope",
        True,
    ),
    (
        "the other phrasings are read the same way",
        {"check_a.py": '"""`docs/` is not read by this."""\nROOT = 1\nDOCS = ROOT / "docs"\n'},
        "says `docs/` is out of scope",
        True,
    ),
    (
        # The `[^.\n]` in the pattern. Two sentences are not one claim.
        "a full stop between the tree and the phrase is not a claim",
        {
            "check_a.py": '"""It reads `docs/`. Nothing here is out of scope by accident."""\n'
            'ROOT = 1\nDOCS = ROOT / "docs"\n'
        },
        "",
        True,
    ),
    (
        # `check_cited_tests.py` says "the ledger is out of scope by principle"
        # and reads `ledger/` constantly: that sentence is about what it
        # verifies, not about a directory walk.
        "a tree named without a slash is prose, not a scope claim",
        {
            "check_a.py": '"""The ledger is out of scope by principle."""\n'
            'ROOT = 1\nLEDGER = ROOT / "ledger"\n'
        },
        "",
        True,
    ),
    (
        "a guard claiming to read a tree it has no path into is reported",
        {"check_a.py": '"""It reads `site/`."""\nROOT = 1\nDOCS = ROOT / "docs"\n'},
        "says it reads `site/`, and no module-level path points there",
        True,
    ),
    (
        # The false positive that shaped the pattern: a sentence about what
        # some *other* guard reads is not a claim about this one.
        "a mid-sentence mention of what another guard reads is not a claim",
        {
            "check_a.py": '"""It reads `docs/`.\n\nUnlike check_b, which it '
            'reads `site/` for.\n"""\nROOT = 1\nDOCS = ROOT / "docs"\n'
        },
        "",
        True,
    ),
    (
        # The false positives that shaped `ROOTISH`: three of this
        # repository's guards call the root `REPO` or take it as `root`.
        "a root called REPO is followed like ROOT",
        {"check_a.py": '"""It reads `site/`."""\nREPO = 1\nP = REPO / "site"\n'},
        "",
        True,
    ),
    (
        "and a root taken as a parameter, which is where most of them are",
        {
            "check_a.py": '"""It reads `site/`."""\n'
            "def pages(root):\n    return (root / \"site\").glob('*')\n"
        },
        "",
        True,
    ),
    (
        "no guards at all is reported, not passed over",
        {},
        "no `scripts/check_*.py` at all",
        False,
    ),
    (
        "no scope claim anywhere is reported, not passed over",
        {
            "check_a.py": '"""A guard that says nothing about scope."""\n'
            'ROOT = 1\nDOCS = ROOT / "docs"\n'
        },
        "no guard's docstring makes a scope claim",
        False,
    ),
    (
        "no root path anywhere is reported, not passed over",
        {"check_a.py": '"""`site/` is out of scope."""\n'},
        'no guard builds a `<root> / "…"` path at all',
        False,
    ),
]


def main() -> int:
    failed = 0
    for name, guards, wanted, spare in CASES:
        try:
            found = run(guards, spare=spare)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    found = guard.problems()
    ok = not found
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  every guard here describes its own scope correctly")
    if not ok:
        for one in found:
            print(f"      {one}")

    print()
    print(f"{len(CASES) + 1 - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
