#!/usr/bin/env python3
"""`check_proto_copies.py`'s own tests.

A guard that cannot fail is a guard nobody has debugged, which is the lesson
`ci.yml` itself taught this repository. So: a matching pair passes, a drifted
one fails, and a missing one fails — each proved by building the tree rather
than by reading the script.

Three of these cases exist because the guard stopped holding a hand-written
list of pairs and started walking the two trees. A list cannot be wrong about a
file it does not mention; a walk can, so the *set* of files is now a claim and
has to be tested from both sides. `a second pair, one of them drifted` is the
case the list version could not have had at all — it held one pair, and the
two files it did not name had been duplicated the whole time.

Run directly: `python3 scripts/test_check_proto_copies.py`.
"""

from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile

SCRIPT = pathlib.Path(__file__).resolve().parent / "check_proto_copies.py"
CANONICAL = "crates/slate-server/proto"
MIRROR = "clients/typescript/proto"

#: One file's worth of proto, and a second that differs from it by one byte,
#: because a field added to one side and not the other is exactly this and
#: nothing louder.
ONE = "message A { uint32 a = 1; }\n"
OTHER = "message A { uint32 a = 2; }\n"


def run(root: pathlib.Path) -> subprocess.CompletedProcess[str]:
    target = root / "scripts" / SCRIPT.name
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(SCRIPT.read_text())
    return subprocess.run(
        [sys.executable, str(target)], capture_output=True, text=True, check=False
    )


def build(root: pathlib.Path, files: dict[str, str]) -> None:
    """`{"<tree>/<path within it>": text}` — an absent key is an absent file."""
    for relative, text in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def main() -> int:
    cases: list[tuple[str, dict[str, str], int]] = [
        (
            "a matching pair",
            {f"{CANONICAL}/slate/v1/records.proto": ONE,
             f"{MIRROR}/slate/v1/records.proto": ONE},
            0,
        ),
        (
            "a copy that drifted",
            {f"{CANONICAL}/slate/v1/records.proto": ONE,
             f"{MIRROR}/slate/v1/records.proto": OTHER},
            1,
        ),
        (
            # Whitespace counts: a reformatted copy is a copy somebody edited.
            "a copy reformatted",
            {f"{CANONICAL}/slate/v1/records.proto": "message A {\n  uint32 a = 1;\n}\n",
             f"{MIRROR}/slate/v1/records.proto": ONE},
            1,
        ),
        (
            "a missing copy",
            {f"{CANONICAL}/slate/v1/records.proto": ONE},
            1,
        ),
        (
            "a copy with no canonical file",
            {f"{MIRROR}/slate/v1/records.proto": ONE},
            1,
        ),
        (
            # The case the hand-written list could not have. Two pairs where
            # the roster named one: the second is checked because the trees are
            # walked, and this is the real defect that was sitting there —
            # google/rpc/*.proto duplicated across both trees, unnamed.
            "a second pair, one of them drifted",
            {f"{CANONICAL}/slate/v1/records.proto": ONE,
             f"{MIRROR}/slate/v1/records.proto": ONE,
             f"{CANONICAL}/google/rpc/status.proto": ONE,
             f"{MIRROR}/google/rpc/status.proto": OTHER},
            1,
        ),
        (
            "a second pair, both matching",
            {f"{CANONICAL}/slate/v1/records.proto": ONE,
             f"{MIRROR}/slate/v1/records.proto": ONE,
             f"{CANONICAL}/google/rpc/status.proto": OTHER,
             f"{MIRROR}/google/rpc/status.proto": OTHER},
            0,
        ),
        (
            # The never-fires case: nothing found must not read as nothing
            # wrong. A walk over a tree that moved compares zero files and
            # would otherwise print the line a walk over three prints.
            "no protos under either tree",
            {"README.md": "nothing here\n"},
            1,
        ),
    ]
    passed = failed = 0
    for name, files, expected in cases:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            build(root, files)
            result = run(root)
        if result.returncode == expected:
            print(f"ok    {name}")
            passed += 1
        else:
            print(
                f"FAIL  {name}: exit {result.returncode}, wanted {expected}\n"
                f"{result.stdout}{result.stderr}"
            )
            failed += 1
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
