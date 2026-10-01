#!/usr/bin/env python3
"""`check_live_frame.py`, over trackers this file writes.

Against written verdicts rather than against this repository's own, for the
reason every guard test here gives: a check exercised only on today's tracker
asserts that today's tracker is clean, which is also what a check that does
nothing asserts. The guard's rules are about states this repository is
deliberately not in — an `open` row, a `narrowed` row nobody rostered, a
roster entry for a caveat that closed — and none of those can be reached by
running it on the real file.

The roster is swapped rather than the file, because `STILL_NARROWED` is the
half of the guard with judgement in it and a case that could not vary it would
be testing the loop and not the rule.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_live_frame as guard


def tracker(verdicts: list[dict]) -> Path:
    root = Path(tempfile.mkdtemp())
    (root / "docs").mkdir(parents=True)
    path = root / "docs" / "caveat-status.json"
    path.write_text(json.dumps({"verdicts": verdicts}, indent=2), encoding="utf-8")
    return path


def case(
    name: str,
    verdicts: list[dict],
    roster: dict[tuple[str, str], str],
    wanted: int,
) -> bool:
    """Run the guard against one written tracker and one written roster.

    Both globals are restored in a `finally`: a case that left `STATUS`
    pointing at a temporary directory would make every later case pass by
    reading an empty file, which is the never-fires shape one directory out.
    """
    status, still = guard.STATUS, guard.STILL_NARROWED
    guard.STATUS, guard.STILL_NARROWED = tracker(verdicts), roster
    try:
        got = guard.main()
    finally:
        guard.STATUS, guard.STILL_NARROWED = status, still
    if got == wanted:
        print(f"ok    {name}")
        return True
    print(f"FAIL  {name}: exit {got}, wanted {wanted}")
    return False


def row(entry: str, key: str, verdict: str) -> dict:
    return {"entry": entry, "key": key, "verdict": verdict}


ROSTERED = {("e.md", "half done"): "the other half needs MinIO"}


CASES = [
    (
        "a tracker with no open and every narrowed rostered passes",
        [row("e.md", "half done", "narrowed"), row("e.md", "settled", "deliberate")],
        ROSTERED,
        0,
    ),
    (
        "one open verdict fails",
        [row("e.md", "undecided", "open")],
        {},
        1,
    ),
    (
        # The control for the rule above: the other four verdicts all pass, so
        # the guard is refusing `open` rather than refusing a live frame.
        "closed, deliberate and moment all pass",
        [
            row("e.md", "done", "closed"),
            row("e.md", "decided", "deliberate"),
            row("e.md", "one run", "moment"),
        ],
        {},
        0,
    ),
    (
        "a narrowed verdict nobody rostered fails",
        [row("e.md", "half done", "narrowed")],
        {},
        1,
    ),
    (
        "a roster entry for a caveat that is no longer narrowed fails",
        [row("e.md", "half done", "closed")],
        ROSTERED,
        1,
    ),
    (
        "a roster entry with an empty reason fails",
        [row("e.md", "half done", "narrowed")],
        {("e.md", "half done"): "   "},
        1,
    ),
    (
        # A prefix short enough to match two caveats would leave one of them
        # unaccounted for while the roster looked complete.
        "a roster prefix matching two narrowed caveats fails",
        [row("e.md", "half", "narrowed"), row("e.md", "halfway", "narrowed")],
        {("e.md", "half"): "ambiguous on purpose"},
        1,
    ),
    (
        # The never-fires case. An empty tracker satisfies both rules by
        # having nothing to break them.
        "an empty tracker fails rather than passing on nothing",
        [],
        {},
        1,
    ),
    (
        # The roster is matched on entry too, not on the key alone: two
        # entries can carry the same caveat sentence, and 2026-10-01 has a
        # pair that do.
        "a roster row matching the key but not the entry fails",
        [row("other.md", "half done", "narrowed")],
        ROSTERED,
        1,
    ),
]


def main() -> int:
    passed = sum(case(*c) for c in CASES)
    failed = len(CASES) - passed

    # And the real tracker once, which is the assertion the repository's own
    # state is held to. It is the weakest case here and it is the one that
    # notices the day somebody writes an `open` row.
    if guard.main() == 0:
        passed += 1
        print("ok    the real tracker has no open caveats")
    else:
        failed += 1
        print("FAIL  the real tracker has no open caveats")

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
