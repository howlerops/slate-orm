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


#: The ledger's "answered later" shape: the closure struck through, the original
#: bullet left standing underneath. Both produce the same key, and only the
#: standing one reaches the tracker — which is how a caveat closed in September
#: was still recorded `open` in this repository months later.
STRUCK = """# An entry

## What this does not do

- ~~**It does not do the first thing.**~~ **Closed** by
  `2026-09-20-the-entry-that-closed-it.md`, which cost one fixture table
  rather than the rewrite estimated below.

- **It does not do the first thing.** With the original estimate, standing
  unedited, because being wrong about it is the useful part.

**It does not do the second thing.** Also with a sentence.
"""


#: The same shape, with a strike that credits nobody.
#:
#: 30 of the 51 strikes in this ledger are prose like this — "Closed, the same
#: afternoon" — and the `by` rule has nothing to check them against. A rule
#: that demanded a citation from every strike would be a rule about how to
#: write an entry, which is the `pre-commit` hook's business and not this file's.
STRUCK_ANONYMOUS = """# An entry

## What this does not do

- ~~**It does not do the first thing.**~~ **Closed**, later the same afternoon,
  and it cost one fixture table rather than the rewrite estimated below.

- **It does not do the first thing.** With the original estimate, standing
  unedited, because being wrong about it is the useful part.

**It does not do the second thing.** Also with a sentence.
"""


#: A strike that names its own entry, and no other.
#:
#: An entry naming its own filename in a strike is narrating itself, not
#: pointing anywhere — a `by` citing it would send the reader back to the file
#: they are already in. So it credits nobody and the `by` goes unchecked, the
#: same as prose. No strike in this ledger does it today; the rule is here
#: because "the entry that closed it" and "the entry it is in" are the same
#: string when a session closes its own caveat in a later section.
STRUCK_SELF = """# An entry

## What this does not do

- ~~**It does not do the first thing.**~~ **Closed** further up
  `2026-09-19-an-entry.md` itself, which cost one fixture table rather than
  the rewrite estimated below.

- **It does not do the first thing.** With the original estimate, standing
  unedited, because being wrong about it is the useful part.

**It does not do the second thing.** Also with a sentence.
"""


#: A strike crediting two entries, because a closure can take two steps.
STRUCK_TWICE = """# An entry

## What this does not do

- ~~**It does not do the first thing.**~~ **Closed** by
  `2026-09-20-the-entry-that-closed-it.md`, after
  `2026-09-19-the-entry-that-started-it.md` made it possible.

- **It does not do the first thing.** With the original estimate, standing
  unedited, because being wrong about it is the useful part.

**It does not do the second thing.** Also with a sentence.
"""


#: A strike that mentions a Markdown file which is not a ledger entry.
#:
#: `docs/correctness.md` is where a finding gets written up, not somebody who
#: closed a caveat, and a `by` should not have to name it. The date prefix is
#: what separates an entry from every other `.md` in the tree, which is why
#: `CREDITED` requires one.
STRUCK_README = """# An entry

## What this does not do

- ~~**It does not do the first thing.**~~ **Withdrawn** — the property it
  asks for is already written up in `docs/correctness.md`, so there was
  nothing to do.

- **It does not do the first thing.** With the original estimate, standing
  unedited, because being wrong about it is the useful part.

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
        # Two problems, not one: no `by` and no `residual`. A narrowed caveat
        # has to say both halves and each is its own field.
        2,
    )

    case(
        "narrowed with a `by` and no `residual` is refused",
        # The hole `residual` was added for: a `by` that says what closed and
        # forgets what is left reads as a closure with a hedge, and nothing
        # could count what the narrowed caveats still owe.
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "narrowed",
                        "by": "half of it shipped in b.md",
                    }
                ]
            ),
        },
        {"narrowed": 1},
        1,
    )

    case(
        "narrowed with a `by` and a `residual` is accepted and counted as its own thing",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "narrowed",
                        "by": "half of it shipped in b.md",
                        "residual": "the other half is still open",
                    }
                ]
            ),
        },
        {"narrowed": 1},
        0,
    )

    # `checked` is a claim about the tree and `--unread` reads it, so a settled
    # verdict wearing one is a stamp nothing will ever look at again pretending
    # to be one that will. The reverse sweep put `checked` on 316 `deliberate`
    # rows before this told it not to.
    case(
        "a settled verdict carrying `checked` rather than `reviewed` is refused",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "deliberate",
                        "by": "the entry argues it",
                        "checked": "2026-09-26",
                    }
                ]
            ),
        },
        {"deliberate": 1},
        1,
    )

    # The third side, and the one that was written in the docstring and
    # enforced nowhere: `narrowed` is a live verdict with a residual to
    # re-read, so its date belongs in `checked`. Five rows in this repository
    # carried `reviewed` and were therefore invisible to `--unread` forever —
    # not listed as stale, never listed at all.
    case(
        "a narrowed caveat carrying `reviewed` rather than `checked` is refused",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "narrowed",
                        "by": "half of it shipped in b.md",
                        "residual": "the other half is still open",
                        "reviewed": "2026-09-26",
                    }
                ]
            ),
        },
        {"narrowed": 1},
        1,
    )

    case(
        "a narrowed caveat carrying `checked` is accepted",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "narrowed",
                        "by": "half of it shipped in b.md",
                        "residual": "the other half is still open",
                        "checked": "2026-09-26",
                    }
                ]
            ),
        },
        {"narrowed": 1},
        0,
    )

    # The other side of the same rule, and the one a mutation found missing:
    # `checked` is exactly right on an `open` row — it is what `--unread`
    # reads — so a rule written as "no row may carry `checked`" would pass
    # every test above and break the only field that drives a worklist.
    case(
        "an open caveat carrying `checked` is accepted",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "open",
                        "checked": "2026-09-26",
                    }
                ]
            ),
        },
        {"open": 1},
        0,
    )

    case(
        "a narrowed caveat carrying `checked` is accepted",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "narrowed",
                        "by": "half shipped",
                        "residual": "half open",
                        "checked": "2026-09-26",
                    }
                ]
            ),
        },
        {"narrowed": 1},
        0,
    )

    case(
        "a settled verdict carrying `reviewed` is accepted",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "deliberate",
                        "by": "the entry argues it",
                        "reviewed": "2026-09-26",
                    }
                ]
            ),
        },
        {"deliberate": 1},
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

    # The recall half, added 2026-09-28. The bold lead is a *convention*, and
    # it firmed up around the 20th: 379 paragraphs in this repository's own
    # `What this does not do` sections do not carry one, and every single one
    # of them was invisible — 129 entries had no caveat the tracker could see
    # at all. A tracker reporting "0 open" over 74% of the caveats is the
    # never-fires shape wearing a headline number.
    case(
        "a paragraph with no bold lead is a caveat too",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "No deployment. The site is files in a directory.\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    case(
        "a bold-lead caveat and a plain one are both found, and counted once",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "**A bold one.** With a sentence after it.\n\n"
                "A plain one, with no lead at all.\n"
            )
        },
        {"untriaged": 2},
        0,
    )

    # The key has to stay the bold text where there is one, or the widening
    # orphans every verdict this repository has written. Measured before the
    # change: 1065 keys before, 1065 of them still present after, 379 added,
    # nothing colliding.
    case(
        "a bold caveat keeps its key when plain paragraphs are read too",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "**A bold one.** With a sentence after it.\n\n"
                "A plain one.\n"
            ),
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "A bold one.",
                        "verdict": "open",
                        "by": "",
                    }
                ]
            ),
        },
        {"open": 1, "untriaged": 1},
        0,
    )

    # A plain withdrawal is dropped for the same reason a bold one is. It
    # could not arise before, because `WITHDRAWN` only ever saw bold text.
    case(
        "a plain withdrawal is not a caveat either",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "Withdrawn, 2026-09-14. The measurement did not hold.\n\n"
                "A real one.\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    # Three shapes that are *about* a caveat rather than being one. Each was
    # met while widening: 26 blockquoted `> **Closed on …**` notes, 3 fenced
    # blocks of captured output, and one `*(Closed, …)*` parenthetical. All
    # three read as claims once whole paragraphs were read, and none is one.
    case(
        "a blockquoted closure note is not a caveat",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "A real one.\n\n"
                "> **Closed on 2026-09-15** by `some-later-entry`.\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    case(
        "a fenced block of captured output is not a caveat",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "A real one.\n\n"
                "```\nTerminate orphan process: pid (3524)\n```\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    case(
        "a parenthetical closure note is not a caveat",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "A real one.\n\n"
                "*(Closed, 2026-09-14: the runner builds all three now.)*\n"
            )
        },
        {"untriaged": 1},
        0,
    )

    # A tight list is several caveats, not one. 53 of this repository's 54
    # list blocks hold more than one item, each with its own bold lead, and
    # reading the block as a single caveat would key all of them on the first
    # one's opening words — which is worse than missing them, because it looks
    # triaged.
    case(
        "each item of a tight list is its own caveat",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "- **The first.** With a sentence that\n  wraps onto a second line.\n"
                "- **The second.** Another.\n"
                "- **The third.** And another.\n"
            )
        },
        {"untriaged": 3},
        0,
    )

    case(
        "a list item keys on its bold lead, not on the dash",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "- **The first.** With a sentence.\n"
            ),
            "docs/caveat-status.json": status(
                [{"entry": "a.md", "key": "The first.", "verdict": "open", "by": ""}]
            ),
        },
        {"open": 1, "untriaged": 0},
        0,
    )

    # `.match`, never `.search`. 126 plain paragraphs in this repository carry
    # emphasis somewhere in the middle, and a searched lead would key them on
    # that fragment — 26 of them on the same six words, which collides.
    case(
        "mid-paragraph emphasis does not become a plain caveat's key",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "The runner is not wired in, and that is **the whole point**.\n"
            ),
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "The runner is not wired in, and that is **the whole point**.",
                        "verdict": "open",
                        "by": "",
                    }
                ]
            ),
        },
        {"open": 1, "untriaged": 0},
        0,
    )

    # The half `WITHDRAWN` does not cover, and the reason `~~` is in
    # `ANNOTATION` rather than left to it. 47 struck paragraphs here, and most
    # open with the retracted sentence rather than with the word "Withdrawn" —
    # "~~The runner is not wired into CI…~~". A mutation dropping `~~` from
    # `ANNOTATION` survived every case in this file until this one, because
    # every struck fixture happened to say the word.
    case(
        "a struck paragraph not worded as a withdrawal is still not a caveat",
        {
            "ledger/a.md": (
                "# E\n\n## What this does not do\n\n"
                "~~The runner is not wired into CI, so nothing runs it.~~\n\n"
                "A real one.\n"
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
        dict(both_open[0], verdict="narrowed", by="half shipped", residual="half open"),
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

    # `--residual` is the reason `residual` is a field rather than prose in
    # `by`: a narrowed caveat is partly work, and this is what counts it.
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
                            "key": "It does not do the first thing.",
                            "verdict": "narrowed",
                            "by": "half shipped in b.md",
                            "residual": "the other half is still open",
                        },
                        {
                            "entry": "a.md",
                            "key": "It does not do the second thing either.",
                            "verdict": "open",
                        },
                    ]
                ),
            },
        )
        lines = guard.residuals(root)
        want = ["a.md: the other half is still open"]
        if lines != want:
            print(f"FAIL  --residual lists what each narrowed caveat owes\n"
                  f"        got {lines}, want {want}")
            RESULTS.append(False)
        else:
            print("ok    --residual lists what each narrowed caveat owes")
            RESULTS.append(True)

    # The pair the strike rule exists for. `open` on a struck-and-standing
    # claim is the failure; `closed` on the same tree is not.
    case(
        "a claim that also stands struck through cannot be open",
        {
            "ledger/a.md": STRUCK,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "open",
                    },
                    {
                        "entry": "a.md",
                        "key": "It does not do the second thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 2, "untriaged": 0},
        1,
    )

    case(
        "the same pair recorded closed is fine",
        {
            "ledger/a.md": STRUCK,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "closed",
                        "by": "ledger/2026-09-20-the-entry-that-closed-it.md, "
                              "which the strike names",
                    },
                    {
                        "entry": "a.md",
                        "key": "It does not do the second thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 1, "closed": 1, "untriaged": 0},
        0,
    )

    # The gap the rule above left, which its own entry recorded: a `closed`
    # row whose `by` describes the work instead of naming where it is written
    # down. Two of the three real pairs were doing exactly this.
    case(
        "a closed claim whose `by` does not name the entry the strike credits",
        {
            "ledger/a.md": STRUCK,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "closed",
                        "by": "one fixture table, added later",
                    },
                    {
                        "entry": "a.md",
                        "key": "It does not do the second thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 1, "closed": 1, "untriaged": 0},
        1,
    )

    # And the limit of it: a strike naming no entry leaves nothing to check,
    # so the same prose `by` passes. Written because a rule that fired on every
    # `by` without a `.md` in it would pass the case above and reject most of
    # this ledger.
    case(
        "a strike crediting nobody leaves the `by` unchecked",
        {
            "ledger/a.md": STRUCK_ANONYMOUS,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "closed",
                        "by": "one fixture table, added later",
                    },
                    {
                        "entry": "a.md",
                        "key": "It does not do the second thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 1, "closed": 1, "untriaged": 0},
        0,
    )

    # The self-credit case. Without the exclusion this reads as a strike
    # crediting `a.md`, and the prose `by` below is then refused for not
    # naming the file the caveat is already in.
    case(
        "a strike naming only its own entry credits nobody",
        {
            "ledger/2026-09-19-an-entry.md": STRUCK_SELF,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "2026-09-19-an-entry.md",
                        "key": "It does not do the first thing.",
                        "verdict": "closed",
                        "by": "one fixture table, added later",
                    },
                    {
                        "entry": "2026-09-19-an-entry.md",
                        "key": "It does not do the second thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 1, "closed": 1, "untriaged": 0},
        0,
    )

    # Two credits, one named. `any`, not `all`: the second real pair took two
    # entries to close — one gave `purge_deleted` a caller and the other gave
    # it a schedule — and naming either gets a reader to the work, which is
    # what `by` is for. Demanding both would make a `by` a bibliography.
    case(
        "a `by` naming one of two credited entries is enough",
        {
            "ledger/a.md": STRUCK_TWICE,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "closed",
                        "by": "ledger/2026-09-20-the-entry-that-closed-it.md",
                    },
                    {
                        "entry": "a.md",
                        "key": "It does not do the second thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 1, "closed": 1, "untriaged": 0},
        0,
    )

    # A `.md` that is not an entry is not a credit. Without the date in
    # `CREDITED` this strike reads as crediting `correctness.md`, and the prose
    # `by` below is refused for not naming a file that never closed anything.
    case(
        "a strike naming a non-entry Markdown file credits nobody",
        {
            "ledger/a.md": STRUCK_README,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "closed",
                        "by": "the convention was already written down",
                    },
                    {
                        "entry": "a.md",
                        "key": "It does not do the second thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 1, "closed": 1, "untriaged": 0},
        0,
    )

    # And the other direction: an `open` caveat in an entry with *no* strike
    # must not be flagged. Written because a rule that fired on every open
    # caveat would pass the two cases above and be useless.
    case(
        "an open claim with nothing struck is not flagged",
        {
            "ledger/a.md": ENTRY,
            "docs/caveat-status.json": status(
                [
                    {
                        "entry": "a.md",
                        "key": "It does not do the first thing.",
                        "verdict": "open",
                    },
                ]
            ),
        },
        {"open": 1, "untriaged": 1},
        0,
    )

    # The real tracker, last, for the reason every guard's test here gives:
    # the cases above are written trees, so every rule passes for as long as
    # the fixtures stay correct, which is also what a rule aimed at nothing
    # does. `report()` is the whole of what `check.sh` runs, so this is CI's
    # own condition under a name a mutation run can score — and until it was
    # here, `scripts/mutate.py` could not score a change to
    # `docs/caveat-status.json` at all: no suite read the real file, so
    # breaking a verdict left every suite green.
    counts, problems, orphans = guard.report(guard.ROOT)
    RESULTS.append(not problems and not orphans)
    if problems or orphans:
        print("FAIL  the real tracker: every verdict is well formed")
        for one in (problems + orphans)[:10]:
            print(f"        {one}")
    else:
        print(
            "ok    the real tracker: every verdict is well formed  "
            f"({sum(counts.values())} caveats)"
        )

    print(f"\n{sum(RESULTS)} passed, {len(RESULTS) - sum(RESULTS)} failed")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
