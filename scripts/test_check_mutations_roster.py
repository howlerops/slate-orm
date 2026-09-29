#!/usr/bin/env python3
"""Tests for `check_mutations_roster.py`, over a tree this one builds.

Run against the real roster, every case would be "it passed" — and a rule that
had stopped checking would look identical, which is the offence one level up
that this whole roster exists to catch.

Run directly: `python3 scripts/test_check_mutations_roster.py`.
"""

from __future__ import annotations

import contextlib
import io
import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_mutations_roster as guard

GUARD = '"""A guard."""\n'
WATCHED = "the line a mutation anchors on\nand a second line, for the twice case\n"


def tree(root: pathlib.Path, roster: dict, guards: tuple[str, ...] = ("check_a",)) -> None:
    (root / "scripts").mkdir(parents=True)
    for name in guards:
        (root / "scripts" / f"{name}.py").write_text(GUARD)
    (root / "watched.txt").write_text(WATCHED)
    (root / "scripts" / "mutations.json").write_text(json.dumps(roster))


def suite(guard_name: str = "check_a", **over) -> dict:
    return {
        "guard": guard_name,
        "file": "watched.txt",
        "why": "because the guard reads it",
        "cases": [
            {"name": "the line changes", "old": "the line a mutation", "new": "another line"}
        ],
        **over,
    }


def run(root: pathlib.Path) -> tuple[int, str]:
    guard.ROOT = root
    guard.ROSTER = root / "scripts" / "mutations.json"
    out, err = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = guard.main()
    except Exception as raised:  # noqa: BLE001 - any raise is a failing case
        return 70, f"{out.getvalue()}{err.getvalue()}the guard raised {raised!r}"
    return code, out.getvalue() + err.getvalue()


CASES = [
    (
        "a guard with a suite passes",
        lambda root: tree(root, {"suites": [suite()], "unmutated": {}}),
        0,
        "1 guards, 1 with a real-tree mutation",
    ),
    (
        "a guard with a written reason passes",
        lambda root: tree(
            root, {"suites": [], "unmutated": {"check_a": "not yet, and here is why"}}
        ),
        0,
        "1 with a written reason",
    ),
    (
        "a guard with neither fails — the headline case",
        lambda root: tree(root, {"suites": [], "unmutated": {}}),
        1,
        "scripts/check_a.py has no real-tree mutation and no reason",
    ),
    (
        "a guard with both fails",
        lambda root: tree(root, {"suites": [suite()], "unmutated": {"check_a": "why"}}),
        1,
        "has a suite and an `unmutated` reason",
    ),
    (
        "a roster naming a guard that is gone fails",
        lambda root: tree(root, {"suites": [suite("check_b")], "unmutated": {"check_a": "why"}}),
        1,
        "which is not a guard under scripts/ any more",
    ),
    (
        "two suites for one guard fails",
        lambda root: tree(root, {"suites": [suite(), suite()], "unmutated": {}}),
        1,
        "two suites name the same guard",
    ),
    (
        # The anchor rot `mutate.py`'s docstring calls the first of six lies:
        # the patch silently matches nothing and the suite runs against
        # unmutated code.
        "an anchor that no longer occurs fails",
        lambda root: tree(
            root,
            {
                "suites": [
                    suite(cases=[{"name": "n", "old": "moved under a reformat", "new": "x"}])
                ],
                "unmutated": {},
            },
        ),
        1,
        "anchors on text occurring 0 times",
    ),
    (
        "an anchor occurring twice fails",
        lambda root: tree(
            root,
            {"suites": [suite(cases=[{"name": "n", "old": "line", "new": "x"}])], "unmutated": {}},
        ),
        1,
        "anchors on text occurring 2 times",
    ),
    (
        "a suite whose target file is gone fails",
        lambda root: tree(root, {"suites": [suite(file="nowhere.txt")], "unmutated": {}}),
        1,
        "which is not a file",
    ),
    (
        "a suite with no `why` fails",
        lambda root: tree(root, {"suites": [suite(why="   ")], "unmutated": {}}),
        1,
        "has no `why`",
    ),
    (
        "an `unmutated` entry with no reason fails",
        lambda root: tree(root, {"suites": [], "unmutated": {"check_a": "  "}}),
        1,
        "with no reason",
    ),
    (
        "no guards at all fails",
        lambda root: tree(root, {"suites": [], "unmutated": {}}, guards=()),
        1,
        "so this compared nothing",
    ),
]


def main() -> int:
    was = (guard.ROOT, guard.ROSTER)
    failures = []
    try:
        for name, build, wanted, needle in CASES:
            guard.ROOT, guard.ROSTER = was
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                try:
                    build(root)
                except Exception as raised:  # noqa: BLE001 - so is a bad fixture
                    code, output = 70, f"the fixture raised {raised!r}"
                else:
                    code, output = run(root)
            if code != wanted:
                failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
            elif needle not in output:
                failures.append(f"FAIL  {name}: no {needle!r} in\n{output}")
            else:
                print(f"ok    {name}")
    finally:
        guard.ROOT, guard.ROSTER = was

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
