#!/usr/bin/env python3
"""`caveats.py`, over trees this file writes.

Against a written tree rather than against the repository, for the reason the
other guards here give: a check tested only by running it over `slate-orm` can
only assert that today's tree is clean, which is also what a check that does
nothing asserts.

The orphan case is the one that matters. A verdict is keyed by the opening of
the bullet it judges, so editing a bullet detaches its verdict — and the
failure mode that would hurt is doing that *silently*, quietly reverting a
triaged caveat to untriaged where nobody looks at it again.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import caveats as guard

RESULTS: list[bool] = []

#: A bullet longer than `KEY`, so that editing it past the cut exercises the
#: truncation. Written out rather than generated: the point is the length.
LONG = (
    "# An entry\n\n## What this does not do\n\n"
    "**It does not cover the case where a sentence runs on well past sixty "
    "characters before it reaches the detail that later gets corrected.** "
    "And a sentence after it.\n"
)

ENTRY = """# An entry

## What this does not do

**It does not do the first thing.** With a sentence after it.

**It does not do the second thing.** Also with a sentence.
"""


def tree(root: Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")


def status(verdicts: list[dict[str, str]]) -> str:
    return json.dumps({"note": "fixture", "verdicts": verdicts}, indent=1)


def case(name: str, files: dict[str, str], counts: dict[str, int], problems: int) -> None:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(root, files)
        got_counts, got_problems, orphans = guard.report(root)
        bad = []
        for verdict, want in counts.items():
            if got_counts[verdict] != want:
                bad.append(f"{verdict}: wanted {want}, got {got_counts[verdict]}")
        if len(got_problems) + len(orphans) != problems:
            bad.append(
                f"problems: wanted {problems}, got "
                f"{len(got_problems) + len(orphans)} {got_problems + orphans}"
            )
        if bad:
            print(f"FAIL  {name}: {'; '.join(bad)}")
            RESULTS.append(False)
            return
        print(f"ok    {name}")
        RESULTS.append(True)


def main() -> int:
    case(
        "a bullet with no verdict is untriaged",
        {"ledger/a.md": ENTRY},
        {"untriaged": 2, "open": 0},
        0,
    )

    case(
        "a verdict is read by the opening of its bullet",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": "It does not do the first thing.",
                  "verdict": "open", "by": ""}]
            ),
        },
        {"open": 1, "untriaged": 1},
        0,
    )

    # The case this file exists for.
    case(
        "a reworded bullet orphans its verdict, and says so",
        {
            "ledger/a.md": ENTRY.replace("the first thing", "the first thing, reworded"),
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": "It does not do the first thing.",
                  "verdict": "open", "by": ""}]
            ),
        },
        {"untriaged": 2, "open": 0},
        1,
    )

    case(
        "closed without naming what closed it is refused",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": "It does not do the first thing.",
                  "verdict": "closed", "by": ""}]
            ),
        },
        {"closed": 1},
        1,
    )

    case(
        "deliberate without naming the reasoning is refused",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": "It does not do the first thing.",
                  "verdict": "deliberate", "by": ""}]
            ),
        },
        {"deliberate": 1},
        1,
    )

    case(
        "an unknown verdict is refused and counted untriaged",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": "It does not do the first thing.",
                  "verdict": "probably-fine", "by": "x"}]
            ),
        },
        {"untriaged": 2},
        1,
    )

    case(
        "README and TEMPLATE are not entries",
        {"ledger/README.md": ENTRY, "ledger/TEMPLATE.md": ENTRY, "ledger/a.md": ENTRY},
        {"untriaged": 2},
        0,
    )

    case(
        "an entry with no such section contributes nothing",
        {"ledger/a.md": "# An entry\n\n## Why\n\nBecause.\n"},
        {"untriaged": 0},
        0,
    )

    # `KEY` exists so that fixing a typo late in a long bullet does not orphan
    # its verdict. Found missing by mutation: `KEY = 10000` survived, because
    # every other fixture here is shorter than the cut.
    long_key = guard.key(
        "It does not cover the case where a sentence runs on well past sixty "
        "characters before it reaches the detail that later gets corrected."
    )
    case(
        "a verdict survives an edit past the key's cut",
        {
            "ledger/a.md": LONG.replace("gets corrected", "gets corrected later"),
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": long_key, "verdict": "open", "by": ""}]
            ),
        },
        {"open": 1, "untriaged": 0},
        0,
    )

    case(
        "an edit before the cut still orphans it",
        {
            "ledger/a.md": LONG.replace("does not cover", "does not yet cover"),
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": long_key, "verdict": "open", "by": ""}]
            ),
        },
        {"untriaged": 1, "open": 0},
        1,
    )

    # The never-fires guard: a ledger that moved finds nothing and reads
    # exactly like a repository that never wrote a caveat down.
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(root, {"notledger/a.md": ENTRY})
        if guard.main(root) == 0:
            print("FAIL  an empty ledger passed, which it must not")
            RESULTS.append(False)
        else:
            print("ok    a tree with no caveats at all is refused")
            RESULTS.append(True)

    print(f"\n{sum(RESULTS)} passed, {len(RESULTS) - sum(RESULTS)} failed")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
