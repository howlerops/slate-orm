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

# The roster used to grep, and grep cannot see a refusal that does not fire

`ledger/2026-09-30-the-exemptions-i-wrote-without-reading.md` recorded that as
the roster's own limit, in the entry that built it:

  > **The roster cannot tell whether a file's refusal works** — only that it
  > mentions one. A harness that imports `prebuilt` and never calls it passes.

Every row now names a **drive**: something that reaches that harness's own
binary selection with a deliberately stale path and must refuse. Three of the
six are shell scripts and one is a Python module, so this file drives those
four itself, every run, in about a second. The Go and TypeScript harnesses
need their own toolchains, which this file cannot assume — `check.sh` runs
with neither — so their drive is a named test in that client's own suite, and
the roster checks the test is there. That split is written into the roster
rather than implied, because the two halves are worth different amounts: four
are executed here and two are executed where the toolchain is.

What is left is what no roster reaches: a drive that exists, runs and asserts
the wrong thing. That is the same cost `check_retired_claims.py` states about
its registry.

Run directly: `python3 scripts/test_prebuilt.py`.
"""

from __future__ import annotations

import os
import pathlib
import re
import subprocess
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
MUST_REFUSE: dict[str, dict[str, str]] = {
    "clients/python/tests/conftest.py": {
        "needle": "refuse_if_stale",
        # `_build` is the function the fixtures call to get a binary. Loaded
        # by path and called with a stale `SLATE_TESTSERVER`, so no server
        # starts and no test collects.
        "drive": "python",
        "entry": "_build",
    },
    "clients/go/slate/harness_test.go": {
        "needle": "was built before",
        # Driven by `go test`, which this file cannot run: `check.sh` needs no
        # toolchain, and a drive that silently skips when `go` is missing is
        # the green skip `CLAUDE.md` warns about.
        "drive": "test",
        "entry": "clients/go/slate/prebuilt_test.go::TestAStalePrebuiltBinaryIsRefused",
    },
    "clients/typescript/test/harness.ts": {
        "needle": "refuseIfStale",
        "drive": "test",
        "entry": (
            "clients/typescript/test/prebuilt.test.ts::"
            "a prebuilt binary older than the source is refused"
        ),
    },
    "examples/explorer/run.sh": {
        "needle": "scripts/prebuilt.py",
        "drive": "shell",
        # No mode argument: the refusal sits above everything that starts a
        # process, so the script reaches it and exits without binding a port.
        "entry": "",
    },
    "examples/retention/run.sh": {
        "needle": "scripts/prebuilt.py",
        "drive": "shell",
        "entry": "",
    },
    "examples/batchbench/run.sh": {
        "needle": "scripts/prebuilt.py",
        "drive": "shell",
        "entry": "",
    },
}

#: What a refusal has to say. Shared by every drive, because a harness that
#: exits non-zero for some *other* reason — a missing port, a bad argument —
#: would otherwise read as a refusal, and that is the failure mode of testing
#: an exit status rather than an answer.
REFUSAL = "was built before"

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
    "clients/go/slate/prebuilt_test.go": (
        "is the Go harness's drive: it sets no variable, it calls the "
        "selection the variable feeds"
    ),
    "clients/typescript/test/prebuilt.test.ts": "is the TypeScript harness's drive",
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


def stale() -> pathlib.Path:
    """A file inside this repository, timestamped in 2000.

    Inside the repository on purpose. Every refusal walks up to the checkout
    root and compares against the newest source there; a binary in the system
    temp directory has no checkout above it, so each refusal returns "fine"
    for the legitimate reason it documents — a released binary, no sources
    beside it — and a drive placed there passes without the refusal firing.

    Under `target/`, which every walk skips as build output, so the file
    cannot become the newest source it is being compared against.
    """
    directory = ROOT / "target" / "prebuilt-drive"
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "slate-serverd"
    path.write_text("not a real binary")
    path.chmod(0o755)
    os.utime(path, (946684800, 946684800))  # 2000-01-01
    return path


def drive(name: str, row: dict[str, str]) -> None:
    """Reach `name`'s own binary selection with a stale path; it must refuse."""
    how = row["drive"]
    if how == "test":
        # The two whose drive needs a toolchain this file cannot assume. A
        # drive that skipped when `go` or `node` is missing would be green,
        # and a green skip is what `CLAUDE.md` says to prefer a hard error to.
        where, _, named = row["entry"].partition("::")
        body = (ROOT / where).read_text(encoding="utf-8") if (ROOT / where).is_file() else ""
        check(
            f"{name} has a test that drives its refusal: {where}",
            named in body,
            f"{where} does not contain {named!r}. The roster can see that "
            f"{name} mentions a refusal; only that test sees it fire.",
        )
        return

    binary = stale()
    try:
        if how == "shell":
            variable, argv = "SLATE_SERVERD", ["bash", str(ROOT / name)]
            if row["entry"]:
                argv.append(row["entry"])
        else:
            variable = "SLATE_TESTSERVER"
            argv = [
                sys.executable,
                "-c",
                # Loaded by path and called directly. Importing it as a module
                # would need `clients/python` on the path and would still not
                # reach the selection, which is a function the fixtures call.
                "import importlib.util,sys;"
                f"spec=importlib.util.spec_from_file_location('h', {str(ROOT / name)!r});"
                "m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);"
                f"m.{row['entry']}()",
            ]
        result = subprocess.run(
            argv,
            cwd=ROOT,
            env={**os.environ, variable: str(binary)},
            capture_output=True,
            text=True,
            timeout=180,
            stdin=subprocess.DEVNULL,
        )
        said = result.stdout + result.stderr
        check(
            f"{name} refuses a stale prebuilt binary when driven",
            result.returncode != 0 and REFUSAL in said,
            f"exited {result.returncode} saying {said.strip()[-300:]!r}. A "
            f"harness that names the refusal and does not call it looks "
            f"exactly like one that does, until it is driven.",
        )
    finally:
        binary.unlink(missing_ok=True)
        binary.parent.rmdir()


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
    for name, row in MUST_REFUSE.items():
        path = ROOT / name
        if not path.is_file():
            check(f"{name} is still in the tree", False, "the roster names a file that is gone")
            continue
        body = path.read_text(encoding="utf-8")
        check(
            f"{name} mentions a refusal",
            row["needle"] in body,
            f"it does not mention {row['needle']!r}; a harness that takes the "
            "variable and skips the refusal tests a server this tree did not produce",
        )
        check(
            f"{name} still reads the variable, so its roster row is live",
            bool(READS.search(body)),
            "it no longer reads SLATE_SERVERD or SLATE_TESTSERVER; drop the row",
        )
        drive(name, row)

    # And the other direction: a reader that is on neither list.
    known = set(MUST_REFUSE) | set(NOT_A_HARNESS)
    strays = []
    for path in ROOT.rglob("*"):
        parts = path.parts
        # `dist-test` is the TypeScript suite compiled; it is build output in
        # the same way `target` is, and a copy of a file already on a list.
        if not path.is_file() or "node_modules" in parts or "target" in parts:
            continue
        if "dist" in parts or "dist-test" in parts:
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
