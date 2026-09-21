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


def run(
    scripts: dict[str, str],
    seconds: str = "3",
    budget: str = "60",
    smoke: bool = False,
) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as directory:
        sources, binaries = fixture(pathlib.Path(directory), scripts)
        try:
            finished = subprocess.run(
                ["sh", str(RUNNER), "slate-slatedb", *(["--smoke"] if smoke else [])],
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
                    # Sixty by default, so nothing here waits out the real
                    # fifteen minutes; the two cases that are *about* the
                    # budget set it to a second or two.
                    "EXAMPLE_SECONDS": budget,
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

#: An example that never returns and is in nobody's roster.
#:
#: The case the budget exists for. `check_examples_roster.py` warns when a
#: *source* matches `future::pending` or `signal::ctrl_c`, and a third way of
#: never returning — a `recv()` nothing sends to, a joined thread that never
#: exits — would slip past it and hang the loop. In CI that is the job's own
#: six-hour limit with no line saying which example did it.
#:
#: It outlives the budget these cases give it, on purpose, for the same reason
#: `MUTE` outlives the handshake wait: an example that exits on its own proves
#: nothing about the timeout.
#:
#: Twenty seconds rather than `MUTE`'s six hundred, and the difference is
#: measured. A mutation that removes the budget leaves the runner waiting for
#: this to finish, and *both* numbers catch that — but at 600 the wait is the
#: 120-second outer timeout in `run` above, three cases over, and a mutation
#: run over this file took more than ten minutes. At 20 the example exits on
#: its own, the run reports eight passing where the case wants a kill, and the
#: mutation is caught in a fifth of the time.
STUCK = "#!/bin/sh\nexec sleep 20\n"

#: A binary that passes only if `--smoke` set every size knob it looks for.
#:
#: The export list in `run_examples.sh` is three names kept by hand, and a
#: fourth crate's knob is exactly the kind of thing that gets declared and not
#: exported. Nothing would say so: an example that does not see its knob runs
#: at its recorded size, which is slower and still green. The per-example
#: budget below would eventually catch it as a timeout; this catches it as the
#: thing it is.
KNOBS = (
    "#!/bin/sh\n"
    'test "$KERNELBENCH_ROWS" = 500 || { echo "no KERNELBENCH_ROWS"; exit 1; }\n'
    'test "$SCALE_ROWS" = 2000 || { echo "no SCALE_ROWS"; exit 1; }\n'
    'test "$HEADBENCH_ROWS" = 500 || { echo "no HEADBENCH_ROWS"; exit 1; }\n'
)

#: name, the fixture, the handshake wait, the per-example budget, whether to
#: pass `--smoke`, the exit code, and the text the report must carry.
CASES: list[tuple[str, dict[str, str], str, str, bool, int, str]] = [
    (
        "every example passing reports every one",
        passing(LEAST),
        "3",
        "60",
        False,
        0,
        f"{LEAST} passed, 0 failed",
    ),
    (
        "one failing example is counted, and the rest still run",
        {**passing(LEAST - 1), "broken": "#!/bin/sh\nexit 3\n"},
        "3",
        "60",
        False,
        1,
        f"{LEAST - 1} passed, 1 failed",
    ),
    (
        "and it is named with its exit code",
        {**passing(LEAST - 1), "broken": "#!/bin/sh\nexit 3\n"},
        "3",
        "60",
        False,
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
        "60",
        False,
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
        "60",
        False,
        1,
        f"found {LEAST - 1} examples",
    ),
    (
        "a server is checked by its handshake rather than run to completion",
        # `s3_server` serves until killed. Without the roster this hangs; with
        # it, the run finishes and says what it saw.
        {**passing(LEAST - 1), "s3_server": SERVER},
        "5",
        "60",
        False,
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
        "60",
        False,
        1,
        "FAIL  s3_server, no LISTENING",
    ),
    (
        "a server that takes a moment to bind is waited for",
        {**passing(LEAST - 1), "s3_server": SLOW},
        "20",
        "60",
        False,
        0,
        "ok    s3_server, said LISTENING",
    ),
    (
        "a server that exits at once fails rather than waiting out the clock",
        {**passing(LEAST - 1), "s3_server": "#!/bin/sh\nexit 1\n"},
        "30",
        "60",
        False,
        1,
        "FAIL  s3_server, no LISTENING",
    ),
    (
        "an example that never returns is killed and named, not waited on",
        {**passing(LEAST - 1), "stuck": STUCK},
        "3",
        "2",
        False,
        1,
        "FAIL  stuck, still running after 2s",
    ),
    (
        "and the report says what to do if it never returns by design",
        {**passing(LEAST - 1), "stuck": STUCK},
        "3",
        "2",
        False,
        1,
        "give it a handshake line",
    ),
    (
        "the rest of the run still happens after one is killed",
        # The same argument as the failing-exit-code cases: the point of
        # running all of them is knowing how many are broken. A timeout that
        # aborted the loop would report one failure and seven unknowns.
        {**passing(LEAST - 1), "stuck": STUCK},
        "3",
        "2",
        False,
        1,
        f"{LEAST - 1} passed, 1 failed",
    ),
    (
        # The bug the `code=$?` comment in the runner records: reading `$?` in
        # an `elif` test and again after it reports exit 1 for everything.
        "a failing example is still reported with its own exit code",
        {**passing(LEAST - 1), "broken": "#!/bin/sh\nexit 7\n"},
        "3",
        "60",
        False,
        1,
        "FAIL  broken, exit 7",
    ),
    (
        "--smoke sets every size knob the examples read",
        {**passing(LEAST - 1), "knobs": KNOBS},
        "3",
        "60",
        True,
        0,
        f"{LEAST} passed, 0 failed",
    ),
    (
        # The other direction, and the one that makes the case above mean
        # something: without `--smoke` the same binary must fail, or it would
        # pass on a runner that exported nothing.
        "and a run without it leaves them unset",
        {**passing(LEAST - 1), "knobs": KNOBS},
        "3",
        "60",
        False,
        1,
        "no KERNELBENCH_ROWS",
    ),
]


def main() -> int:
    failed = 0
    for name, scripts, seconds, budget, smoke, expected, wanted in CASES:
        code, said = run(scripts, seconds, budget, smoke)
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
