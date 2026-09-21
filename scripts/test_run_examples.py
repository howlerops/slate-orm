#!/usr/bin/env python3
"""Tests for `run_examples.sh`, over directories this one writes.

Run against the real crates — where every example passes — a mutation to the
floor, the counting or the handshake check survives, because nothing in a green
run distinguishes a runner that counts failures from one that ignores them.
`scripts/mutate.py` reported exactly that about three of them, which is why
`EXAMPLES_DIR`, `BINARIES_DIR` and `SKIP_BUILD` exist: with a directory of
binaries this file wrote, every one of those properties has a case.

The fixtures use the crate name `slate-slatedb` because the floor is per-crate
and read from the script's own `case`. A test-only crate name would have meant
a test-only branch in the thing being tested.

Run directly: `python3 scripts/test_run_examples.py`.
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
RUNNER = ROOT / "scripts/run_examples.sh"

#: The floor `run_examples.sh` holds `slate-slatedb` to.
LEAST = 8


def fixture(directory: pathlib.Path, scripts: dict[str, str]) -> tuple[pathlib.Path, pathlib.Path]:
    """A directory of `.rs` names and a directory of runnable stand-ins."""
    sources = directory / "examples"
    binaries = directory / "bin"
    sources.mkdir()
    binaries.mkdir()
    for name, body in scripts.items():
        (sources / f"{name}.rs").write_text("fn main() {}\n")
        binary = binaries / name
        binary.write_text(body)
        binary.chmod(0o755)
    return sources, binaries


def run(scripts: dict[str, str], seconds: str = "3") -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as directory:
        sources, binaries = fixture(pathlib.Path(directory), scripts)
        try:
            finished = subprocess.run(
                ["sh", str(RUNNER), "slate-slatedb"],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
                timeout=120,
                env={
                    **os.environ,
                    "EXAMPLES_DIR": str(sources),
                    "BINARIES_DIR": str(binaries),
                    "SKIP_BUILD": "1",
                    "HANDSHAKE_SECONDS": seconds,
                },
            )
        except subprocess.TimeoutExpired:
            # A hang is a failure, not a wait. The fixture's server loops
            # forever by design, so a runner that stopped recognising it would
            # otherwise leave this test — and any mutation run over it —
            # waiting indefinitely rather than reporting. Found exactly that
            # way: mutating the handshake roster hung `scripts/mutate.py`.
            return 124, "timed out: the runner did not finish"
        return finished.returncode, finished.stdout + finished.stderr


def passing(count: int) -> dict[str, str]:
    return {f"ex{n}": "#!/bin/sh\nexit 0\n" for n in range(count)}


#: A stand-in for `s3_server`: prints the handshake and then serves.
#:
#: `exec sleep`, not `while true; do sleep 1; done`, so the process the runner
#: kills *is* the one that sleeps. The loop version leaves an orphaned `sleep`
#: behind every kill, which is a fixture that does not clean up after itself —
#: eight cases' worth of them, and the run that found it timed out rather than
#: failing.
SERVER = "#!/bin/sh\necho 'LISTENING 127.0.0.1:9000'\nexec sleep 60\n"

#: The same, without ever saying so — a server that failed to bind.
#:
#: It outlives this file's own 120-second timeout on purpose. A shorter one
#: exits while the runner is still waiting, and the liveness check catches it
#: whatever the budget is — so a mutation making the wait unbounded survived,
#: because the only thing the *budget* is for is a server that stays alive and
#: stays silent.
MUTE = "#!/bin/sh\nexec sleep 600\n"

#: One that takes a moment to bind, which is what a real one does.
#:
#: Every other fixture here prints its handshake before the runner's first
#: look, so a mutation shrinking the wait to nothing still found it and
#: survived. A server that is not instantaneous is the only thing that makes
#: the *waiting* observable, and a real one binds a socket first.
SLOW = "#!/bin/sh\nsleep 3\necho 'LISTENING 127.0.0.1:9000'\nexec sleep 60\n"

CASES: list[tuple[str, dict[str, str], str, int, str]] = [
    (
        "every example passing reports every one",
        passing(LEAST),
        "3",
        0,
        f"{LEAST} passed, 0 failed",
    ),
    (
        "one failing example is counted, and the rest still run",
        {**passing(LEAST - 1), "broken": "#!/bin/sh\nexit 3\n"},
        "3",
        1,
        f"{LEAST - 1} passed, 1 failed",
    ),
    (
        "and it is named with its exit code",
        {**passing(LEAST - 1), "broken": "#!/bin/sh\nexit 3\n"},
        "3",
        1,
        "FAIL  broken, exit 3",
    ),
    (
        "two failing examples are both counted",
        {
            **passing(LEAST - 2),
            "broken": "#!/bin/sh\nexit 3\n",
            "alsobroken": "#!/bin/sh\nexit 4\n",
        },
        "3",
        1,
        f"{LEAST - 2} passed, 2 failed",
    ),
    (
        "a short directory fails rather than reporting a smaller green run",
        # The never-fires half, and the one a real tree can never exercise:
        # move an example out and without the floor the loop runs the rest and
        # prints a green summary over a tree with one missing.
        passing(LEAST - 1),
        "3",
        1,
        f"found {LEAST - 1} examples",
    ),
    (
        "a server is checked by its handshake rather than run to completion",
        # `s3_server` serves until killed. Without the roster this hangs; with
        # it, the run finishes and says what it saw.
        {**passing(LEAST - 1), "s3_server": SERVER},
        "5",
        0,
        "ok    s3_server, said LISTENING",
    ),
    (
        "a server that never says it is listening fails",
        # A skip is green, which is what this exists not to be: the roster
        # entry is a claim that the binary prints something, and this is the
        # case where it does not.
        {**passing(LEAST - 1), "s3_server": MUTE},
        "3",
        1,
        "FAIL  s3_server, no LISTENING",
    ),
    (
        "a server that takes a moment to bind is waited for",
        {**passing(LEAST - 1), "s3_server": SLOW},
        "20",
        0,
        "ok    s3_server, said LISTENING",
    ),
    (
        "a server that exits at once fails rather than waiting out the clock",
        {**passing(LEAST - 1), "s3_server": "#!/bin/sh\nexit 1\n"},
        "30",
        1,
        "FAIL  s3_server, no LISTENING",
    ),
]


def main() -> int:
    failed = 0
    for name, scripts, seconds, expected, wanted in CASES:
        code, said = run(scripts, seconds)
        ok = code == expected and wanted in said
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected} and {wanted!r}, got {code}")
            for line in said.splitlines()[-12:]:
                print(f"      {line}")
    print()
    print(f"{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
