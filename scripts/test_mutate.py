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


def case(name: str, body: str, expect_code: int, expect_text: list[str]) -> bool:
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        subject = home / "subject.txt"
        subject.write_text("ORIGINAL\n")
        fake = home / "fake.py"
        fake.write_text(FAKE)
        spec = body.replace("__SUBJECT__", str(subject)).replace(
            "__FAKE__", f'"{sys.executable}", "{fake}", "{subject}"'
        )
        code, output = run(spec, subject, fake)
        problems = []
        if code != expect_code:
            problems.append(f"exit {code}, expected {expect_code}")
        for wanted in expect_text:
            if wanted not in output:
                problems.append(f"missing {wanted!r}")
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
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
