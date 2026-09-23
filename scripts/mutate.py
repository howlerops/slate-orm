#!/usr/bin/env python3
"""Run a mutation, and refuse to be ambiguous about what happened.

`CLAUDE.md` asks for a mutation test on everything written here: break each
thing, confirm a *named* test fails, restore, re-verify. The discipline is
sound and doing it by hand has a failure mode that looks exactly like success.

**Five ways a mutation run lies. The first three were met by hand in one
session; the fourth was found by pointing this script at itself; the fifth was
this script's own fault:**

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

5. **A killed run leaves the tree mutated.** The restore lives in a `finally`,
   which does not run through a `SIGKILL` — and a mutation run is exactly what
   a timeout kills. One did, leaving `scripts/run_examples.sh` carrying a
   mutated roster line, and the next test run failed against it in a way that
   read as a broken test rather than a dirty tree. That is the first lie again
   with a longer fuse: a suite running against code nobody meant to be there.

Each is caught here rather than trusted to a reader's attention:

- the old text must occur **exactly once**, and the count is reported when not;
- the file is restored in a `finally`, so an interrupt does not leave a mutated
  tree, and an in-flight marker names the file and both versions, so a run that
  is *killed* — where no `finally` runs — is put back by the next one;
- the command's output is parsed for how many suites *reported*, and zero is a
  hard error that says so rather than a quiet pass;
- the spec arrives as JSON on stdin, so no replacement ever touches a shell;
- every run compiles into a fresh bytecode cache, so a same-size mutation
  cannot be scored against the previous one;
- **every run records itself** under `ledger/mutations/`, one file per run,
  including the runs that could not score. #285 found the `-q` defect above and
  then could not say which earlier runs it had spoiled, because a spec is
  written per run and stored nowhere — the only trace one could leave was a
  command line somebody happened to quote in a ledger entry, and none had. A
  defect in this script is only as expensive as the runs it silently ruined,
  and that number was unknowable.

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
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Where an in-flight mutation records itself, so a killed run is recoverable.
#:
#: The restore below lives in a `finally`, which does not run through a
#: `SIGKILL` — and a mutation run *is* the kind of thing that gets killed, by a
#: timeout or by somebody's patience. That happened: a timed-out run left
#: `scripts/run_examples.sh` carrying a mutated roster line, the next test run
#: failed against it, and the failure looked like a broken test rather than a
#: dirty tree. A fifth way a mutation run lies, and the one the script itself
#: causes.
#:
#: Written before the file is touched and removed after it is restored, so its
#: existence means exactly "a mutation is applied right now, or was when
#: something killed us".
#:
#: One per repository rather than one beside each subject, because the point is
#: to be *noticed*: the run that trips over a leftover mutation is usually a
#: run of something else entirely, which is how the original went unnoticed
#: until a test failed for no visible reason.
#:
#: `MUTATE_MARKER` overrides it, and only the tests beside this file set it.
#: They drive `mutate.py` as a subprocess *while mutating `mutate.py`*, so
#: parent and child would otherwise share one marker and recover each other's
#: state — which is not a hypothetical: it made a mutation of the restore path
#: read as a survivor when running the suite by hand caught it twice.
MARKER = Path(os.environ.get("MUTATE_MARKER", ROOT / ".mutate-in-flight.json"))

#: Where a finished run records itself. One file per run, never a shared log.
#:
#: #285 found a defect in this script — `cargo test -q` suppresses the lines
#: the `rust` dialect matches, so every mutation scored as a survivor — and
#: then could not answer the only question that mattered: which earlier runs
#: had it spoiled? Specs are written per run and passed on stdin, so the only
#: trace one could leave was a command line somebody happened to quote in a
#: ledger entry. None had. The blast radius was unmeasurable, and the entry
#: guessed at it, wrongly, until it was audited.
#:
#: One file per run rather than an appended log, for the reason `ledger/README`
#: gives for entries: several agents work here at once and a shared file
#: conflicts on every commit. The name carries the time and the mutated file,
#: so a directory listing is already the index.
#:
#: These sit under `ledger/` deliberately. The pre-commit hook's entry pattern
#: is `ledger/<date>-*.md` anchored at that directory, so a record is never
#: mistaken for an entry — but it *is* inside `ledger/`, so a commit carrying
#: only records still counts as ledger-only and needs no entry of its own.
RECORDS = Path(os.environ.get("MUTATE_RECORDS", ROOT / "ledger" / "mutations"))

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
    #
    # `(.+?)` and not `(\S+)`, because two of libtest's four name shapes
    # contain spaces and the strict version could match neither:
    #
    #   test a_unit_test ... FAILED
    #   test a_unit_test - should panic ... FAILED
    #   test crates/slate-orm/src/lib.rs - Enum (line 387) ... FAILED
    #   test crates/slate-orm/src/factory.rs - factory (line 15) - compile ... FAILED
    #
    # So a mutation caught by a `#[should_panic]` test, or by *any doctest at
    # all*, scored as UNREADABLE — which reads as "your command is wrong" and
    # sends the next person to fix a command that was already right. Both were
    # met in one session: the first mutating `loader_cliff`'s empty-store check
    # (#293), the second checking whether a `compile_fail` doctest defends the
    # cross-tenant `has_many` refusal (#294). It does; nothing could say so.
    #
    # Each shape was read off a real run rather than recalled — the first two
    # from `rustc --test` on a two-test file, the last two from
    # `cargo test -p slate-orm --doc`.
    #
    # ` - should panic` is stripped from the capture and ` - compile` is not,
    # deliberately: the point of the name is that a reader can re-run it, and
    # `cargo test a_unit_test` works while `cargo test "a_unit_test - should
    # panic"` does not. A doctest's name is its location either way.
    "rust": (
        re.compile(r"^test (.+?)(?: - should panic)? \.\.\. FAILED\s*$", re.MULTILINE),
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
    mutated = original.replace(mutation.old, mutation.new, 1)
    # The marker first, then the write. The other order leaves a window in
    # which the file is mutated and nothing says so, which is the whole failure
    # this is for.
    MARKER.write_text(
        json.dumps(
            {
                # Absolute: the tests beside this file drive it over fixtures
                # in a temporary directory, which have no path relative to the
                # repository at all.
                "file": str(path),
                "case": mutation.name,
                "original": original,
                "mutated": mutated,
            }
        ),
        encoding="utf-8",
    )
    path.write_text(mutated)
    return original


def restore(path: Path, original: str) -> None:
    """Put the file back and drop the marker, in that order."""
    path.write_text(original)
    MARKER.unlink(missing_ok=True)


def recover() -> int:
    """Put back what a killed run left mutated, or refuse and say so.

    Called before anything else. Restores only when the file still holds
    *exactly* what was written — anything else means somebody edited it since,
    and overwriting that would trade one silent wrong state for another.
    """
    if not MARKER.exists():
        return 0
    try:
        left = json.loads(MARKER.read_text(encoding="utf-8"))
        path = Path(left["file"])
    except (OSError, ValueError, KeyError) as why:
        print(
            f"{MARKER} exists and cannot be read ({why}).\n"
            "  A previous run was killed while a file was mutated. Check "
            "`git status`, put the file back, and delete the marker.",
            file=sys.stderr,
        )
        return 1
    if not path.exists():
        # The subject is gone — a fixture in a temporary directory, usually.
        # There is nothing to restore and nothing to warn about.
        #
        # The unlink is belt and braces and a mutation run says so: removing it
        # **survives**, because the run that follows writes the marker again
        # and removes it on the way out. What it covers is the path where that
        # run never gets that far — a spec naming a file that does not exist
        # raises in `apply_once` — and a stale marker then outlives the
        # process. Recorded rather than deleted, with the reason, because the
        # line costs nothing and the case it covers is the one nobody meets
        # until they do.
        MARKER.unlink(missing_ok=True)
        return 0
    try:
        current = path.read_text()
    except OSError as why:
        print(
            f"{MARKER} exists and cannot be read ({why}).\n"
            "  A previous run was killed while a file was mutated. Check "
            "`git status`, put the file back, and delete the marker.",
            file=sys.stderr,
        )
        return 1
    if current == left["mutated"]:
        path.write_text(left["original"])
        MARKER.unlink(missing_ok=True)
        print(
            f"recovered: {left['file']} was left mutated by a killed run "
            f"({left['case']!r}); restored."
        )
        return 0
    if current == left["original"]:
        MARKER.unlink(missing_ok=True)
        print(f"recovered: {left['file']} was already back; dropped the marker.")
        return 0
    print(
        f"{left['file']} was mutated by a killed run ({left['case']!r}) and "
        "has changed since.\n"
        "  Refusing to guess which version you want. Read `git diff` on it, "
        f"put it right, and delete {MARKER}.",
        file=sys.stderr,
    )
    return 1


def run(command: list[str], dialect: str) -> tuple[list[str], int, str, int]:
    """The command, its failing test names, how many suites reported, and the
    exit status.

    The exit status is returned so that `unreadable` can compare it against
    the names found. See that function for the failure it exists to catch.

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
    return (
        failed.findall(output),
        len(reported.findall(output)),
        output,
        finished.returncode,
    )


def unreadable(failures: list[str], reported: int, status: int) -> bool:
    """Whether the command failed in a way this could not read.

    **The fifth way a mutation run lies, and the worst so far**, because it
    fails silently in the safe-looking direction: every mutation is scored a
    survivor, which reads as "your tests are weak" rather than "this tool is
    not working".

    Met in #285, using this script. The `rust` dialect finds failing tests by
    `^test NAME ... FAILED$`, and **`cargo test -q` never prints that line** —
    `-q` suppresses the per-test results and leaves only `test result: FAILED.
    7 passed; 1 failed`, which satisfies the "did anything run" check and
    matches no name. A real mutation, one that a real test really caught,
    scored as surviving. Two of them did, and two `expect_survivor` records
    were written against results that meant nothing.

    The signal is an inconsistency the dialect cannot explain: the command
    exited non-zero, something reported, and no failing test was named. A
    genuine survivor exits zero. A mutation that will not compile reports
    nothing and is caught one branch earlier.
    """
    return status != 0 and reported > 0 and not failures


def describe_tree() -> str:
    """The commit a run was made against, or why it could not be read.

    Recorded because a verdict is about a tree, not about a file: "this
    mutation survived" means nothing without knowing what it survived against.
    """
    try:
        done = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=ROOT, capture_output=True, text=True, timeout=10, check=False,
        )
    except (OSError, subprocess.SubprocessError) as why:
        return f"unknown ({why})"
    return done.stdout.strip() or "unknown (no HEAD)"


def write_record(entry: dict) -> None:
    """Write one run's record, and never fail a run over it.

    A mutation run that dies because it could not write its own diary would be
    a sixth way for this script to lie, so every failure here is reported and
    swallowed. The verdict on the screen is the verdict.
    """
    try:
        RECORDS.mkdir(parents=True, exist_ok=True)
        stamp = entry["at"].replace(":", "").replace("-", "")[:15]
        slug = re.sub(r"[^a-z0-9]+", "-", entry["file"].lower()).strip("-")[:60]
        (RECORDS / f"{stamp}-{slug}.json").write_text(
            json.dumps(entry, indent=2) + "\n", encoding="utf-8"
        )
    except OSError as why:
        # Never fail a run over its own record.
        print(f"  (could not record this run: {why})")


def note(entry: dict, mutation, verdict: str, failures: list[str], reported: int) -> None:
    """Record one case's verdict, with the tests that named it.

    `caught_by` is the point: "survived" and "caught" are the headline, but the
    *names* are what make a verdict re-checkable later. A run whose cases were
    all caught by a test that no longer exists is a run worth re-reading.
    """
    entry["cases"].append(
        {
            "name": mutation.name,
            "old": mutation.old,
            "new": mutation.new,
            "verdict": verdict,
            "caught_by": failures[:8],
            "suites_reported": reported,
            "expect_survivor": mutation.expect_survivor,
        }
    )


def check(spec: dict) -> int:
    if recover():
        return 1
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

    # Begun before the baseline, and written in a `finally`, so the runs that
    # *cannot score* are recorded too. Those are the ones worth having: the
    # `-q` run #285 found scored nothing, and left nothing behind saying so.
    entry: dict = {
        "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "file": spec["file"],
        "dialect": dialect,
        "command": command,
        "commit": describe_tree(),
        "python": sys.version.split()[0],
        "outcome": "interrupted",
        "cases": [],
    }
    try:
        return scored(spec, entry, path, command, dialect, cases)
    finally:
        write_record(entry)


def scored(
    spec: dict,
    entry: dict,
    path: Path,
    command: list[str],
    dialect: str,
    cases: list,
) -> int:
    """The run itself. Split out so `check` can record it whatever happens."""
    # A baseline, because a mutation run says nothing if the suite was already
    # red. This is the check a hand-run mutation always skips and the one that
    # makes every result below mean something.
    failures, reported, output, status = run(command, dialect)
    if reported == 0:
        print(f"the command reported no test results at all:\n{output[-2000:]}")
        entry["outcome"] = "baseline-reported-nothing"
        return 1
    if unreadable(failures, reported, status):
        print(
            f"the command exited {status} but named no failing test, so this "
            f"cannot score anything.\n"
            f"  The {dialect!r} dialect does not match this command's output. "
            f"For `cargo test`, drop `-q`:\n"
            f"  it suppresses the per-test `... FAILED` lines this reads, and "
            f"every mutation then scores as a survivor.\n{output[-1500:]}"
        )
        entry["outcome"] = "baseline-unreadable"
        return 1
    if failures:
        print(f"the suite is red before any mutation: {failures[:5]}")
        entry["outcome"] = "baseline-red"
        return 1
    print(f"baseline: {reported} suites reported, none failing")

    problems = 0
    for mutation in cases:
        original = apply_once(path, mutation)
        try:
            failures, reported, output, status = run(command, dialect)
        finally:
            restore(path, original)

        if unreadable(failures, reported, status):
            print(
                f"  !! {mutation.name}: UNREADABLE — the command exited "
                f"{status}, something reported, and no failing test was "
                f"named, so this cannot score anything.\n"
                f"       The {dialect!r} dialect does not match this "
                f"command's output. For `cargo test`, drop `-q`: it "
                f"suppresses the\n"
                f"       per-test `... FAILED` lines this reads, and every "
                f"mutation then scores as a survivor."
            )
            note(entry, mutation, "unreadable", failures, reported)
            problems += 1
            continue

        if reported == 0:
            errors = [x for x in output.splitlines() if x.startswith("error")][:3]
            print(f"  !! {mutation.name}: NOTHING RAN — {errors}")
            note(entry, mutation, "nothing-ran", failures, reported)
            problems += 1
            continue

        caught = ", ".join(failures[:4]) if failures else ""
        if mutation.expect_survivor is None:
            if failures:
                print(f"  ok   {mutation.name}  ->  {caught}")
                note(entry, mutation, "caught", failures, reported)
            else:
                print(
                    f"  !!   {mutation.name}: SURVIVED ({reported} suites ran).\n"
                    "       A surviving mutation is a missing test or redundant "
                    "code. Write the test, or record why it cannot be caught\n"
                    "       with `expect_survivor`."
                )
                note(entry, mutation, "survived", failures, reported)
                problems += 1
        elif failures:
            print(
                f"  !!   {mutation.name}: expected to survive and was CAUGHT "
                f"by {caught}.\n"
                f"       The recorded reason has stopped being true: "
                f"{mutation.expect_survivor}"
            )
            note(entry, mutation, "caught-but-expected-to-survive", failures, reported)
            problems += 1
        else:
            print(f"  ok   {mutation.name}  ->  survived, as recorded")
            note(entry, mutation, "survived-as-recorded", failures, reported)

    # Restored and re-verified, which is the step `CLAUDE.md` names and which is
    # skipped most often: a mutation run that leaves the tree broken makes every
    # later result a lie.
    failures, reported, _, status = run(command, dialect)
    if failures or reported == 0 or unreadable(failures, reported, status):
        print(f"  !! the tree did not come back clean: {failures[:5]}")
        entry["restored_clean"] = False
        problems += 1
    else:
        print(f"restored: {reported} suites reported, none failing")
        entry["restored_clean"] = True
    entry["outcome"] = "problems" if problems else "clean"
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
