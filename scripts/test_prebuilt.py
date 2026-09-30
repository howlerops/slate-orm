#!/usr/bin/env python3
"""`scripts/prebuilt.py`, and a roster of everything that must use it.

Two halves, and the second is the one that matters.

The first is the ordinary one: `complaint` over written trees, because run
only against `slate-orm` it would pass for as long as the tree stayed correct,
which is also what a rule that does nothing does.

The second is a roster. `SLATE_SERVERD` and `SLATE_TESTSERVER` are read in six
places, and on 2026-09-30 three of them refused a stale binary and three did
not — including `examples/explorer/run.sh`, which is what the three-SDK
conformance runner starts. A mutation of `crates/slate-server/src/status.rs`
survived that runner for exactly that reason: the mutated source never ran.

So the roster is the point. A seventh harness that takes the variable and
forgets the refusal is the failure this file exists to make loud, and it
cannot be caught by testing `prebuilt.py` itself.

Run directly: `python3 scripts/test_prebuilt.py`.
"""

from __future__ import annotations

import pathlib
import re
import sys
import tempfile
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import prebuilt

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: Every file that reads a `SLATE_*` binary variable, and the text that shows
#: it refuses a stale one.
#:
#: Written out rather than derived, because "does this file refuse a stale
#: binary" is not something a grep can decide — but "does this file mention
#: the refusal at all" is, and the pairing is what makes the roster's own
#: staleness visible: a file here that stops reading the variable is reported
#: just as loudly as one that starts reading it and is not here.
MUST_REFUSE: dict[str, str] = {
    "clients/python/tests/conftest.py": "refuse_if_stale",
    "clients/go/slate/harness_test.go": "was built before",
    "clients/typescript/test/harness.ts": "refuseIfStale",
    "examples/explorer/run.sh": "scripts/prebuilt.py",
    "examples/retention/run.sh": "scripts/prebuilt.py",
    "examples/batchbench/run.sh": "scripts/prebuilt.py",
}

#: What a reader of the variable looks like, for the other direction.
READS = re.compile(r"SLATE_(?:SERVERD|TESTSERVER)")

#: Files that name the variable and are not harnesses that start one, with the
#: reason each is not. Kept short on purpose: every entry is a place the
#: roster above cannot see.
NOT_A_HARNESS: dict[str, str] = {
    "scripts/check.sh": "passes the variable through to the suites it runs",
    "scripts/prebuilt.py": "is the refusal",
    "scripts/test_prebuilt.py": "is this file",
    "scripts/codegen.py": "runs the binary to print a catalog; it writes no tests",
    "scripts/test_codegen.py": "covers `codegen.py`, which is exempt above",
    "scripts/check_closed_caveats.py": "cites the variable in a witness needle",
    "scripts/test_check_sh.py": "names the variable in prose about `check.sh`",
    "CLAUDE.md": "documents the variable for a reader; it starts nothing",
    "crates/slate-serverd/tests/refusals.rs": (
        "matches on `SLATE_SERVERD_NO_SUCH_VARIABLE`, a deliberately absent "
        "secret's name. The prefix is a coincidence of spelling"
    ),
    "scripts/mutate.py": "its docstring names a stale `SLATE_TESTSERVER` as one of the lies",
    "scripts/test_mutate.py": "covers `mutate.py`, which is exempt above",
}

RESULTS: list[bool] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"{'ok  ' if ok else 'FAIL'}  {name}")
    if not ok and detail:
        print(f"        {detail}")
    RESULTS.append(ok)


def written(newer: bool) -> tuple[pathlib.Path, pathlib.Path]:
    """A tree with one source file and one binary, in the order asked for."""
    root = pathlib.Path(tempfile.mkdtemp())
    (root / "crates").mkdir()
    source = root / "crates" / "lib.rs"
    binary = root / "serverd"
    if newer:
        binary.write_text("x")
        time.sleep(0.01)
        source.write_text("fn main() {}")
    else:
        source.write_text("fn main() {}")
        time.sleep(0.01)
        binary.write_text("x")
    return source, binary


def main() -> int:
    source, binary = written(newer=True)
    said = prebuilt.complaint(binary, "SLATE_SERVERD", source.parents[1])
    check(
        "a binary older than the source is refused",
        said is not None and "was built before" in said,
        f"got {said!r}",
    )

    source, binary = written(newer=False)
    check(
        "a binary newer than the source is allowed",
        prebuilt.complaint(binary, "SLATE_SERVERD", source.parents[1]) is None,
    )

    # A path that is set and missing is a hard error everywhere else in this
    # repository, and silence here would let a harness fall back to building.
    missing = binary.parent / "not-there"
    said = prebuilt.complaint(missing, "SLATE_SERVERD", source.parents[1])
    check(
        "a path that is set and missing is refused, not passed over",
        said is not None and "is not a file" in said,
        f"got {said!r}",
    )

    # The never-fires half: a tree with no source at all cannot say anything
    # about staleness, and returning "fine" there is right — but a tree where
    # `SUFFIXES` stopped matching looks identical, so the emptiness is
    # reported by the roster below rather than swallowed here.
    empty = pathlib.Path(tempfile.mkdtemp())
    check(
        "a tree with no source files finds no newest one",
        prebuilt.newest_source(empty) is None,
    )
    check(
        "the real tree does have one, so `SUFFIXES` still matches",
        prebuilt.newest_source(ROOT) is not None,
    )

    # --- the roster, which is why this file exists ------------------------
    for name, needle in MUST_REFUSE.items():
        path = ROOT / name
        if not path.is_file():
            check(f"{name} is still in the tree", False, "the roster names a file that is gone")
            continue
        body = path.read_text(encoding="utf-8")
        check(
            f"{name} refuses a stale prebuilt binary",
            needle in body,
            f"it does not mention {needle!r}; a harness that takes the variable "
            "and skips the refusal tests a server this tree did not produce",
        )
        check(
            f"{name} still reads the variable, so its roster row is live",
            bool(READS.search(body)),
            "it no longer reads SLATE_SERVERD or SLATE_TESTSERVER; drop the row",
        )

    # And the other direction: a reader that is on neither list.
    known = set(MUST_REFUSE) | set(NOT_A_HARNESS)
    strays = []
    for path in ROOT.rglob("*"):
        parts = path.parts
        if not path.is_file() or "node_modules" in parts or "target" in parts:
            continue
        if ".git" in parts or "ledger" in parts or path.suffix not in {
            ".py", ".sh", ".go", ".ts", ".yml", ".md", ".rs",
        }:
            continue
        name = str(path.relative_to(ROOT))
        if name in known or name.startswith((".github/", "docs/", "site/")):
            continue
        try:
            if READS.search(path.read_text(encoding="utf-8")):
                strays.append(name)
        except UnicodeDecodeError:  # pragma: no cover - binaries in the tree
            continue
    check(
        "every file that reads the variable is on a list",
        not strays,
        f"not in MUST_REFUSE and not in NOT_A_HARNESS: {', '.join(sorted(strays))}",
    )

    # The roster's own never-fires half: an empty `MUST_REFUSE` would make
    # every loop above vacuous and this file would print only passes.
    check("the roster is not empty", bool(MUST_REFUSE))

    failed = RESULTS.count(False)
    print(f"\n{len(RESULTS) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
