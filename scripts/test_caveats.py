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
            # `.get`, not `[...]`: a verdict dropped from `VERDICTS` should
            # fail this case, and indexing raises `KeyError` instead — which
            # kills the run before it prints its tally, so `mutate.py` reads
            # NOTHING RAN and cannot tell a caught mutation from a crashed
            # suite. Found by mutating `VERDICTS` and watching exactly that.
            if got_counts.get(verdict) != want:
                bad.append(f"{verdict}: wanted {want}, got {got_counts.get(verdict)}")
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
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "open",
                        "by": "",
                    }
                ]
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
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "open",
                        "by": "",
                    }
                ]
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
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "closed",
                        "by": "",
                    }
                ]
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
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "deliberate",
                        "by": "",
                    }
                ]
            ),
        },
        {"deliberate": 1},
        1,
    )

    # `narrowed` is held to `by` for a stronger reason than `closed` is: the
    # whole point of the verdict is that part of the claim is still true, so a
    # `narrowed` with nothing written down is a caveat nobody can act on — it
    # says "some of this is done" and not which part.
    case(
        "narrowed without naming what closed and what is left is refused",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "narrowed",
                    }
                ]
            ),
        },
        {"narrowed": 1},
        1,
    )

    case(
        "narrowed with a `by` is accepted and counted as its own thing",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "narrowed",
                        "by": "half of it shipped in b.md; the other half is open",
                    }
                ]
            ),
        },
        {"narrowed": 1},
        0,
    )

    case(
        "an unknown verdict is refused and counted untriaged",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "probably-fine",
                        "by": "x",
                    }
                ]
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

    # The two bugs the paragraph-boundary fix was for, one in each direction.
    case(
        "the first caveat of a section is found",
        {"ledger/a.md": "# E\n\n## What this does not do\n\n**The first one.** Body.\n"},
        {"untriaged": 1},
        0,
    )

    case(
        "emphasis at a line break is not a caveat",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "**A real one.** A sentence that wraps so that it is\n"
                "**12** of 24, not 11, which is emphasis and not a caveat.\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    case(
        "a struck-through withdrawal is not a caveat",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "~~**Withdrawn.** It was closed later.~~\n\n"
                "**Still open.** Body.\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    # The fifth verdict, and the listing. Both were added while triaging 677
    # caveats and neither had a case until the entry that added them recorded
    # that as a gap.
    case(
        "a moment is counted as its own verdict, not as open",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": guard.key("It does not do the first thing."),
                        "verdict": "moment",
                        "by": "",
                    }
                ]
            ),
        },
        {"moment": 1, "open": 0, "untriaged": 1},
        0,
    )

    # `moment` is the one verdict that needs no `by`: the entry's date is the
    # context, and demanding a citation for "this was true that afternoon"
    # would be theatre. `closed` and `deliberate` are still held to it.
    case(
        "a moment needs no `by`, where closed and deliberate do",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": guard.key("It does not do the first thing."),
                        "verdict": "moment",
                        "by": "",
                    },
                    {
                        "entry": "a.md",
                        "key": guard.key("It does not do the second thing."),
                        "verdict": "closed",
                        "by": "",
                    },
                ]
            ),
        },
        {"moment": 1, "closed": 1},
        1,
    )

    # A withdrawal is not a caveat. This shape — the word rather than the
    # strikethrough — was the last of 772 the triage could not give a verdict.
    case(
        "a caveat withdrawn in prose is not counted",
        {
            "ledger/a.md": (
                "# An entry\n\n## What this does not do\n\n"
                "**Withdrawn, 2026-09-14.** Wrong on both halves, and here "
                "is why it was wrong.\n\n"
                "**It does not do the first thing.** A reason.\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    # The listing is the answer to the question the counts only size.
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(
            root,
            {
                "ledger/a.md": ENTRY,
                "docs/caveat-status.json": status(
                    [
                        {
                            "entry": "a.md",
                            "key": guard.key("It does not do the first thing."),
                            "verdict": "open",
                            "by": "",
                        }
                    ]
                ),
            },
        )
        lines = guard.listing("open", root)
        if lines == ["a.md: It does not do the first thing."]:
            print("ok    the listing names the caveat, not just how many")
            RESULTS.append(True)
        else:
            print(f"FAIL  the listing returned {lines!r}")
            RESULTS.append(False)

    # --- `--unread`, the re-triage worklist -------------------------------
    #
    # It cannot decide whether a claim is still true. What it must get right is
    # *which* caveats it puts in front of a reader, and the two ways to get
    # that wrong are opposite: listing one somebody read yesterday wastes the
    # reading, and dropping one nobody has read hides it.
    def unread_case(name: str, verdicts: list[dict[str, str]], days: int, want: int) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree(root, {"ledger/a.md": ENTRY, "docs/caveat-status.json": status(verdicts)})
            got = guard.unread(days, root, today="2026-09-25")
            if len(got) != want:
                print(f"FAIL  {name}: wanted {want} listed, got {len(got)}: {got}")
                RESULTS.append(False)
            else:
                print(f"ok    {name}")
                RESULTS.append(True)

    both_open = [
        {"entry": "a.md", "key": "It does not do the first thing.", "verdict": "open"},
        {"entry": "a.md", "key": "It does not do the second thing.", "verdict": "open"},
    ]
    unread_case("an open caveat nobody stamped is listed", both_open, 30, 2)

    stamped = [dict(both_open[0], checked="2026-09-25"), both_open[1]]
    unread_case("one read today drops off the list", stamped, 30, 1)

    # A `narrowed` caveat still has a residual, so it is still work and still
    # wants re-reading. Listing only `open` would quietly retire the half that
    # is not done — which is the failure `narrowed` was invented to avoid,
    # reappearing one layer down.
    half = [
        dict(both_open[0], verdict="narrowed", by="half shipped; half open"),
        both_open[1],
    ]
    unread_case("a narrowed caveat is re-read like an open one", half, 30, 2)

    settled = [
        dict(both_open[0], verdict="closed", by="b.md"),
        dict(both_open[1], verdict="deliberate", by="a reason"),
    ]
    unread_case("closed and deliberate are not re-read", settled, 30, 0)

    stale = [dict(both_open[0], checked="2026-01-01"), both_open[1]]
    unread_case("a stamp older than the window is listed again", stale, 30, 2)
    unread_case("and is not listed when the window reaches it", stale, 400, 1)

    unread_case(
        "a closed caveat is not re-triage work",
        [
            {
                "entry": "a.md",
                "key": "It does not do the first thing.",
                "verdict": "closed",
                "by": "something",
            },
            both_open[1],
        ],
        30,
        1,
    )
    unread_case(
        "a deliberate one is not either",
        [
            {
                "entry": "a.md",
                "key": "It does not do the first thing.",
                "verdict": "deliberate",
                "by": "a reason",
            },
            both_open[1],
        ],
        30,
        1,
    )
    # A date nobody can read is not a date somebody checked. The alternative —
    # treating it as absent — is the same list with the error invisible, and
    # the alternative to *that* is raising, which takes the whole run down for
    # one typo in a file of 800 entries.
    unread_case(
        "an unreadable stamp is listed, with the text that could not be read",
        [dict(both_open[0], checked="yesterday"), both_open[1]],
        30,
        2,
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
