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

import json
import re
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


#: The same stand-in, for a suite whose only failing test is a `should_panic`
#: one. libtest inserts ` - should panic` between the name and the dots, which
#: the `rust` pattern missed until #294 — a caught mutation then reported as
#: unreadable. The exact line was taken from `rustc --test` on a two-test file,
#: not written from memory.
SHOULD_PANIC_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "MUTATED" in text:
    print("test refuses_the_bad_shape - should panic ... FAILED")
    print("test result: FAILED. 0 passed; 1 failed; 0 ignored")
else:
    print("test result: ok. 1 passed; 0 failed; 0 ignored")
'''


#: The same stand-in, for a suite whose only failing test is a doctest.
#: libtest names a doctest by its location — `path.rs - Item (line N)`, with a
#: `- compile` suffix for a `compile_fail` one — so the name is full of spaces
#: and the old `(\S+)` capture could not match any of it. Every doctest in this
#: repository was therefore invisible to the mutation runner. Taken from a real
#: `cargo test -p slate-orm --doc`.
DOCTEST_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "MUTATED" in text:
    print("test crates/slate-orm/src/lib.rs - Record (line 341) - compile fail ... FAILED")
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


#: pytest again, this time with the mutation making a *fixture* raise.
#:
#: pytest calls that an ERROR rather than a FAILED, and summarises it as
#: `1 error in 0.01s` — a line the report pattern matches. The dialect used to
#: match only `FAILED`, so this shape scored as a clean run and the mutation
#: read as a survivor. Met for real: a stale `SLATE_TESTSERVER` made the Python
#: client's harness refuse to start, every test errored in setup, and two
#: genuine mutations were reported as surviving.
PYTEST_ERROR_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "MUTATED" in text:
    print("ERROR tests/t.py::a_named_test")
    print("1 error in 0.01s")
else:
    print("1 passed in 0.01s")
'''


#: The same stand-in, speaking `node --test`'s TAP.
#:
#: The per-file wrapper line is the point, and the reason it is a *third*
#: branch rather than a line beside the failure: node emits it only for a file
#: that threw while loading, and it is then the only thing that file says.
#: Observed on v22.22.2 — the second branch below is a real failing run and has
#: no wrapper in it. The dialect used to skip that line, which would have
#: scored a mutation that broke a test file outright as a survivor.
NODE_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "WILL_NOT_LOAD" in text:
    print("ok 1 - a named case")
    print("not ok 2 - c.test.js")
    print("# pass 1")
    print("# fail 1")
elif "MUTATED" in text:
    print("not ok 1 - a named case")
    print("# pass 0")
    print("# fail 1")
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
#: What `cargo test -q` prints when a test fails: the summary line the
#: `reported` pattern matches, **no** `test NAME ... FAILED` line for the
#: `failed` pattern, and a non-zero exit.
#:
#: #285. `-q` suppresses the per-test results, so a real mutation caught by a
#: real test scored as a survivor — silently, in the direction that reads as
#: "your tests are weak" rather than "this tool is broken". Two were scored
#: that way and two `expect_survivor` records were written against nothing.
QUIET_CARGO_FAKE = """import sys
subject = open(sys.argv[1]).read()
print("running 8 tests")
if "MUTATED" in subject:
    print("test result: FAILED. 7 passed; 1 failed; 0 ignored; 0 measured")
    sys.exit(101)
print("test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured")
"""

GO_FAKE = '''
import sys
text = open(sys.argv[1]).read()
if "WILL_NOT_BUILD" in text:
    print("./x.go:3:2: undefined: nope", file=sys.stderr)
    print("FAIL")
    sys.exit(1)
if "WILL_NOT_BUILD_TAGGED" in text:
    print("# example.com/pkg [example.com/pkg.test]", file=sys.stderr)
    print("./x.go:3:2: undefined: nope", file=sys.stderr)
    print("FAIL\texample.com/pkg [build failed]")
    print("FAIL")
    sys.exit(1)
if "CACHED" in text:
    print("--- PASS: TestANamedCase (0.00s)")
    print("ok  \texample.com/pkg\t(cached)")
    sys.exit(0)
if "MUTATED" in text:
    print("--- FAIL: TestANamedCase (0.00s)")
    print("FAIL")
    print("FAIL\texample.com/pkg\t0.4s")
else:
    print("ok  \texample.com/pkg\t0.4s")
'''


#: Real output from each dialect's real runner, on a clean run and on a run
#: with one failing test.
#:
#: # Why this is here
#:
#: Everything else in this file drives `mutate.py` against a *fake* command,
#: which makes the parsing testable with no toolchain — and means every sample
#: it parses was written by the same hand as the pattern parsing it. Three of
#: `mutate.py`'s six recorded lies were a failure pattern that matched too
#: *little*: `- should panic`, pytest's `ERROR`, `go test -q`. Each scored a
#: caught mutation as a survivor, and each was loud in the end, because a
#: survivor exits non-zero and demands an explanation.
#:
#: The opposite error is silent. A failure pattern that matches a line which is
#: not a failure makes **every** mutation look caught: the run reports a name,
#: exits 0, and the session writes up a test that defends nothing. Nothing in
#: `mutate.py` can detect it — the exact-once rule is about the anchor and the
#: reported-suites count is about the report pattern, and neither reads the
#: failure pattern at all. That was recorded as a caveat in
#: `ledger/2026-09-22-four-dependencies-and-a-tool-that-was-lying.md`, and this
#: closes it.
#:
#: # How the samples were taken
#:
#: Each was captured from the real runner on 2026-09-26, on this container,
#: rather than recalled — the same rule the dialect comments in `mutate.py`
#: already follow, with the output kept instead of paraphrased:
#:
#:   rust    `rustc --test -o t t.rs && ./t`, on a two-test file
#:   python  `python3 scripts/test_check_examples_roster.py`, this repository's
#:           own house style, clean and with one guard arm disabled
#:   pytest  `pytest -q`, on a two-test file
#:   node    `node --test`, on a two-case `.js` file (node 22)
#:   go      `go test -count=1 ./...`, on a one-package module
#:
#: They are trimmed only where a sample would otherwise carry a duration that
#: differs every run; nothing is reworded. A sample that goes stale against a
#: newer runner is a sample that stops matching its own dialect, which the
#: second case below reports rather than passes over.
#:
#: dialect -> (a clean run, a run with one failure, the name that failure has)
CORPUS: dict[str, tuple[str, str, str]] = {
    "rust": (
        "running 2 tests\n"
        "test a_unit_test ... ok\n"
        "test b_unit_test ... ok\n"
        "\n"
        "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; "
        "0 filtered out; finished in 0.00s\n",
        "running 2 tests\n"
        "test b_unit_test ... ok\n"
        "test a_unit_test ... FAILED\n"
        "\n"
        "failures:\n"
        "\n"
        "---- a_unit_test stdout ----\n"
        "\n"
        "thread 'a_unit_test' panicked at f.rs:1:26:\n"
        "assertion failed: false\n"
        "\n"
        "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; "
        "0 filtered out; finished in 0.09s\n",
        "a_unit_test",
    ),
    "python": (
        "ok    a crate whose floor matches, with no servers, is clean\n"
        "ok    a crate with examples and no floor is reported\n"
        "ok    the real tree's four tables agree\n"
        "\n"
        "24 passed, 0 failed\n",
        "ok    a crate whose floor matches, with no servers, is clean\n"
        "FAIL  an example that takes sections and has no refusal line is reported\n"
        "        expected 'has no line in `refuses()`', got []\n"
        "ok    the real tree's four tables agree\n"
        "\n"
        "23 passed, 1 failed\n",
        "an example that takes sections and has no refusal line is reported",
    ),
    "pytest": (
        "..                                                                       "
        "[100%]\n"
        "2 passed in 0.00s\n",
        ".F                                                                       "
        "[100%]\n"
        "=================================== FAILURES ==========================="
        "====\n"
        "____________________________ test_a_failing_case ______________________"
        "______\n"
        "\n"
        "    def test_a_failing_case():\n"
        ">       assert False\n"
        "E       assert False\n"
        "\n"
        "test_f.py:2: AssertionError\n"
        "=========================== short test summary info ===================="
        "========\n"
        "FAILED test_f.py::test_a_failing_case - assert False\n"
        "1 failed, 2 passed in 0.01s\n",
        "test_f.py::test_a_failing_case",
    ),
    "node": (
        "TAP version 13\n"
        "# Subtest: a named case\n"
        "ok 1 - a named case\n"
        "  ---\n"
        "  duration_ms: 0.756938\n"
        "  type: 'test'\n"
        "  ...\n"
        "1..1\n"
        "# tests 1\n"
        "# suites 0\n"
        "# pass 1\n"
        "# fail 0\n"
        "# cancelled 0\n"
        "# skipped 0\n"
        "# todo 0\n",
        "TAP version 13\n"
        "# Subtest: a failing case\n"
        "not ok 1 - a failing case\n"
        "  ---\n"
        "  duration_ms: 1.104\n"
        "  type: 'test'\n"
        "  error: 'no'\n"
        "  ...\n"
        "1..1\n"
        "# tests 1\n"
        "# suites 0\n"
        "# pass 0\n"
        "# fail 1\n",
        "a failing case",
    ),
    "go": (
        "ok  \texample.com/pkg\t0.003s\n",
        "--- FAIL: TestFails (0.00s)\n"
        "    y_test.go:5: no\n"
        "FAIL\n"
        "FAIL\texample.com/pkg\t0.003s\n"
        "FAIL\n",
        "TestFails",
    ),
}


def case_no_dialect_reads_a_clean_run_as_a_failure() -> list[bool]:
    """No failure pattern matches anything in any runner's clean output.

    The whole cross product, not each dialect against its own sample. A
    dialect is chosen by the caller and applied to whatever that command
    prints, so `python` pointed at `go test` is an ordinary mistake — and a
    `python` pattern of `FAIL` alone would match `go test`'s bare trailing
    `FAIL` and report a failure on a clean run. That is not hypothetical: the
    `go` dialect's own report pattern had to be narrowed for the same line.
    """
    sys.path.insert(0, str(ROOT / "scripts"))
    import mutate

    results = []
    for dialect, (failed, _) in sorted(mutate.DIALECTS.items()):
        for other, (clean, _, _) in sorted(CORPUS.items()):
            found = failed.findall(clean)
            ok = not found
            results.append(ok)
            print(
                f"{'ok  ' if ok else 'FAIL'}  the {dialect} failure pattern "
                f"reads no failure in a clean {other} run"
                + ("" if ok else f" (it found {found})")
            )
    return results


def case_every_dialect_reads_its_own_runner() -> list[bool]:
    """And each pattern still matches the output it was written for.

    The other half, and the reason the case above cannot stand alone: a
    failure pattern of `^will-never-match` passes it perfectly. Both
    directions against real output, so a runner whose format moves is reported
    here rather than discovered as a mutation run that scores nothing.
    """
    sys.path.insert(0, str(ROOT / "scripts"))
    import mutate

    results = []
    for dialect, (clean, broken, named) in sorted(CORPUS.items()):
        failed, reported = mutate.DIALECTS[dialect]
        for what, ok in (
            ("names the failing test", failed.findall(broken) == [named]),
            ("sees the clean run report", bool(reported.search(clean))),
            ("sees the failing run report", bool(reported.search(broken))),
        ):
            results.append(ok)
            print(f"{'ok  ' if ok else 'FAIL'}  the {dialect} dialect {what}")
    return results


def case_restore_says_dirty_and_unscorable_apart() -> list[bool]:
    """The restore's three outcomes, which read as two before this.

    `apply` printed `the tree did not come back clean: []` whether the restore
    *failed* or merely could not be scored, and an empty list after that
    sentence asserts a dirty tree while showing no evidence of one. Two of the
    three states mean "I could not tell", and a reader deciding whether to
    trust the results above needs to know which.

    Four cases, because there are four states and each has a wrong answer that
    the other three would not catch: a real failure reported as unscorable
    would hide a dirty tree, and an unscorable run reported as clean would
    hide the tool being broken — which is the fifth lie `unreadable` exists
    for.

    Called rather than driven through a mutation run: reaching the two
    unscorable arms needs a command that scores its baseline and then stops
    reporting, which cannot be arranged on purpose here.
    """
    sys.path.insert(0, str(ROOT / "scripts"))
    import mutate

    expected = [
        ("a named failure is a dirty tree", ["a_test FAILED"], 1, 1, False, "not come back clean"),
        ("nothing reporting cannot be scored", [], 0, 0, None, "could not be scored"),
        ("an unreadable exit cannot be scored", [], 1, 1, None, "could not be scored"),
        ("a clean run says so", [], 1, 0, True, "restored:"),
    ]
    results = []
    for name, failures, reported, status, clean, phrase in expected:
        message, got = mutate.restore_report(failures, reported, status)
        ok = got is clean and phrase in message
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        got {got!r} and {message!r}")
        results.append(ok)
    return results


def case_every_dialect_has_a_sample() -> bool:
    """The never-fires half: a sixth dialect needs a sample, not a pass.

    Without this the two cases above iterate whatever `CORPUS` happens to
    hold, and a dialect added to `mutate.py` and not here is checked by
    nothing — silently, since both loops still print a screenful of `ok`. The
    `EXPECTED_REFUSALS` idiom: a list you are forced to edit is a list that
    stays true.
    """
    sys.path.insert(0, str(ROOT / "scripts"))
    import mutate

    missing = sorted(set(mutate.DIALECTS) - set(CORPUS))
    invented = sorted(set(CORPUS) - set(mutate.DIALECTS))
    ok = not missing and not invented
    print(
        f"{'ok  ' if ok else 'FAIL'}  every dialect has real output to be "
        "checked against"
        + ("" if ok else f" (missing {missing}, invented {invented})")
    )
    return ok


def run(
    spec: str,
    subject: Path,
    fake: Path,
    marker: Path | None = None,
    records: Path | None = None,
) -> tuple[int, str]:
    """`mutate.py` over `spec`, with its command pointed at the fake.

    `marker` gives the child its own in-flight marker. Without one it shares
    the repository's with whatever is driving this suite — and when that is
    `mutate.py` mutating `mutate.py`, the two recover each other.

    `records` does the same for the run record #287 added, and it is not
    optional in practice: without it every case in this file would write a
    real record into `ledger/mutations/`, so running the suite would dirty the
    repository with dozens of fake runs against `/tmp` subjects. A test
    fixture that writes into the tree it is testing is the #281 failure
    pointed the other way.
    """
    import os

    environment = dict(os.environ)
    environment["MUTATE_RECORDS"] = str(records or Path(str(subject) + ".records"))
    if marker is not None:
        environment["MUTATE_MARKER"] = str(marker)
    else:
        # A marker of its own even when a case does not ask, so a suite run
        # inside a mutation run never touches the outer one's.
        environment["MUTATE_MARKER"] = str(subject) + ".in-flight"
    finished = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "mutate.py")],
        input=spec.replace("SUBJECT", str(subject)).replace("FAKE", str(fake)),
        capture_output=True,
        text=True,
        check=False,
        env=environment,
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
        pytest_error_fake = home / "pytest_error_fake.py"
        pytest_error_fake.write_text(PYTEST_ERROR_FAKE)
        node_fake = home / "node_fake.py"
        node_fake.write_text(NODE_FAKE)
        go_fake = home / "go_fake.py"
        go_fake.write_text(GO_FAKE)
        quiet_cargo = home / "quiet_cargo.py"
        quiet_cargo.write_text(QUIET_CARGO_FAKE)
        should_panic = home / "should_panic_fake.py"
        should_panic.write_text(SHOULD_PANIC_FAKE)
        doctest_fake = home / "doctest_fake.py"
        doctest_fake.write_text(DOCTEST_FAKE)
        spec = (
            body.replace("__SUBJECT__", str(subject))
            .replace("__FAKE__", f'"{sys.executable}", "{fake}", "{subject}"')
            .replace("__PYTEST__", f'"{sys.executable}", "{pytest_fake}", "{subject}"')
            .replace(
                "__PYTEST_ERROR__",
                f'"{sys.executable}", "{pytest_error_fake}", "{subject}"',
            )
            .replace("__NODE__", f'"{sys.executable}", "{node_fake}", "{subject}"')
            .replace("__GO__", f'"{sys.executable}", "{go_fake}", "{subject}"')
            .replace(
                "__SHOULD_PANIC__",
                f'"{sys.executable}", "{should_panic}", "{subject}"',
            )
            .replace(
                "__DOCTEST__",
                f'"{sys.executable}", "{doctest_fake}", "{subject}"',
            )
            .replace(
                "__QUIET_CARGO__",
                f'"{sys.executable}", "{quiet_cargo}", "{subject}"',
            )
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


def case_help_lists_every_adapter() -> bool:
    """`--help` names every `scripts/mutate_*.py` beside it, and nothing else.

    Same rule as the dialect list below and for the same reason, one level out.
    `mutate_guard.py` existed, worked, and appeared in nothing this script
    printed — recorded in
    `ledger/2026-09-29-the-dialect-mutate-py-was-missing.md` as the staleness
    `mutate.py`'s docstring argues against, in the one place it could not fix
    itself.

    Both directions again. A help text naming an adapter that is not there
    sends the reader to a file that does not exist, which is the same failure
    walking the other way.
    """
    name = "--help lists exactly the adapters beside it, each with a real dialect"
    spelled = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "mutate.py"), "--help"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    rows = dict(re.findall(r"^    (mutate_\w+\.py)\s+(\S+)", spelled, re.M))
    beside = {path.name for path in (ROOT / "scripts").glob("mutate_*.py")}
    said = []
    if not beside:
        said.append("scripts/ holds no adapters at all; the glob is not matching")
    if set(rows) != beside:
        said.append(f"help lists {sorted(rows)}, scripts/ holds {sorted(beside)}")
    # And the dialect each reports is one the table holds. A declared name is
    # only worth printing if it resolves: `--help` naming a dialect that does
    # not exist sends the next reader somewhere worse than silence did.
    sys.path.insert(0, str(ROOT / "scripts"))
    import mutate

    for adapter, dialect in sorted(rows.items()):
        if dialect not in mutate.DIALECTS:
            said.append(
                f"{adapter} reports dialect {dialect!r}, which is not one of "
                f"{sorted(mutate.DIALECTS)}"
            )
    ok = not said
    print(f"{'ok  ' if ok else 'FAIL'}  {name}")
    for one in said:
        print(f"        {one}")
    return ok


def case_help_lists_every_dialect() -> bool:
    """`--help` names every dialect the table holds, and nothing it does not.

    The hand-written list in the docstring went stale twice — it stopped at
    `pytest` while `node` and `go` were added under it — and the cost was
    somebody reading it, concluding their suite was unsupported, and writing a
    throwaway harness that reimplemented the four protections against a dialect
    that already existed. `--help` now prints the table, so the only way to
    have an undocumented dialect is to have no dialect.

    Asserted in both directions. A help text that printed *more* names than the
    table holds would send the next reader to a dialect that is not there,
    which is the same failure walking the other way.
    """
    name = "--help lists exactly the dialects the table holds"
    spelled = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "mutate.py"), "--help"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    sys.path.insert(0, str(ROOT / "scripts"))
    import mutate

    # `^` is what makes this precise rather than "an indented line with a
    # space in it". The docstring above the list contains an indented JSON
    # example, and the loose version parsed `{"name":` and `"file":` as
    # dialects — so the equality below could never have been asserted, and the
    # first draft quietly checked one direction while claiming two.
    rows = [
        line.strip().split(None, 1)
        for line in spelled.splitlines()
        if re.match(r"^    \w+\s+\^", line)
    ]
    listed = {name for name, _ in rows}
    known = set(mutate.DIALECTS)
    # The pattern each row shows must be the one that dialect will actually
    # look for, not merely something non-empty. "Non-empty" was the first
    # version and a mutation printing a bare `^` for every dialect walked
    # through it — a help text that lists five names and five wrong patterns
    # is worse than one that lists nothing, because it reads as specific.
    ok = listed == known and all(
        pattern == mutate.DIALECTS[name][1].pattern for name, pattern in rows
    )
    print(
        f"{'ok  ' if ok else 'FAIL'}  {name}"
        + (
            ""
            if ok
            else f" (missing {sorted(known - listed)}, invented {sorted(listed - known)})"
        )
    )
    return ok


def case_records_every_run() -> list[bool]:
    """A run leaves a record, and so does a run that cannot score.

    The second half is the whole point of #287. When the `-q` defect was found
    in #285, the question "which earlier runs did this spoil?" had no answer,
    because a run that scores nothing left nothing behind — not even the
    command that made it unscoreable. A record written only on success would
    have reproduced that exactly.
    """
    results = []
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        subject = root / "subject.py"
        subject.write_text("ORIGINAL\n")
        fake = root / "fake.py"
        fake.write_text(FAKE)
        records = root / "records"

        code, _ = run(
            # `FAKE` speaks the rust dialect; the dialect is not what this
            # case is about, only that the run leaves a record behind.
            ('{"file": "__SUBJECT__", "command": [__FAKE__], "dialect": "rust",'
             ' "cases": [{"name": "a named case", "old": "ORIGINAL",'
             '             "new": "MUTATED"}]}')
            .replace("__SUBJECT__", str(subject))
            .replace("__FAKE__", f'"{sys.executable}", "{fake}", "{subject}"'),
            subject, fake, records=records,
        )
        written = sorted(records.glob("*.json")) if records.is_dir() else []
        entry = json.loads(written[0].read_text()) if written else {}
        # `bool(...)`, not the bare chain: `a and b` yields the last operand,
        # and `caught_by` is a list, so `ok` came out a list and the summary
        # died on `sum()`. The suite caught it; the lesson is that a truthy
        # chain is not a predicate.
        ok = bool(
            code == 0
            and len(written) == 1
            and entry.get("outcome") == "clean"
            and entry.get("command")
            and [one["verdict"] for one in entry.get("cases", [])] == ["caught"]
            and entry["cases"][0]["name"] == "a named case"
            and entry["cases"][0]["caught_by"]
        )
        results.append(ok)
        print(f"{'ok  ' if ok else 'FAIL'}  a run records itself, with its command and verdicts")
        if not ok:
            print(f"        exit {code}, wrote {len(written)}: {entry}")

    # And the unscoreable one: a command whose output the dialect cannot read.
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        subject = root / "subject.py"
        subject.write_text("ORIGINAL\n")
        fake = root / "quiet.py"
        # The `-q` fake #285 already keeps here: what `cargo test -q` prints
        # on a failure, a suite result with no test named. Reused rather than
        # rewritten, so if that output is ever corrected both cases move.
        fake.write_text(QUIET_CARGO_FAKE)
        # A red baseline, so the run is unreadable from its first command.
        subject.write_text("MUTATED\n")
        records = root / "records"
        code, _ = run(
            ('{"file": "__SUBJECT__", "command": [__FAKE__], "dialect": "rust",'
             ' "cases": [{"name": "never reached", "old": "MUTATED",'
             '             "new": "OTHER"}]}')
            .replace("__SUBJECT__", str(subject))
            .replace("__FAKE__", f'"{sys.executable}", "{fake}", "{subject}"'),
            subject, fake, records=records,
        )
        written = sorted(records.glob("*.json")) if records.is_dir() else []
        entry = json.loads(written[0].read_text()) if written else {}
        ok = bool(
            code == 1
            and len(written) == 1
            and entry.get("outcome") == "baseline-unreadable"
            and entry.get("command")
            and entry.get("cases") == []
        )
        results.append(ok)
        print(f"{'ok  ' if ok else 'FAIL'}  a run that cannot score is recorded as such")
        if not ok:
            print(f"        exit {code}, wrote {len(written)}: {entry}")
    return results


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


def case_recovers_from_a_kill() -> list[bool]:
    """A run killed mid-mutation is recovered by the next one, or refused.

    `mutate.py` restores in a `finally`, which does not run through a
    `SIGKILL` — and a mutation run is exactly the kind of thing a timeout
    kills. That happened: a killed run left `scripts/run_examples.sh` carrying
    a mutated roster line, and the next test run failed against it in a way
    that read as a broken test rather than a dirty tree.

    Rather than kill a real run, which would be slow and racy, these write the
    marker by hand in each of the three states it can be found in. That is what
    a killed run leaves behind, and it is the input `recover` actually takes.
    """
    import json

    results = []

    def attempt(
        name: str,
        contents: str,
        expect_code: int,
        expect_text: str,
        expect_after: str | None,
        vanished: bool = False,
    ) -> bool:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            subject = home / "subject.txt"
            subject.write_text(contents)
            fake = home / "fake.py"
            fake.write_text(FAKE)
            # A marker of this case's own, via `MUTATE_MARKER`. The repository
            # one is shared with the `mutate.py` run that may be driving this
            # suite, and the two would recover each other's state.
            marker = home / "marker.json"
            marker.write_text(
                json.dumps(
                    {
                        "file": str(home / "gone.txt") if vanished else str(subject),
                        "case": "a killed case",
                        "original": "ORIGINAL\n",
                        "mutated": "MUTATED\n",
                    }
                )
            )
            try:
                body = spec('{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}')
                text = body.replace("__SUBJECT__", str(subject)).replace(
                    "__FAKE__",
                    f'"{sys.executable}", "{fake}", "{subject}"',
                )
                code, output = run(text, subject, fake, marker)
                problems = []
                if code != expect_code:
                    problems.append(f"exit {code}, expected {expect_code}")
                if expect_text not in output:
                    problems.append(f"missing {expect_text!r}")
                if expect_after is not None and subject.read_text() != expect_after:
                    problems.append(
                        f"the file holds {subject.read_text()!r}, "
                        f"expected {expect_after!r}"
                    )
                if expect_code == 0 and marker.exists():
                    problems.append("the marker was left behind after a clean run")
            finally:
                marker.unlink(missing_ok=True)
        if problems:
            print(f"FAIL  {name}")
            for problem in problems:
                print(f"        {problem}")
            for line in output.splitlines()[:8]:
                print(f"      {line}")
            return False
        print(f"ok    {name}")
        return True

    results.append(attempt(
        "a file left mutated by a killed run is put back",
        # Exactly what the marker says was written: unambiguously our leftover.
        "MUTATED\n",
        0,
        "recovered",
        # Restored, then mutated and restored again by the run that follows.
        "ORIGINAL\n",
    ))
    results.append(attempt(
        "a marker whose file is already back is just dropped",
        "ORIGINAL\n",
        0,
        "already back",
        "ORIGINAL\n",
    ))
    results.append(attempt(
        "a marker naming a file that is gone is dropped, not acted on",
        # A fixture in a temporary directory that has since been removed,
        # which is the commonest leftover of all: there is nothing to restore
        # and nothing to warn about, and refusing here would make every later
        # run fail on a file nobody can put back.
        "ORIGINAL\n",
        0,
        "ok   m",
        "ORIGINAL\n",
        vanished=True,
    ))
    results.append(attempt(
        "a file edited since the kill is refused rather than guessed at",
        # Neither version. Overwriting would trade one silent wrong state for
        # another, so the run refuses and says which file and which case.
        "SOMEBODY ELSE'S EDIT\n",
        1,
        "Refusing to guess",
        "SOMEBODY ELSE'S EDIT\n",
    ))
    return results


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
            "a failing should_panic test is read as a catch, not as unreadable",
            '{"file": "__SUBJECT__", "dialect": "rust", "command": [__SHOULD_PANIC__], '
            '"cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["ok   m", "refuses_the_bad_shape"],
            # The bug this is for: ` - should panic` between the name and the
            # dots meant the pattern missed the line, so a *caught* mutation
            # reported UNREADABLE — which reads as "your command is wrong" and
            # sends the next person to fix a command that was already right.
            # The reported name must also come back bare: a capture that
            # swallowed the suffix would name a test nobody can run. Rejecting
            # `should panic ...` was the first attempt and it let that through
            # — the swallowed form reports as `NAME - should panic`, with no
            # dots, so the string being rejected never appeared either way. A
            # reject that cannot fire is the absence-checking version of a
            # guard that never runs.
            ["UNREADABLE", "- should panic"],
        ),
        case(
            "a failing doctest is read as a catch, and named by its location",
            '{"file": "__SUBJECT__", "dialect": "rust", "command": [__DOCTEST__], '
            '"cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["ok   m", "lib.rs - Record (line 341) - compile fail"],
            # Doctests were wholly invisible before #294: every name libtest
            # gives one contains spaces, so the capture matched nothing and a
            # mutation a `compile_fail` doctest really caught came back
            # UNREADABLE. `- compile fail` stays in the reported name because
            # without it the line number is all a reader has. That exact
            # suffix is what a failing `compile_fail` doctest prints — copied
            # from the run that proved the cross-tenant `has_many` refusal is
            # defended, not invented for the fixture.
            ["UNREADABLE"],
        ),
        case(
            "a command whose failures cannot be read is refused, not scored",
            '{"file": "__SUBJECT__", "dialect": "rust", "command": [__QUIET_CARGO__], '
            '"cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            1,
            ["UNREADABLE", "cannot score anything", "drop `-q`"],
            # The whole point: it must never call this a survivor.
            ["survived"],
        ),
        case(
            "and a clean run through the same command still scores normally",
            '{"file": "__SUBJECT__", "dialect": "rust", "command": [__QUIET_CARGO__], '
            '"cases": [{"name": "m", "old": "ORIGINAL", "new": "UNTOUCHED", '
            '"expect_survivor": true, "why": "the fake only fails on MUTATED"}]}',
            0,
            ["survived, as recorded"],
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
            # The message must offer the equivalent-mutation cause *first*.
            # #296: three mutations in one session were `&x.clone()` or
            # `if true { x } else { x }` — no change at all — and each
            # survived meaninglessly while the message insisted a survivor is
            # a missing test or redundant code. One was nearly written up as an
            # untested cross-tenant guard. The script checks that `old` occurs
            # once, never that `new` behaves differently, so it cannot detect
            # this; the least it can do is name it.
            [
                "SURVIVED",
                "the mutation may not be a change",
                "missing test or redundant code",
            ],
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
            "a pytest ERROR counts as a catch, not as a clean run",
            # A fixture that raises is an ERROR, not a FAILED, and pytest's
            # summary line for it (`1 error in 0.01s`) matches the report
            # pattern — so before this the run looked clean and the mutation
            # looked like a survivor. Two real mutations were mis-scored that
            # way before the dialect learned the word.
            '{"file": "__SUBJECT__", "command": [__PYTEST_ERROR__], "dialect": "pytest",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["ok   m", "tests/t.py::a_named_test"],
            ["SURVIVED"],
        ),
        case(
            "a node --test failure is read through the node dialect",
            '{"file": "__SUBJECT__", "command": [__NODE__], "dialect": "node",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "MUTATED"}]}',
            0,
            ["ok   m", "a named case"],
        ),
        case(
            "a test file that will not load is counted, not skipped",
            # This case replaces one asserting the opposite. node emits the
            # per-file line only for a file that threw while loading, and it is
            # then the only thing that file says — so skipping it leaves a run
            # where `# fail 1` matches the report pattern, no failure name is
            # found, and a mutation that broke a test file outright scores as a
            # survivor. Same hole as the pytest `ERROR` one, written on purpose.
            '{"file": "__SUBJECT__", "command": [__NODE__], "dialect": "node",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "WILL_NOT_LOAD"}]}',
            0,
            ["ok   m", "c.test.js"],
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
            "a cached go result is not a suite that reported",
            # `go test` replays a cached package result when its *Go* inputs
            # have not changed. `clients/go` exercises a Rust server over a
            # socket, so a mutation in `crates/` changes nothing the cache
            # hashes: the replay prints the old `--- PASS` lines and
            # `ok <pkg> (cached)`, and a caught mutation scores as a survivor.
            # Met for real on a chain's grouped sort — `-count=1` on the very
            # same command failed two named tests. NOTHING RAN is the honest
            # answer, because nothing did.
            '{"file": "__SUBJECT__", "command": [__GO__], "dialect": "go",'
            ' "cases": [{"name": "m", "old": "ORIGINAL", "new": "CACHED"}]}',
            1,
            ["NOTHING RAN"],
            ["SURVIVED"],
        ),
        case(
            "a go build failure with a package name is not a suite that reported",
            # The shape the case below did not cover, and the one `go test`
            # actually prints: `FAIL\texample.com/pkg [build failed]` — a
            # per-package line, with a package name, matching a marker written
            # to demand exactly that. So a mutation that did not compile scored
            # as a clean run and read as a survivor. Met for real, on a Go
            # mutation that used `strings.Split` in a file that does not import
            # `strings`.
            '{"file": "__SUBJECT__", "command": [__GO__], "dialect": "go",'
            ' "cases": [{"name": "m", "old": "ORIGINAL",'
            '             "new": "WILL_NOT_BUILD_TAGGED"}]}',
            1,
            ["NOTHING RAN"],
            ["SURVIVED"],
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
        *case_records_every_run(),
        case_fresh_bytecode(),
        *case_recovers_from_a_kill(),
        case_help_lists_every_dialect(),
        case_help_lists_every_adapter(),
        *case_no_dialect_reads_a_clean_run_as_a_failure(),
        *case_every_dialect_reads_its_own_runner(),
        case_every_dialect_has_a_sample(),
        *case_restore_says_dirty_and_unscorable_apart(),
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
