#!/usr/bin/env python3
"""Run every real-tree mutation in `scripts/mutations.json`.

A guard's fixture tests prove its rules refuse what they should, over a tree
the test builds. Only the guard, run against the *real* tree with the real tree
broken, proves the rules are pointed at anything — and three sessions' worth of
findings say that distinction is not academic:

- `check_renamed_column.py` passed all fifteen fixture cases while its main
  rule matched a line in an unrelated test;
- `check_toolchain_pins.py` had never seen the one file in this repository that
  installs pinned Go tools, and reported on itself instead;
- `check_guard_scope.py` checked half of its own property.

None of those is visible from a fixture, and until
`ledger/2026-09-29-the-dialect-mutate-py-was-missing.md` none could be run
through `scripts/mutate.py` at all. The mutations that found them were run
once, by hand, and then sat in `ledger/mutations/` as history. This is what
re-runs them.

**It is not in `scripts/check.sh` and should not be.** Each case restores the
file it mutated, but between the patch and the restore the working tree is
wrong, which is not a thing to do beside another session's work or on every
commit. It takes about a minute per suite. Run it deliberately — after
changing a guard, or on a schedule — and read
`scripts/check_mutations_roster.py`'s verdict on whether the roster is still
complete, which *is* in `check.sh` because it reads files and changes nothing.

Run directly: `python3 scripts/run_mutations.py [guard ...]`.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROSTER = ROOT / "scripts" / "mutations.json"


def spec(suite: dict) -> dict:
    """One suite as `scripts/mutate.py` wants it."""
    return {
        "file": suite["file"],
        "dialect": "python",
        "command": ["python3", "scripts/mutate_guard.py", f"scripts/{suite['guard']}.py"],
        "cases": suite["cases"],
    }


def main(argv: list[str]) -> int:
    roster = json.loads(ROSTER.read_text())
    wanted = set(argv[1:])
    suites = [s for s in roster["suites"] if not wanted or s["guard"] in wanted]

    if wanted - {s["guard"] for s in roster["suites"]}:
        print(
            f"no suite for {sorted(wanted - {s['guard'] for s in roster['suites']})}",
            file=sys.stderr,
        )
        return 70
    if not suites:
        # The never-fires case. An empty roster runs nothing and, without this,
        # prints the same closing line as one that ran seventeen.
        print("no suites to run", file=sys.stderr)
        return 70

    failed = []
    for at, suite in enumerate(suites, start=1):
        print(f"\n--- [{at}/{len(suites)}] {suite['guard']} against {suite['file']}")
        done = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "mutate.py")],
            input=json.dumps(spec(suite)),
            text=True,
            cwd=ROOT,
        )
        if done.returncode != 0:
            failed.append(suite["guard"])

    print()
    for guard in failed:
        print(f"FAIL  {guard}", file=sys.stderr)
    print(f"{len(suites) - len(failed)} suites clean, {len(failed)} with findings")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
