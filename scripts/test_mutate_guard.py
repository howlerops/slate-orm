#!/usr/bin/env python3
"""Tests for `mutate_guard.py`, over guards this one writes.

Run against the real guards, every case would be "it passed" — and the cases
that matter are the ones where a guard *cannot* answer, which the real tree
never produces on demand. The one real-tree case here is the baseline: every
guard in `scripts/` that needs nothing but Python prints something and exits 0,
which is what the "a silent pass is not a pass" rule assumes and is worth
pinning rather than believing. The ones that need more are in `TOOLCHAIN`.

Run directly: `python3 scripts/test_mutate_guard.py`.
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import mutate

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
RUNNER = HERE / "mutate_guard.py"

#: What `scripts/mutate.py`'s `python` dialect reads as proof a suite ran,
#: imported rather than copied. Every "died" case below asserts this pattern is
#: *absent*, and a test asserting the absence of a pattern that had drifted
#: from the real one would pass for the wrong reason — which is the same
#: offence as a guard passing on a word. A first draft used the substring
#: `" passed, "` and matched this runner's own prose about a guard that
#: "passed, so no result line is printed", failing six cases on the message
#: rather than the behaviour.
REPORT = mutate.DIALECTS["python"][1]

#: Real guards the baseline sweep leaves out, because they need a toolchain the
#: sweep's two homes promise not to: `scripts/check.sh` runs with no npm and no
#: `node_modules`, and CI's `scripts` job installs Python and nothing else.
#:
#: `check_npm_package` asks `npm pack` what the tarball would hold, and `npm
#: pack` runs `prepack`, which runs `tsc`. Swept here it exited 2 on every push
#: from the commit that added it — `main` went red on the guard and on nothing
#: in the tree. Installing Node in the `scripts` job would have fixed CI and
#: left `check.sh` broken on every machine without it, which is the promise
#: that script is built on.
#:
#: An entry is a hole in the sweep, so it is held to two things below: the
#: guard still exists (a stale name exempts nothing and hides that it does), and
#: `ci.yml` still runs it somewhere that has the toolchain. Leaving the
#: exemption and losing the step would otherwise mean the guard ran nowhere.
TOOLCHAIN = {
    "check_npm_package": "needs npm and an installed node_modules; CI's `typescript` job runs it",
}

GUARDS = {
    "passes": "import sys\nprint('ok    fine')\nsys.exit(0)\n",
    "refuses": "import sys\nprint('a problem', file=sys.stderr)\nsys.exit(1)\n",
    # The shape every real guard in this repository has: its passing rules
    # print first and the refusing one last. Quoting the *first* line of a
    # refusal therefore shows a sentence beginning `ok`, which is what the
    # roster's first full run did and what `a refusal quotes the line that
    # refused` below pins.
    "refuses_after_passing": (
        "import sys\n"
        "print('ok    the first rule held')\n"
        "print('FAIL  the second rule did not', file=sys.stderr)\n"
        "sys.exit(1)\n"
    ),
    "raises": "raise ValueError('boom')\n",
    "exits_two": "import sys\nprint('confused')\nsys.exit(2)\n",
    "silent": "import sys\nsys.exit(0)\n",
    "prints_a_traceback_and_passes": (
        "import sys, traceback\n"
        "try:\n"
        "    raise ValueError('swallowed')\n"
        "except ValueError:\n"
        "    traceback.print_exc()\n"
        "print('ok    fine really')\n"
        "sys.exit(0)\n"
    ),
}


def run(*guards: str, root: pathlib.Path | None = None) -> tuple[int, str]:
    done = subprocess.run(
        [sys.executable, str(RUNNER), *guards],
        cwd=root or ROOT,
        capture_output=True,
        text=True,
    )
    return done.returncode, done.stdout + done.stderr


def written(directory: pathlib.Path) -> None:
    """The fixture guards, written where `mutate_guard.py` resolves them.

    It resolves relative to the repository root it computes from its own
    location, so the fixture has to live under `scripts/` rather than in a
    temporary directory of its own. They are removed in the `finally` below.
    """
    for name, body in GUARDS.items():
        (directory / f"{name}.py").write_text(body)


CASES = [
    ("a guard that passes reports one pass", ["passes"], 0, ["ok    passes", "1 passed, 0 failed"]),
    (
        "a guard that refuses reports a failure mutate.py can read",
        ["refuses"],
        1,
        ["FAIL  refuses: a problem", "0 passed, 1 failed"],
    ),
    (
        # A refusal quotes the line that refused, not the first line printed.
        # Found by the mutation roster's first full run, where
        # `check_caveat_citations` was reported red with an `ok` sentence
        # beside it and the reader went looking for a parser bug.
        "a refusal quotes the line that refused",
        ["refuses_after_passing"],
        1,
        ["FAIL  refuses_after_passing: FAIL  the second rule did not"],
    ),
    (
        "two guards, one of each, are counted separately",
        ["passes", "refuses"],
        1,
        ["ok    passes", "FAIL  refuses", "1 passed, 1 failed"],
    ),
    (
        "a guard that raises produces no result line at all",
        ["raises"],
        70,
        ["raised rather than refused"],
    ),
    (
        "a guard that exits 2 produces no result line at all",
        ["exits_two"],
        70,
        ["exited 2, which is neither a pass nor a refusal"],
    ),
    (
        "a guard that passes silently produces no result line at all",
        ["silent"],
        70,
        ["exited 0 and printed nothing"],
    ),
    (
        "a guard that prints a traceback and exits 0 produces no result line",
        ["prints_a_traceback_and_passes"],
        70,
        ["raised rather than refused"],
    ),
    (
        "one guard that cannot answer suppresses the whole run's result line",
        ["passes", "raises"],
        70,
        ["ok    passes", "raised rather than refused"],
    ),
    (
        "a guard that is not there produces no result line at all",
        ["scripts/check_nothing_like_this.py"],
        70,
        ["does not exist"],
    ),
    ("naming no guard at all fails", [], 70, ["name at least one"]),
]


def main() -> int:
    """Every case, plus the real tree once.

    A case wanting exit 70 also has its output checked for the absence of
    `REPORT`, derived from the exit code rather than listed per case: 70 is the
    only code this runner uses for "could not judge", so a case added with it
    gets the check that matters most for free.
    """
    written(HERE)
    failures = []
    try:
        for name, guards, wanted, needles in CASES:
            paths = [g if "/" in g else f"scripts/{g}.py" for g in guards]
            code, output = run(*paths)
            if code != wanted:
                failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
                continue
            absent = [n for n in needles if n not in output]
            if absent:
                failures.append(f"FAIL  {name}: no {absent!r} in\n{output}")
            elif wanted == 70 and REPORT.search(output):
                failures.append(
                    f"FAIL  {name}: printed a line matching {REPORT.pattern!r}, "
                    f"which mutate.py reads as a suite that reported\n{output}"
                )
            else:
                print(f"ok    {name}")

        # The real tree, once: every guard prints something and exits 0, which
        # is what "a silent pass is not a pass" assumes about them.
        real = sorted(p.name for p in HERE.glob("check_*.py") if not p.name.startswith("test_"))
        real = [
            f"scripts/{n}"
            for n in real
            if n[len("check_") : -3] not in GUARDS and n[:-3] not in TOOLCHAIN
        ]
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        for name in sorted(TOOLCHAIN):
            if not (HERE / f"{name}.py").exists():
                failures.append(
                    f"FAIL  TOOLCHAIN names {name}, which is not in scripts/; remove the entry"
                )
            # A `run:` line, not a mention: ci.yml also names this guard in a
            # comment above the step, and a substring check stayed green with
            # the step deleted.
            elif not re.search(rf"^\s*(?:-\s+)?run:.*\bscripts/{name}\.py\b", workflow, re.M):
                failures.append(
                    f"FAIL  TOOLCHAIN leaves {name} out of the sweep, and ci.yml no longer runs it,\n"
                    f"      so it runs nowhere. Restore the step or remove the exemption."
                )
            else:
                print(f"ok    {name} is left to ci.yml: {TOOLCHAIN[name]}")
        code, output = run(*real)
        if code != 0 or f"{len(real)} passed, 0 failed" not in output:
            failures.append(f"FAIL  every real guard passes and says so: exit {code}\n{output}")
        else:
            print(f"ok    every real guard passes and says so  ({len(real)} of them)")
    finally:
        for name in GUARDS:
            (HERE / f"{name}.py").unlink(missing_ok=True)

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) + 1 + len(TOOLCHAIN) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
