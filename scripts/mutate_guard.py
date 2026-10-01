#!/usr/bin/env python3
"""Run repository guards and report in a shape `scripts/mutate.py` can read.

`scripts/mutate.py` scores a mutation by reading a runner's output: one
pattern says a suite reported, another says something failed. It has a dialect
for each of the five runners this repository uses, and none of them is a
`scripts/check_*.py` guard — a guard prints its own summary and says everything
else with an exit code. So a mutation aimed at the *real tree* and judged by a
guard could not be run through that script at all, and
`ledger/2026-09-29-a-roster-of-test-names-is-weaker-and-worth-having.md`
recorded three that were run by hand instead, losing every protection
`mutate.py` exists to provide:

> the restore in a `finally`, the reported-suite count, the JSON spec that
> never reaches a shell — did not apply to the three checks that matter most.

That matters because a guard's fixture tests and the guard itself answer
different questions. `scripts/test_check_renamed_column.py` proves the *rules*
refuse what they should, over a tree it builds. Only the guard, against the
real clients, proves the rules are pointed at anything. The first draft of that
guard passed all fifteen fixture cases while its main rule matched a line in an
unrelated test — and mutating the real client files is what said so.

This is the missing dialect, written as a runner rather than as a sixth entry
in `mutate.py`'s table: the twenty-seven guards agree on an exit code and on
nothing else. Two print `ok    …`, several print a bare count, one leads with
the subject. Teaching `mutate.py` to read all of them would be a regex that
matches almost any line, which is the over-matching failure its own
`scripts/test_mutate.py` was written to catch — every mutation would look
caught and the run would exit 0 having judged nothing.

**A guard that dies is not a guard that refused, and this refuses to call it
one.** That is the whole reason the exit code is not simply passed through. A
mutation that breaks the file's syntax makes every guard importing it exit
non-zero, which under a naive reading scores as *caught* — a false catch, and
false catches are silent. So a traceback, a silent success, or any exit code
but 0 and 1 suppresses the `N passed, M failed` line entirely, and `mutate.py`
then reports that the command produced no test results rather than scoring.

Usage, and the shape a mutation run takes:

    python3 scripts/mutate_guard.py scripts/check_renamed_column.py

    python3 scripts/mutate.py <<'JSON'
    {"file": "clients/python/tests/test_fixture.py",
     "dialect": "python",
     "command": ["python3", "scripts/mutate_guard.py",
                 "scripts/check_renamed_column.py"],
     "cases": [{"name": "the declaration leaves the renamed-column test",
                "old": "Column(\\"comment\\", ValueType.STR),  # `note`",
                "new": "Column(\\"note\\", ValueType.STR),  # `note`"}]}
    JSON
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Which of `scripts/mutate.py`'s dialects reads this adapter's output.
#:
#: Declared rather than left for a reader to infer, because `--help` prints it
#: beside the adapter's name and inferring it means opening this file — which
#: is the gap `ledger/2026-09-29-the-tool-now-prints-its-own-adapters.md`
#: recorded when it made the adapters visible at all. The name is checked
#: against `mutate.py`'s own table by `scripts/test_mutate.py`, so a typo here
#: is a failure rather than a line of help that sends the next reader to a
#: dialect that does not exist.
DIALECT = "python"

#: What a Python traceback always starts with. Looked for in the output rather
#: than inferred from the exit code, because an uncaught exception and a
#: refusal both exit 1 — and a guard that catches its own exception, prints the
#: traceback and exits 0 is the same accident with the evidence still visible.
DIED = "Traceback (most recent call last):"


def run(guard: str) -> tuple[str, str]:
    """`(verdict, detail)` for one guard: "ok", "fail", or "died"."""
    path = ROOT / guard
    if not path.exists():
        return "died", f"{guard} does not exist"
    if path.suffix not in (".py", ".sh"):
        # Named rather than guessed: a runner this does not know would be
        # handed to the Python interpreter and die in a way that reads as a
        # guard refusing.
        return "died", f"{guard} is neither a .py nor a .sh, so this does not know how to run it"
    # A `.sh` runs under `sh`, which is the one thing in `scripts/`-shaped
    # roster that is not Python: `.githooks/test-pre-commit.sh`. Its own
    # output cannot be read by `mutate.py` directly — it prints `  FAIL  …`
    # with two leading spaces and the `python` dialect anchors at column
    # zero — so a mutation of the hook scored UNREADABLE and could not be
    # judged at all. Running it here re-emits the verdict at column zero,
    # which is exactly what this adapter is for; the alternative was
    # unindenting a suite whose format matches `check.sh`'s on purpose.
    runner = ["sh", str(path)] if path.suffix == ".sh" else [sys.executable, str(path)]
    done = subprocess.run(runner, cwd=ROOT, capture_output=True, text=True)
    output = done.stdout + done.stderr
    if DIED in output:
        return "died", f"{guard} raised rather than refused"
    if done.returncode not in (0, 1):
        return "died", f"{guard} exited {done.returncode}, which is neither a pass nor a refusal"
    if done.returncode == 0 and not output.strip():
        # A guard that checks nothing and a guard that checked and liked it
        # print the same thing here, which is the failure every guard in this
        # repository is written to avoid. None of the twenty-seven is silent.
        return "died", f"{guard} exited 0 and printed nothing"
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if done.returncode == 0:
        return "ok", lines[0] if lines else ""
    # The *refusing* line, not the first line. Every guard here prints its
    # `ok` rules before its failing one, so quoting the first line reported a
    # refusal and showed a sentence beginning `ok` — which cost a reader a
    # minute on the roster's first full run and is the same
    # names-what-it-found-not-what-the-reader-wanted shape
    # `ledger/2026-10-01-the-other-messages-that-name-what-they-found.md` is
    # about, one level out. Falls back to the first line for a guard that
    # refuses without the word, because a wrong quote beats no quote.
    refusal = next((line for line in lines if line.startswith("FAIL")), None)
    return "fail", refusal or (lines[0] if lines else "")


def main(argv: list[str]) -> int:
    guards = argv[1:]
    if not guards:
        # The never-fires case, and it is reachable: a spec whose command lost
        # its argument would otherwise run nothing, print `0 passed, 0 failed`,
        # and hand `mutate.py` a clean report of an empty run.
        print("name at least one scripts/check_*.py to run", file=sys.stderr)
        return 70

    passed, failed, died = 0, 0, []
    for guard in guards:
        verdict, detail = run(guard)
        name = Path(guard).stem
        if verdict == "ok":
            passed += 1
            print(f"ok    {name}  ({detail})")
        elif verdict == "fail":
            failed += 1
            print(f"FAIL  {name}: {detail}")
        else:
            died.append(detail)

    if died:
        for reason in died:
            print(reason, file=sys.stderr)
        print(
            "no verdict: a guard that could not run is not a guard that passed, "
            "so no result line is printed and mutate.py will say the command "
            "reported nothing rather than score this as a catch.",
            file=sys.stderr,
        )
        return 70

    print(f"{passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
