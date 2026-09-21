#!/usr/bin/env python3
"""Run a mutation, and refuse to be ambiguous about what happened.

`CLAUDE.md` asks for a mutation test on everything written here: break each
thing, confirm a *named* test fails, restore, re-verify. The discipline is
sound and doing it by hand has a failure mode that looks exactly like success.

**Four ways a mutation run lies. The first three were met by hand in one
session; the fourth was found by pointing this script at itself:**

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
4. **A stale cache runs the wrong code.** CPython keys a `.pyc` on the
   source's mtime and size, and a mutation is usually the same size as what it
   replaces — so a same-second edit can be scored against the *previous*
   mutation's bytecode, and a restore can be invisible. Found by running this
   script against itself; see `run()`.

Each is caught here rather than trusted to a reader's attention:

- the old text must occur **exactly once**, and the count is reported when not;
- the file is restored in a `finally`, so an interrupt does not leave a mutated
  tree;
- the command's output is parsed for how many suites *reported*, and zero is a
  hard error that says so rather than a quiet pass;
- the spec arrives as JSON on stdin, so no replacement ever touches a shell;
- every run compiles into a fresh bytecode cache, so a same-size mutation
  cannot be scored against the previous one.

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

`"dialect"` says whose output to read. It defaults to `"rust"`, and `--help`
lists every one this script knows — **generated from the table rather than
written out**, because the hand-written list went stale the moment a fourth
dialect was added and stayed stale through a fifth. The cost of that is not
hypothetical: a session read this docstring, concluded `node` was unsupported,
and wrote a throwaway harness reimplementing the four protections above against
a dialect that had been here for weeks. A list of what a tool supports is the
one thing a tool should never be asked to keep in sync by hand.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: How to read a suite's output: which lines name a failure, and which line
#: proves a suite reported at all.
#:
#: Two dialects rather than one because this repository has two kinds of suite
#: and the first version of this script only knew about Rust — which meant the
#: Python guards, the ones whose whole job is to catch a mistake nobody
#: remembers to look for, were the ones that could not be mutation-tested. A
#: third dialect is a two-line entry here; the point is that an unrecognised
#: one reports *zero suites* and is refused, rather than reading as a clean
#: pass, which is failure mode 2 in the docstring above.
DIALECTS = {
    # libtest: `test some_name ... FAILED`, and `test result: ok. 12 passed`
    # once per test binary that actually ran.
    "rust": (
        re.compile(r"^test (\S+) \.\.\. FAILED\s*$", re.MULTILINE),
        re.compile(r"^test result:", re.MULTILINE),
    ),
    # The house style of `scripts/test_*.py`: `ok    name` / `FAIL  name`, and
    # a closing `N passed, M failed`.
    "python": (
        re.compile(r"^FAIL\s+(.+?)\s*$", re.MULTILINE),
        re.compile(r"^\d+ passed, \d+ failed\s*$", re.MULTILINE),
    ),
    # pytest, which `clients/python` uses and the house style does not match:
    # `FAILED path::name - reason` in the short summary, and a closing
    # `4 passed in 0.01s` or `1 failed, 3 passed in 0.02s`.
    #
    # Added because pointing this script at the Python client produced
    # `the command reported no test results at all` — which is the second
    # failure mode in the docstring above, met while using the tool written
    # for it. The alternative was reading `4 passed` as a clean run under the
    # `python` dialect, which would have scored a real mutation as a survivor.
    # `ERROR` and not only `FAILED`, because a fixture that raises is reported
    # as `ERROR path::name` and summarised as `7 errors in 0.11s` — which the
    # report pattern matches and the failure pattern did not, so a run where
    # *every* test errored in setup scored as a clean pass and the mutation
    # read as a survivor. Met on the first mutation run against the Python
    # client's full-text tests, where a stale `SLATE_TESTSERVER` made the
    # harness refuse to start. A survivor is supposed to mean "write a test",
    # so this lie costs a test that should never have been written — the first
    # failure mode in the docstring above, wearing a different hat.
    "pytest": (
        re.compile(r"^(?:FAILED|ERROR) (\S+)", re.MULTILINE),
        re.compile(r"^\d+ (?:passed|failed|error)", re.MULTILINE),
    ),
    # `node --test`'s TAP output, which `clients/typescript` uses:
    # `not ok 3 - the name` per failure, and a closing `# pass N` / `# fail N`.
    #
    # The failure pattern skips the per-file wrapper line, which node emits as
    # `not ok 1 - test/foo.test.ts` alongside the real case — a name ending in
    # `.ts` is the file, not a test, and counting it would report a failure
    # nobody wrote. The `# fail` line is the report marker rather than `# pass`
    # because a run where everything fails still prints it.
    "node": (
        re.compile(r"^not ok \d+ - (?!.*\.ts$)(.+?)\s*$", re.MULTILINE),
        re.compile(r"^# fail \d+\s*$", re.MULTILINE),
    ),
    # `go test`, which `clients/go` uses: `--- FAIL: TestName (0.00s)` per
    # failure — indented for a subtest, hence the leading `\s*` — and one
    # `ok   <package>  0.5s` or `FAIL <package>  0.5s` line per package.
    #
    # The report marker demands a package name after the verdict *on the same
    # line*, because `go test` also prints a bare `FAIL` as its last word on a
    # failing run. Matching that alone would read a build error — which prints
    # `FAIL` and no per-package line — as a suite that reported, which is
    # failure mode 2 in the docstring above wearing a green hat.
    #
    # `[ \t]+` and not `\s+`, which is not pedantry: `\s` matches the newline,
    # so `^(?:ok|FAIL)\s+\S+` reads a bare `FAIL` plus whatever the compiler
    # printed on the next line as a package verdict. Written that way first,
    # and the case below caught it.
    #
    # And the `[build failed]` exclusion, which the paragraph above was wrong
    # about: `go test` on a package that does not compile prints
    # `FAIL\tgithub.com/x/y [build failed]` — a per-package line with a
    # package name on it, matching the marker exactly. So a mutation that did
    # not compile scored as a clean run and read as a survivor, which is the
    # same lie the pytest `ERROR` hole told, met in the same session. `[setup
    # failed]` is the other shape `go test` prints in that position.
    "go": (
        re.compile(r"^\s*--- FAIL: (\S+)", re.MULTILINE),
        re.compile(
            r"^(?:ok|FAIL)[ \t]+\S+(?![^\n]*\[(?:build|setup) failed\])", re.MULTILINE
        ),
    ),
}


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


def run(command: list[str], dialect: str) -> tuple[list[str], int, str]:
    """The command, its failing test names, and how many suites reported.

    Every run gets a **fresh bytecode cache**, which is not housekeeping — it
    is the fourth way a mutation run lies, and the only one found by pointing
    this script at itself rather than by being bitten in a session.

    CPython invalidates a `.pyc` on the source's *mtime and size*, and a
    mutation worth making is usually the same size as what it replaces:
    `{4,}` for `{3,}`, `not any(` for `not all(`, `return 1` for `return 0`.
    All three of those were run against `check_cited_tests.py` inside one
    second, and the second mutation was scored against the first one's cached
    bytecode — it reported a *different test* as the one that caught it, which
    is the only reason this was noticed at all. Worse, the restore afterwards
    wrote the original bytes and the final verification still ran the mutated
    code, so the script announced the tree had not come back clean when it
    had.

    A same-second, same-size edit is exactly what this tool does, so the
    ordinary assumption behind the cache does not hold here. `cargo` hashes
    contents and is immune, but the prefix is set for every dialect: a run
    that is slower by one recompile is cheaper than a result nobody can trust.
    """
    failed, reported = DIALECTS[dialect]
    with tempfile.TemporaryDirectory(prefix="mutate-pyc-") as cache:
        environment = dict(os.environ, PYTHONPYCACHEPREFIX=cache)
        finished = subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )
    output = finished.stdout + finished.stderr
    return failed.findall(output), len(reported.findall(output)), output


def check(spec: dict) -> int:
    path = ROOT / spec["file"]
    command = spec["command"]
    # Defaulted rather than required: every spec written before dialects
    # existed drives a Rust suite, and silently changing what those mean would
    # be the same class of lie this script exists to stop.
    dialect = spec.get("dialect", "rust")
    if dialect not in DIALECTS:
        raise SystemExit(
            f"unknown dialect {dialect!r}; known: {sorted(DIALECTS)}"
        )
    cases = [Mutation(case) for case in spec["cases"]]

    # A baseline, because a mutation run says nothing if the suite was already
    # red. This is the check a hand-run mutation always skips and the one that
    # makes every result below mean something.
    failures, reported, output = run(command, dialect)
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
            failures, reported, output = run(command, dialect)
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
    failures, reported, _ = run(command, dialect)
    if failures or reported == 0:
        print(f"  !! the tree did not come back clean: {failures[:5]}")
        problems += 1
    else:
        print(f"restored: {reported} suites reported, none failing")
    return 1 if problems else 0


def main(argv: list[str]) -> int:
    if argv and argv[0] in {"-h", "--help"}:
        print(__doc__)
        print("Dialects, with the line each reads as proof a suite ran:\n")
        for name, (_, reported) in sorted(DIALECTS.items()):
            print(f"    {name:<12} {reported.pattern}")
        print()
        return 0
    text = sys.stdin.read()
    if not text.strip():
        print("expected a JSON spec on stdin; see --help", file=sys.stderr)
        return 2
    return check(json.loads(text))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
