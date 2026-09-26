#!/usr/bin/env python3
"""`check_caveat_citations.py`, over trees this file writes.

Against a written tree rather than against `slate-orm`, for the reason the
other guards here give: a check tested only by running it over this repository
asserts that today's file is clean, which is also what a check that does
nothing asserts. Every case below breaks one thing and names what must fail.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_caveat_citations as guard


def tree(verdicts: list[dict], files: tuple[str, ...] = ()) -> Path:
    root = Path(tempfile.mkdtemp())
    (root / "docs").mkdir(parents=True, exist_ok=True)
    (root / "docs" / "caveat-status.json").write_text(
        json.dumps({"verdicts": verdicts}, indent=2), encoding="utf-8"
    )
    for name in files:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("x\n", encoding="utf-8")
    return root


def case(name: str, verdicts, files, failing: set[str]) -> bool:
    root = tree(verdicts, files)
    report = guard.check(root)
    failed = [what for what, ok, _ in report if not ok]
    unexpected = [f for f in failed if not any(frag in f for frag in failing)]
    unseen = [frag for frag in failing if not any(frag in f for f in failed)]
    if unexpected or unseen:
        print(f"FAIL  {name}")
        for f in unexpected:
            print(f"        unexpected failure: {f}")
        for frag in unseen:
            print(f"        expected a failure mentioning {frag!r}, got {failed}")
        return False
    print(f"ok    {name}")
    return True


def missing(name: str) -> bool:
    """No `docs/caveat-status.json` at all must fail, not pass with nothing to check."""
    root = Path(tempfile.mkdtemp())
    failed = [what for what, ok, _ in guard.check(root) if not ok]
    if not any("exists" in f for f in failed):
        print(f"FAIL  {name}\n        got {failed}")
        return False
    print(f"ok    {name}")
    return True


def malformed(name: str) -> bool:
    """A file that is not JSON must fail rather than read as zero verdicts."""
    root = Path(tempfile.mkdtemp())
    (root / "docs").mkdir(parents=True)
    (root / "docs" / "caveat-status.json").write_text("{not json", encoding="utf-8")
    failed = [what for what, ok, _ in guard.check(root) if not ok]
    if not any("parses" in f for f in failed):
        print(f"FAIL  {name}\n        got {failed}")
        return False
    print(f"ok    {name}")
    return True


def parses(name: str, by: str, want: list[str]) -> bool:
    got = guard.citations(by)
    if got != want:
        print(f"FAIL  {name}\n        {by!r}\n        got {got}, want {want}")
        return False
    print(f"ok    {name}")
    return True


def main() -> int:
    passed = [
        case(
            "a file whose every citation resolves passes",
            [{"entry": "a.md", "key": "k", "verdict": "closed", "by": "ledger/b.md did it"}],
            ("ledger/b.md",),
            set(),
        ),
        case(
            "a citation naming a file that does not exist is reported",
            # The defect this was written for: `scripts/check_unverifiable_claims.py`
            # in a verdict written four hours earlier, a file that never existed.
            [{"entry": "a.md", "key": "k", "verdict": "deliberate",
              "by": "the class `scripts/check_unverifiable_claims.py` refuses it"}],
            (),
            {"every cited path resolves"},
        ),
        case(
            "the failure names the entry and the path, not just a count",
            [{"entry": "an-entry.md", "key": "k", "verdict": "closed", "by": "crates/gone.rs"}],
            (),
            {"every cited path resolves"},
        ),
        case(
            "a by that cites only the caveat itself is not a failure",
            # 38 verdicts written on 2026-09-26 are of this form. They are
            # honest and unfollowable, and demanding a path would invite the
            # fabrication this guard exists to catch.
            [{"entry": "a.md", "key": "k", "verdict": "deliberate",
              "by": "the caveat itself: the alternative is guessing"},
             {"entry": "b.md", "key": "k", "verdict": "closed", "by": "ledger/c.md"}],
            ("ledger/c.md",),
            set(),
        ),
        case(
            "a file where no verdict cites any path is reported, not passed",
            # The never-fires guard. A renamed field reads exactly like a file
            # whose every citation resolves.
            [{"entry": "a.md", "key": "k", "verdict": "deliberate", "by": "this entry"}],
            (),
            {"cites a path at all"},
        ),
        case(
            "an empty verdict list is reported rather than passing empty",
            [],
            (),
            {"cites a path at all"},
        ),
        case(
            "a verdict with no by at all does not crash",
            [{"entry": "a.md", "key": "k", "verdict": "open"},
             {"entry": "b.md", "key": "k", "verdict": "closed", "by": "ledger/c.md"}],
            ("ledger/c.md",),
            set(),
        ),
        case(
            "a by of null does not crash",
            [{"entry": "a.md", "key": "k", "verdict": "open", "by": None},
             {"entry": "b.md", "key": "k", "verdict": "closed", "by": "ledger/c.md"}],
            ("ledger/c.md",),
            set(),
        ),
        case(
            "a directory citation resolves",
            # `ledger/mutations/` is cited as a directory by several verdicts.
            [{"entry": "a.md", "key": "k", "verdict": "closed",
              "by": "ledger/mutations/ holds 80 records"}],
            ("ledger/mutations/r.json",),
            set(),
        ),
        case(
            "one dead citation among several live ones is still reported",
            [{"entry": "a.md", "key": "k", "verdict": "closed",
              "by": "ledger/b.md and scripts/gone.py and crates/c.rs"}],
            ("ledger/b.md", "crates/c.rs"),
            {"every cited path resolves"},
        ),
        missing("a missing status file is reported rather than passing empty"),
        malformed("a status file that is not JSON is reported rather than passing empty"),
        parses("a trailing full stop is not part of the path",
               "closed by ledger/a-b-c.md.", ["ledger/a-b-c.md"]),
        parses("a trailing comma is not part of the path",
               "ledger/a.md, and more", ["ledger/a.md"]),
        parses("a path inside backticks is found",
               "the class `scripts/x.py` refuses it", ["scripts/x.py"]),
        parses("a relative path out of the tree is not a citation",
               "see ../scripts/x.py", []),
        parses("a word ending in a tree name is not a citation",
               "in xledger/a.md", []),
        parses("the same path twice is one citation",
               "ledger/a.md and again ledger/a.md", ["ledger/a.md"]),
        # `ledger/2026-09-22 #285` was written as prose meaning "the 09-22 entry
        # for task #285". It is path-shaped, so it is matched and then fails to
        # resolve — which is the right outcome: a citation a tool cannot follow
        # should be spelled as one it can, and both were.
        parses("a prose reference that looks like a path is still matched",
               "ledger/2026-09-22 #285 — four dependencies stamped",
               ["ledger/2026-09-22"]),
        parses("a bare filename is not a citation",
               "fixed in check_cost_prose.py", []),
        parses("two different paths are both found, in order",
               "crates/a.rs then ledger/b.md", ["crates/a.rs", "ledger/b.md"]),
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
