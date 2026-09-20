#!/usr/bin/env python3
"""Run a mutation, and refuse to be ambiguous about what happened.

`CLAUDE.md` asks for a mutation test on everything written here: break each
thing, confirm a *named* test fails, restore, re-verify. The discipline is
sound and doing it by hand has a failure mode that looks exactly like success.

**Three ways a hand-run mutation lies, all of them met in one session:**

1. **The patch does not apply.** An anchor string moves under `cargo fmt` and
   the replacement silently matches nothing. The suite then runs against
   *unmutated* code and passes, which reads as "the mutation survived" — and a
   survival is supposed to mean "write a test", so the lie costs a test that
   should never have been written. Three times in one day.
2. **Nothing runs.** The build fails — on this container, usually ENOSPC — and
   the grep for `FAILED` finds nothing. "No test failed" and "no test ran" are
   the same empty output.
3. **The shell eats the mutation.** A replacement containing a backtick, a
   `$`, or a bare word the shell wants to run arrives mangled or not at all.

Each is caught here rather than trusted to a reader's attention:

- the old text must occur **exactly once**, and the count is reported when not;
- the file is restored in a `finally`, so an interrupt does not leave a mutated
  tree;
- the command's output is parsed for how many suites *reported*, and zero is a
  hard error that says so rather than a quiet pass;
- the spec arrives as JSON on stdin, so no replacement ever touches a shell.

**A surviving mutation exits non-zero.** That is the point: a survival is a
finding — a missing test, or code that is redundant — and it should interrupt
whoever ran it rather than scroll past. `--expect-survivor` names the cases
where survival is the established answer, with a reason, in the
`EXPECTED_REFUSALS` idiom this repository already uses: a list you are forced
to edit is a list that stays true.

Usage:

    python3 scripts/mutate.py <<'JSON'
    {
      "file": "crates/slate-kernel/src/record.rs",
      "command": ["cargo", "test", "-p", "slate-kernel", "--no-fail-fast"],
      "cases": [
        {"name": "the restrict arm reads Hidden",
         "old": "Deleted::Visible,", "new": "Deleted::Hidden,"}
      ]
    }
    JSON

`expect_survivor` on a case takes the reason survival is correct, and inverts
that case: it then fails if the mutation *is* caught, because the reason has
stopped being true.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Lines like `test some_name ... FAILED`, which is how libtest names a failure.
FAILED = re.compile(r"^test (\S+) \.\.\. FAILED\s*$", re.MULTILINE)

#: `test result: ok. 12 passed; ...` — one per test binary that actually ran.
#:
#: Counted rather than ignored because it is the only thing that separates "the
#: suite ran and nothing failed" from "the suite never built". Those are the
#: same empty `FAILED` output and opposite conclusions.
REPORTED = re.compile(r"^test result:", re.MULTILINE)


class Mutation:
    """One replacement, and what running it did."""

    def __init__(self, spec: dict) -> None:
        self.name: str = spec["name"]
        self.old: str = spec["old"]
        self.new: str = spec["new"]
        #: A reason survival is correct, or None to require it be caught.
        self.expect_survivor: str | None = spec.get("expect_survivor")


def apply_once(path: Path, mutation: Mutation) -> str:
    """Write the mutated file, returning the original text.

    The exact-once rule is the whole reason this is a function. A replacement
    that matches zero times leaves the file untouched and the run meaningless;
    one that matches twice mutates somewhere nobody looked at.
    """
    original = path.read_text()
    found = original.count(mutation.old)
    if found != 1:
        raise SystemExit(
            f"{mutation.name}: the text to replace occurs {found} times in "
            f"{path}, not once.\n"
            "  A moved anchor is the usual cause — `cargo fmt` rewraps a call "
            "and the string stops matching.\n"
            "  Re-read the file and fix the anchor; do not run the suite, "
            "because it would pass against unmutated code."
        )
    path.write_text(original.replace(mutation.old, mutation.new, 1))
    return original


def run(command: list[str]) -> tuple[list[str], int, str]:
    """The command, its failing test names, and how many suites reported."""
    finished = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, check=False
    )
    output = finished.stdout + finished.stderr
    return FAILED.findall(output), len(REPORTED.findall(output)), output


def check(spec: dict) -> int:
    path = ROOT / spec["file"]
    command = spec["command"]
    cases = [Mutation(case) for case in spec["cases"]]

    # A baseline, because a mutation run says nothing if the suite was already
    # red. This is the check a hand-run mutation always skips and the one that
    # makes every result below mean something.
    failures, reported, output = run(command)
    if reported == 0:
        print(f"the command reported no test results at all:\n{output[-2000:]}")
        return 1
    if failures:
        print(f"the suite is red before any mutation: {failures[:5]}")
        return 1
    print(f"baseline: {reported} suites reported, none failing")

    problems = 0
    for mutation in cases:
        original = apply_once(path, mutation)
        try:
            failures, reported, output = run(command)
        finally:
            path.write_text(original)

        if reported == 0:
            errors = [x for x in output.splitlines() if x.startswith("error")][:3]
            print(f"  !! {mutation.name}: NOTHING RAN — {errors}")
            problems += 1
            continue

        caught = ", ".join(failures[:4]) if failures else ""
        if mutation.expect_survivor is None:
            if failures:
                print(f"  ok   {mutation.name}  ->  {caught}")
            else:
                print(
                    f"  !!   {mutation.name}: SURVIVED ({reported} suites ran).\n"
                    "       A surviving mutation is a missing test or redundant "
                    "code. Write the test, or record why it cannot be caught\n"
                    "       with `expect_survivor`."
                )
                problems += 1
        elif failures:
            print(
                f"  !!   {mutation.name}: expected to survive and was CAUGHT "
                f"by {caught}.\n"
                f"       The recorded reason has stopped being true: "
                f"{mutation.expect_survivor}"
            )
            problems += 1
        else:
            print(f"  ok   {mutation.name}  ->  survived, as recorded")

    # Restored and re-verified, which is the step `CLAUDE.md` names and which is
    # skipped most often: a mutation run that leaves the tree broken makes every
    # later result a lie.
    failures, reported, _ = run(command)
    if failures or reported == 0:
        print(f"  !! the tree did not come back clean: {failures[:5]}")
        problems += 1
    else:
        print(f"restored: {reported} suites reported, none failing")
    return 1 if problems else 0


def main(argv: list[str]) -> int:
    if argv and argv[0] in {"-h", "--help"}:
        print(__doc__)
        return 0
    text = sys.stdin.read()
    if not text.strip():
        print("expected a JSON spec on stdin; see --help", file=sys.stderr)
        return 2
    return check(json.loads(text))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
