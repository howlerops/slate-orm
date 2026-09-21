#!/usr/bin/env python3
"""`scripts/mutate.py`, tested against its own three failure modes.

The script exists because a hand-run mutation cannot tell "the code is right"
from "the patch never applied" from "nothing built". Those are the cases here,
and they are run against a *fake* command rather than `cargo`, so this needs no
toolchain, no build and no seconds — which is what lets it sit in the static
check run beside the linters.

The fake is the interesting part. It is a tiny Python program that reads the
file under mutation and decides what to print: a passing `test result:` line, a
`FAILED` line, or a compiler-style error and nothing else. That makes the
script's *parsing* the thing under test, which is where all three failure modes
live.

Run directly: `python3 scripts/test_mutate.py`.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: A stand-in for `cargo test`. Prints what a real run prints, chosen by what
#: it finds in the file — so a mutation that lands changes the output exactly
#: as a real suite would, and one that does not land leaves it alone.
FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "WILL_NOT_BUILD" in text:
    print("error[E0599]: no method named `nope`", file=sys.stderr)
    sys.exit(101)
if "MUTATED" in text:
    print("test a_named_test ... FAILED")
    print("test result: FAILED. 0 passed; 1 failed; 0 ignored")
else:
    print("test result: ok. 1 passed; 0 failed; 0 ignored")
'''


#: The same stand-in, speaking pytest. `-q` prints `FAILED path::name` in the
#: short summary and a closing `N passed in Xs` — neither of which the other
#: two dialects match, which is the whole reason this one exists.
PYTEST_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "MUTATED" in text:
    print("FAILED tests/t.py::a_named_test - AssertionError")
    print("1 failed, 0 passed in 0.01s")
else:
    print("1 passed in 0.01s")
'''


#: The same stand-in, speaking `node --test`'s TAP.
#:
#: The wrapper line is the point: node reports the *file* as a failing test
#: alongside the real case, so a dialect that counted every `not ok` would name
#: `test/t.test.ts` as a test nobody wrote. The fake emits both.
NODE_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "MUTATED" in text:
    print("not ok 1 - a named case")
    print("not ok 2 - test/t.test.ts")
    print("# pass 0")
    print("# fail 2")
else:
    print("ok 1 - a named case")
    print("# pass 1")
    print("# fail 0")
'''


#: The same stand-in, speaking `go test`.
#:
#: The bare trailing `FAIL` is the point: `go test` prints it as its last line
#: whether a test failed or the package would not build, so a dialect that took
#: it as proof a suite reported would read a build error as a clean run.
GO_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "WILL_NOT_BUILD" in text:
    print("./x.go:3:2: undefined: nope", file=sys.stderr)
    print("FAIL")
    sys.exit(1)
if "MUTATED" in text:
    print("--- FAIL: TestANamedCase (0.00s)")
    print("FAIL")
    print("FAIL\texample.com/pkg\t0.4s")
else:
    print("ok  \texample.com/pkg\t0.4s")
'''


def run(spec: str, subject: Path, fake: Path) -> tuple[int, str]:
    """`mutate.py` over `spec`, with its command pointed at the fake."""
    finished = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "mutate.py")],
        input=spec.replace("SUBJECT", str(subject)).replace("FAKE", str(fake)),
        capture_output=True,
        text=True,
        check=False,
    )
    return finished.returncode, finished.stdout + finished.stderr


def case(
    name: str,
    body: str,
    expect_code: int,
    expect_text: list[str],
    reject_text: list[str] | None = None,
) -> bool:
    """One case. `reject_text` is what must *not* appear.

    Added for the node dialect, whose property is an absence: node reports the
    test file itself as a failing test beside the real case, and the thing
    worth asserting is that the file's name never turns up as the test that
    caught a mutation. A presence check cannot say that.
    """
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        subject = home / "subject.txt"
        subject.write_text("ORIGINAL\n")
        fake = home / "fake.py"
        fake.write_text(FAKE)
        pytest_fake = home / "pytest_fake.py"
        pytest_fake.write_text(PYTEST_FAKE)
        node_fake = home / "node_fake.py"
        node_fake.write_text(NODE_FAKE)
        go_fake = home / "go_fake.py"
        go_fake.write_text(GO_FAKE)
        spec = (
            body.replace("__SUBJECT__", str(subject))
            .replace("__FAKE__", f'"{sys.executable}", "{fake}", "{subject}"')
            .replace("__PYTEST__", f'"{sys.executable}", "{pytest_fake}", "{subject}"')
            .replace("__NODE__", f'"{sys.executable}", "{node_fake}", "{subject}"')
            .replace("__GO__", f'"{sys.executable}", "{go_fake}", "{subject}"')
        )
        code, output = run(spec, subject, fake)
        problems = []
        if code != expect_code:
            problems.append(f"exit {code}, expected {expect_code}")
        for wanted in expect_text:
            if wanted not in output:
                problems.append(f"missing {wanted!r}")
        for unwanted in reject_text or []:
            if unwanted in output:
                problems.append(f"present and should not be: {unwanted!r}")
        # The guarantee that matters most and is easiest to lose: whatever
        # happened, the file is as it was. A harness that leaves a mutated tree
        # makes every later run a lie.
        if subject.read_text() != "ORIGINAL\n":
            problems.append(f"the subject was left mutated: {subject.read_text()!r}")
        if problems:
            print(f"FAIL  {name}")
            for problem in problems:
                print(f"        {problem}")
            print("      ---- output ----")
            for line in output.splitlines():
                print(f"      {line}")
            return False
    print(f"ok    {name}")
    return True


#: A subject that is *imported* rather than read, which is what makes the
#: bytecode cache reachable at all. The fake command above opens its subject as
#: text and is immune, which is why this case needs a runner of its own.
IMPORTED_SUBJECT = "VALUE = 1\n"

RUNNER = '''
import os, pathlib, sys
home = pathlib.Path(sys.argv[1])
# One line per run, so the test can assert every run got its own cache. This
# is the deterministic half: whether two writes land in the same mtime second
# is a race, but "each run is given a fresh cache directory" is not.
with (home / "caches.log").open("a") as log:
    print(os.environ.get("PYTHONPYCACHEPREFIX", "unset"), file=log)
sys.path.insert(0, str(home))
import subject
if subject.VALUE == 2:
    print("test saw_two ... FAILED")
    print("test result: FAILED. 0 passed; 1 failed; 0 ignored")
elif subject.VALUE == 3:
    print("test saw_three ... FAILED")
    print("test result: FAILED. 0 passed; 1 failed; 0 ignored")
else:
    print("test result: ok. 1 passed; 0 failed; 0 ignored")
'''


def case_fresh_bytecode() -> bool:
    """Two same-size mutations of an imported module are scored separately.

    CPython invalidates a `.pyc` on mtime and size. `VALUE = 1` and `VALUE = 2`
    are the same size, and two mutation runs land in the same second, so
    without a fresh cache per run the second is scored against the first one's
    bytecode — and the restore afterwards is invisible too. That is exactly
    what happened the first time `mutate.py` was pointed at
    `check_cited_tests.py`, and it is the reason this case exists.
    """
    name = "two same-size mutations are not scored against each other's bytecode"
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        subject = home / "subject.py"
        subject.write_text(IMPORTED_SUBJECT)
        runner = home / "runner.py"
        runner.write_text(RUNNER)
        body = spec(
            '{"name": "two", "old": "VALUE = 1", "new": "VALUE = 2"},'
            '{"name": "three", "old": "VALUE = 1", "new": "VALUE = 3"}'
        )
        text = body.replace("__SUBJECT__", str(subject)).replace(
            "__FAKE__", f'"{sys.executable}", "{runner}", "{home}"'
        )
        code, output = run(text, subject, runner)

        problems = []
        if code != 0:
            problems.append(f"exit {code}, expected 0")
        # The symptom: without the fix the second mutation reports the first
        # one's test name, so `saw_three` never appears.
        for wanted in ("saw_two", "saw_three"):
            if wanted not in output:
                problems.append(f"missing {wanted!r} — the second mutation was "
                                "scored against the first one's bytecode")
        caches = (home / "caches.log").read_text().split()
        if len(caches) != len(set(caches)):
            problems.append(f"a cache directory was reused across runs: {caches}")
        if any(cache == "unset" for cache in caches):
            problems.append("a run was given no cache prefix at all")
        if subject.read_text() != IMPORTED_SUBJECT:
            problems.append(f"the subject was left mutated: {subject.read_text()!r}")

        if problems:
            print(f"FAIL  {name}")
            for problem in problems:
                print(f"        {problem}")
            print("      ---- output ----")
            for line in output.splitlines():
                print(f"      {line}")
            return False
    print(f"ok    {name}")
    return True


def spec(cases: str) -> str:
    return '{"file": "__SUBJECT__", "command": [__FAKE__], "cases": [' + cases + "]}"


def main() -> int:
    # `file` is absolute in these specs; `mutate.py` joins it against ROOT,
    # which leaves an absolute path unchanged.
    passed = [
        case(
            "a mutation the suite catches is reported with the test's name",
            spec('{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}'),
            0,
            ["ok   m", "a_named_test"],
        ),
        case(
            "an anchor that matches nothing is refused before anything runs",
            spec('{"name": "m", "old": "NOT_PRESENT", "new": "x"}'),
            1,
            ["occurs 0 times", "would pass against unmutated code"],
        ),
        case(
            "an anchor that matches twice is refused too",
            '{"file": "__SUBJECT__", "command": [__FAKE__], "cases": ['
            '{"name": "m", "old": "I", "new": "1"}]}',
            1,
            ["occurs 2 times"],
        ),
        case(
            "a mutation that does not build is not mistaken for a survivor",
            spec('{"name": "m", "old": "ORIGINAL", "new": "WILL_NOT_BUILD"}'),
            1,
            ["NOTHING RAN", "E0599"],
        ),
        case(
            "a surviving mutation fails loudly rather than scrolling past",
            spec('{"name": "m", "old": "ORIGINAL", "new": "ORIGINAL_BUT_HARMLESS"}'),
            1,
            ["SURVIVED", "missing test or redundant code"],
        ),
        case(
            "a survivor with a recorded reason is accepted",
            spec(
                '{"name": "m", "old": "ORIGINAL", "new": "ORIGINAL_BUT_HARMLESS",'
                ' "expect_survivor": "the branch is redundant"}'
            ),
            0,
            ["survived, as recorded"],
        ),
        case(
            "a recorded survivor that is now caught fails, because the reason has expired",
            spec(
                '{"name": "m", "old": "ORIGINAL", "new": "MUTATED",'
                ' "expect_survivor": "stale"}'
            ),
            1,
            ["was CAUGHT", "stopped being true"],
        ),
        case(
            "a pytest failure is read through the pytest dialect",
            # Three dialects and one of them silently misreading the others'
            # output is the failure this script exists to stop, so each is
            # exercised against output shaped like the real thing.
            '{"file": "__SUBJECT__", "command": [__PYTEST__], "dialect": "pytest",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["ok   m", "tests/t.py::a_named_test"],
        ),
        case(
            "a pytest run that reports nothing is not a clean pass",
            # The case that produced this dialect: pytest's `4 passed in 0.01s`
            # matches neither of the other two, so reading it under the wrong
            # one reports zero suites — which is right, and reading it as clean
            # would not be.
            '{"file": "__SUBJECT__", "command": [__PYTEST__], "dialect": "python",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            1,
            ["reported no test results at all"],
        ),
        case(
            "a node --test failure is read through the node dialect",
            '{"file": "__SUBJECT__", "command": [__NODE__], "dialect": "node",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["ok   m", "a named case"],
        ),
        case(
            "the node dialect does not count the per-file wrapper as a test",
            # node reports the file itself as `not ok N - test/t.test.ts`
            # beside the real case. Counting it would attribute the catch to a
            # test nobody wrote, which reads like coverage that is not there.
            '{"file": "__SUBJECT__", "command": [__NODE__], "dialect": "node",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["a named case"],
            reject_text=["t.test.ts"],
        ),
        case(
            "node output read as libtest reports nothing rather than a pass",
            # The `rust` dialect matches neither node's failures nor its
            # summary, so pointing it at node output must report zero suites.
            # Reading it as clean would score every mutation a survivor.
            '{"file": "__SUBJECT__", "command": [__NODE__], "dialect": "rust",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            1,
            ["reported no test results at all"],
        ),
        case(
            "a go test failure is read through the go dialect",
            '{"file": "__SUBJECT__", "command": [__GO__], "dialect": "go",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["ok   m", "TestANamedCase"],
        ),
        case(
            "a go build error is not read as a suite that reported",
            # `go test` prints a bare `FAIL` for a package that would not
            # build, with no per-package line and no test names. Counting that
            # bare line as a report is the difference between "the mutation was
            # not caught" and "nothing ran", which the docstring calls failure
            # mode 2 -- and the two look identical from the outside.
            #
            # This case earned its keep before it was ever committed: the
            # marker was first written `^(?:ok|FAIL)\\s+\\S+`, and `\\s` matches
            # the newline, so a bare `FAIL` followed by a compiler error on the
            # next line parsed as a package verdict and the build failure was
            # scored a survivor.
            '{"file": "__SUBJECT__", "command": [__GO__], "dialect": "go",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "WILL_NOT_BUILD"}]}',
            1,
            ["NOTHING RAN"],
        ),
        case_fresh_bytecode(),
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
