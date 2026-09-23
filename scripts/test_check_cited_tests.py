#!/usr/bin/env python3
"""Tests for `check_cited_tests.py`, over a tree this file builds.

Against the repository a guard like this passes for whatever reason the
repository happens to supply, and would go on passing if it stopped checking
anything. Each case here builds the smallest tree that exhibits one rule and
runs the real `main()` over it, so the *rule* is what is under test.

The cases that matter most are the two that must NOT fire: this guard's whole
justification is that a naive version flags 87% noise, and a guard people
switch off is worth less than no guard.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_cited_tests


def tree(root: pathlib.Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)


def run(files: dict[str, str]) -> tuple[int, str]:
    """Run the guard over a built tree, capturing what it says.

    Captured rather than let through: eight cases each printing a finding and
    a four-line explanation buries the one line that says which case failed,
    which is the same way an ENOSPC run once read as a clean one. The text is
    returned instead, so a case can assert on it.
    """
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        tree(root, files)
        (root / "docs").mkdir(exist_ok=True)
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check_cited_tests.main([str(root)])
        return code, out.getvalue()


#: The name every "must fail" case cites, and which the finding must name back.
DEAD = "a_test_that_was_renamed_by_its_own_fix"

LIVE = "crates/x/tests/probe.rs"
SUITE = "#[test]\nfn a_live_test_that_really_does_exist() {}\n"

CASES: list[tuple[str, dict[str, str], int]] = [
    (
        "a doc naming a test that exists passes",
        {LIVE: SUITE,
         "docs/d.md": "Demonstrated by `a_live_test_that_really_does_exist`.\n"},
        0,
    ),
    (
        "a doc naming a test that does not exist fails",
        {LIVE: SUITE,
         "docs/d.md": "Demonstrated by `a_test_that_was_renamed_by_its_own_fix`.\n"},
        1,
    ),
    (
        "a dead name beside a live one is history, and passes",
        {LIVE: SUITE,
         "docs/d.md": "It is `a_live_test_that_really_does_exist` now, and was "
                      "`a_test_that_was_renamed_by_its_own_fix` before.\n"},
        0,
    ),
    (
        "a dead name in a paragraph of its own still fails",
        {LIVE: SUITE,
         "docs/d.md": "See `a_live_test_that_really_does_exist`.\n\n"
                      "And `a_test_that_was_renamed_by_its_own_fix`.\n"},
        1,
    ),
    (
        "a four-word name is below the threshold and is ignored",
        {LIVE: SUITE, "docs/d.md": "Set `max_decoding_message_size` on the channel.\n"},
        0,
    ),
    (
        "a python test cited without its test_ prefix resolves",
        {"clients/python/tests/t.py": "def test_a_count_the_keys_do_not_match() -> None:\n    pass\n",
         "docs/d.md": "Demonstrated by `a_count_the_keys_do_not_match`.\n"},
        0,
    ),
    (
        "the ledger is out of scope, because an entry is a dated record",
        {LIVE: SUITE,
         "ledger/e.md": "There was a test called `a_test_that_was_renamed_by_its_own_fix`.\n"},
        0,
    ),
    (
        "a doc with no citations at all passes",
        {LIVE: SUITE, "docs/d.md": "Prose with `snake_case` and `a_short_name` only.\n"},
        0,
    ),
]


def main() -> int:
    failed = 0
    for name, files, expected in CASES:
        got, said = run(files)
        # The exit code alone would pass for a guard that fails on everything,
        # so a case that expects a finding also requires the finding to name
        # the dead test rather than merely to be non-zero.
        ok = got == expected and (expected == 0 or DEAD in said)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected}, got {got}")
            for line in said.splitlines():
                print(f"      {line}")
    print(f"\n{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
