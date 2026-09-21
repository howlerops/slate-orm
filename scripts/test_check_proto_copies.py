#!/usr/bin/env python3
"""`check_proto_copies.py`'s own tests.

A guard that cannot fail is a guard nobody has debugged, which is the lesson
`ci.yml` itself taught this repository. So: a matching pair passes, a drifted
one fails, and a missing one fails — each proved by building the tree rather
than by reading the script.
"""

from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile

SCRIPT = pathlib.Path(__file__).resolve().parent / "check_proto_copies.py"
SOURCE = "crates/slate-server/proto/slate/v1/records.proto"
COPY = "clients/typescript/proto/slate/v1/records.proto"


def run(root: pathlib.Path) -> subprocess.CompletedProcess[str]:
    target = root / "scripts" / SCRIPT.name
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(SCRIPT.read_text())
    return subprocess.run(
        [sys.executable, str(target)], capture_output=True, text=True, check=False
    )


def build(root: pathlib.Path, source: str | None, copy: str | None) -> None:
    for relative, text in ((SOURCE, source), (COPY, copy)):
        if text is None:
            continue
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def main() -> int:
    cases = [
        ("a matching pair", "syntax = 'proto3';\n", "syntax = 'proto3';\n", 0),
        # One byte, because a field added to one side and not the other is
        # exactly this and nothing louder.
        ("a copy that drifted", "message A { uint32 a = 1; }\n",
         "message A { uint32 a = 2; }\n", 1),
        # Whitespace counts: a reformatted copy is a copy somebody edited.
        ("a copy reformatted", "message A {\n  uint32 a = 1;\n}\n",
         "message A { uint32 a = 1; }\n", 1),
        ("a missing copy", "syntax = 'proto3';\n", None, 1),
        ("a missing source", None, "syntax = 'proto3';\n", 1),
    ]
    passed = failed = 0
    for name, source, copy, expected in cases:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            build(root, source, copy)
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
