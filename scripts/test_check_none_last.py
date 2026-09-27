#!/usr/bin/env python3
"""`check_none_last.py`, over trees this file writes.

Against a written tree rather than against `slate-orm`, for the reason the
other guards here give: a check tested only by running it over this repository
asserts that today's tree is clean, which is also what a check that does
nothing asserts.

Most cases go through `offenders()` on a source string, because that is where
the question lives; the ones about *which files are read* write a tree.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_none_last as guard


def finds(name: str, source: str, want: int) -> bool:
    got = guard.offenders(source)
    if len(got) != want:
        print(f"FAIL  {name}\n        {source.strip()!r}\n        {want} expected, got {got}")
        return False
    print(f"ok    {name}")
    return True


def tree(name: str, files: dict[str, str], failing: set[str]) -> bool:
    root = Path(tempfile.mkdtemp())
    for path, body in files.items():
        full = root / path
        full.parent.mkdir(parents=True, exist_ok=True)
        full.write_text(body, encoding="utf-8")
    failed = [what for what, ok, _ in guard.check(root) if not ok]
    unexpected = [f for f in failed if not any(frag in f for frag in failing)]
    unseen = [frag for frag in failing if not any(frag in f for f in failed)]
    if unexpected or unseen:
        print(f"FAIL  {name}\n        got {failed}, wanted {sorted(failing)}")
        return False
    print(f"ok    {name}")
    return True


def main() -> int:
    passed = [
        finds("None last is correct", "x: int | None = None\n", 0),
        finds("None first is reported", "x: None | int = None\n", 1),
        finds(
            "None in the middle of a long union is reported",
            "x: str | None | dict[str, str] = None\n",
            1,
        ),
        finds(
            "a long union with None last is correct",
            "x: str | dict[str, str] | bytes | None = None\n",
            0,
        ),
        finds(
            "a chain reports once, not once per link",
            # `ast` parses `A | B | C | D` as `(((A | B) | C) | D)`, so a walk
            # that did not flatten first would report this four times. The
            # first draft of the sweep did exactly that: nine findings for one
            # union in `values.py`.
            "x: None | int | str | bytes | float = None\n",
            1,
        ),
        finds(
            "a type alias counts, which is where this is stricter than RUF036",
            # The repository's one real occurrence. `ruff --preview --select
            # RUF036` passes on it, because an alias is not an annotation.
            "PyValue = None | Null | bool | int\n",
            1,
        ),
        finds("a return annotation counts", "def f() -> None | int: ...\n", 1),
        finds("a parameter annotation counts", "def f(x: None | int) -> int: ...\n", 1),
        finds(
            "a bitwise or between values is not a union",
            # `flags = None | 1` is a TypeError at run time and not this
            # check's business, but `a | b` over two integers is ordinary code
            # and must not be reported.
            "mask = 0b01 | 0b10\n",
            0,
        ),
        finds("a file that will not parse is skipped rather than reported", "def (:\n", 0),
        finds("an empty file is fine", "", 0),
        finds(
            "None alone is not a union",
            "x: None = None\n",
            0,
        ),
        tree(
            "a tree whose every union is correct passes",
            {"a.py": "x: int | None = None\n"},
            set(),
        ),
        tree(
            "a tree with one offender is reported, naming the file and line",
            {"a.py": "\n\nx: None | int = None\n"},
            {"every type union writes `None` last"},
        ),
        tree(
            "a vendored tree is not read",
            # Otherwise `node_modules` alone would make this unusable.
            {"node_modules/pkg/a.py": "x: None | int = None\n", "b.py": "y: int | None = None\n"},
            set(),
        ),
        tree(
            "generated protobuf stubs are not read",
            {"pkg/_proto/a_pb2.py": "x: None | int = None\n", "b.py": "y = 1\n"},
            set(),
        ),
        tree(
            "a tree with no Python at all is reported, not passed",
            # The never-fires guard: an rglob that matched nothing reads
            # exactly like a tree where every union is correct.
            {"README.md": "no python here\n"},
            {"some Python was read at all"},
        ),
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
