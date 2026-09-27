#!/usr/bin/env python3
"""Every `docs/*.md` a source file points at must be openable.

The companion to `check_cited_tests.py`, one level over. That one catches a
document naming a test that no longer exists; this one catches *code* naming a
document that no longer exists — a comment or, worse, an error message that
sends a reader to a file that is not there.

It was written after adding an error message that ends "See docs/ctes.md",
noticing that nothing would notice if the file were deleted, and writing that
down as a gap rather than closing it. Closing it is one loop.

WHERE IT LOOKS

Source — `.rs`, `.py`, `.go`, `.ts` — **and prose, under `docs/` and
`ledger/`**. `node_modules`, `dist`, `target` and other vendored or generated
trees are skipped. Measured before that list was written: `playwright-core`'s
type definitions alone cite `docs/user_data_dir.md` and
`docs/chromium_browser_vs_google_chrome.md`, neither of which is ours and
neither of which should exist here.

# The prose half, and the argument it overturned

`docs/` and `ledger/` were skipped **as sources**, on two reasons that read
well and were both wrong in the same way:

  * `docs/` cross-links are Markdown links, and `site/check/docs.py` already
    requires every relative link to resolve. True of a link; a bare path in a
    sentence is not one.
  * `ledger/` cites paths that were right on the day, which is what a dated
    record is for. An entry saying "moved to `docs/x.md`" is provenance even
    after `x.md` moves again, and rewriting it destroys the thing a ledger is.

The second is the interesting one, because it is a real property of a ledger
and it does not cover the case that matters. A citation that resolved when it
was written and has since moved is provenance. A citation that **never**
resolved is a fabrication, and the two are only distinguishable by asking
whether the file ever existed — which is exactly what the roster below does,
one sentence at a time.

That distinction is not academic here. This repository has recorded **seven**
invented citations in a week, each a plausible filename recalled instead of
looked up, and every one was caught because the claim happened to sit in a
tree something opened. Two were not: `2026-09-25-the-grammar-block-is-checked-now.md`
cited a `docs/sql.md` that has never existed, and the entry adding this guard
first cited a ledger entry that has never existed. Both were in prose, and
prose was the one tree nothing read.

Measured when this was widened: **439 citations across `docs/` and `ledger/`,
14 of them dead, 10 distinct.** Nine are deliberate — illustrative paths in an
entry *about* citations, an elided filename, the template's
`ledger/YYYY-MM-DD-slug.md` — and are rostered below with why. One was a
defect and is fixed. Zero were the "right on the day, moved since" case the
old exemption was written for, which is why a roster is affordable: it starts
at nine and grows only when somebody writes a path they mean not to resolve.

FIXTURES ARE NOT CITATIONS. `scripts/test_check_cited_tests.py` writes a
temporary tree containing `docs/d.md` and runs the other checker over it. That
string is a *fixture path*, not a claim about this repository, and it is
excluded by name rather than by a pattern — an exclusion with a name is one
somebody has to justify, and a pattern that happened to cover it would also
cover a real citation somebody wrote carelessly.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Where a citation counts, in code.
SOURCE_SUFFIXES = (".rs", ".py", ".go", ".ts")

#: The prose trees, read as sources rather than only as targets.
PROSE_TREES = ("docs", "ledger")

#: Trees that are vendored, generated, or not ours.
SKIP_PARTS = frozenset(
    {"node_modules", "dist", "dist-test", "target", ".git", "_proto", "pb"}
)

#: Files whose `docs/…` or `ledger/…` strings are fixtures rather than claims.
#:
#: A list you are forced to edit is a list that stays true: adding a file here
#: is a sentence somebody has to write, where a glob would be silent.
FIXTURES: dict[str, str] = {
    "scripts/test_check_cited_tests.py": (
        "writes a temporary tree containing `docs/d.md` and `ledger/e.md` and "
        "runs the cited-tests checker over it; the paths describe that tree, "
        "not this repository"
    ),
    "scripts/check_cited_docs.py": (
        "this file, whose docstring names the paths it deliberately does not check"
    ),
    "scripts/check_mutation_claims.py": (
        "its docstring shows the entry-filename shape it matches, "
        "`ledger/<date>-a-slug.md`, which is a pattern rather than a file"
    ),
    "scripts/test_caveats.py": (
        "writes temporary ledgers containing `ledger/a.md`, `ledger/README.md` "
        "and `ledger/TEMPLATE.md` and runs the caveat tracker over them; the "
        "paths describe those trees, not this repository"
    ),
    "scripts/test_check_retired_claims.py": (
        "writes temporary trees containing `ledger/e.md` and `ledger/gone.md` "
        "and runs the retired-claims checker over them; the second is missing "
        "on purpose, to prove that checker refuses a retirement citing nothing"
    ),
    "scripts/test_check_cited_docs.py": (
        "writes temporary trees containing invented `docs/…` paths, for the same "
        "reason the cited-tests guard does"
    ),
    "scripts/test_check_caveat_citations.py": (
        "writes temporary `docs/caveat-status.json` files whose verdicts cite "
        "`ledger/a.md`, `ledger/b.md` and `ledger/c.md`; several cases exist "
        "precisely to prove the caveat-citation checker reports a path that "
        "does not resolve, so the dead ones are the point"
    ),
}

#: Citations in prose that are deliberately not files, one line each.
#:
#: Keyed by `(file, citation)` rather than by file, unlike `FIXTURES` above.
#: A fixture *script* is a fixture all the way through; a ledger entry is a
#: real record with one illustrative path in it, and exempting the whole file
#: would stop checking the citations that are claims. The narrowest exemption
#: that works is the one somebody has to widen.
#:
#: Same idiom, same reason: a list you are forced to edit is a list that stays
#: true. Each reason says why the path is not meant to resolve, and a reason
#: that has stopped being true is one somebody reads when the row fails.
NOT_A_FILE: dict[tuple[str, str], str] = {
    ("ledger/2026-09-20-a-path-in-a-string-literal-compiles.md", "docs/x.md"): (
        "the entry is about paths written in string literals; `docs/x.md` is "
        "its worked example of one, and a real filename there would make the "
        "example about a real file"
    ),
    ("ledger/2026-09-20-a-path-in-a-string-literal-compiles.md", "docs/d.md"): (
        "the fixture path `scripts/test_check_cited_tests.py` writes, quoted "
        "in the entry that explains why that script is exempt"
    ),
    (
        "ledger/2026-09-20-a-path-in-a-string-literal-compiles.md",
        "docs/user_data_dir.md",
    ): (
        "`playwright-core`'s own citation, quoted as the measurement that "
        "produced the vendored-tree skip list"
    ),
    (
        "ledger/2026-09-20-the-helper-that-can-be-used-now.md",
        "ledger/2026-09-20-...-the-accessor-three-adapters-now-call.md",
    ): (
        "an elided filename — the `...` is the elision — written before this "
        "guard existed and left because shortening a name in running prose is "
        "not a citation anybody would follow"
    ),
    ("ledger/2026-09-26-the-citation-nobody-could-follow.md", "ledger/a.md"): (
        "one of three illustrative entry names in the guard's own worked "
        "example, matching the fixtures `test_check_caveat_citations.py` writes"
    ),
    ("ledger/2026-09-26-the-citation-nobody-could-follow.md", "ledger/b.md"): "the second of those three",
    ("ledger/2026-09-26-the-citation-nobody-could-follow.md", "ledger/c.md"): "the third of those three",
    (
        "ledger/2026-09-26-the-refusal-nothing-ran.md",
        "ledger/2026-09-21-mutate-py-reads-three-more-runners.md",
    ): (
        "the sixth invented citation, quoted by the entry that reports it. "
        "Naming a fabrication is the opposite of making one, and spelling it "
        "some other way would make the report unreadable — but it is why this "
        "roster is keyed on the pair: the entry's other citations are claims"
    ),
    (
        "ledger/2026-09-27-a-ledger-entry-could-cite-anything.md",
        "docs/x.md",
    ): (
        "the entry that added this roster, quoting the docstring sentence it "
        "overturned — `an entry saying \"moved to docs/x.md\"` — whose whole "
        "point is a path that need not resolve"
    ),
    ("ledger/README.md", "ledger/YYYY-MM-DD-slug.md"): (
        "the filename shape a new entry takes, which is a pattern rather than "
        "a file"
    ),
}

#: A citation is a path under `docs/` or `ledger/` ending in `.md`.
#:
#: `ledger/` was added after `stats.rs` cited an entry and nothing would have
#: noticed if the name were typed wrong. Nine source files cite one, and the
#: reason this guard exists — an error message sending a reader to a file that
#: is not there — does not care which directory the file was in.
#:
#: This does not contradict the docstring's "never `docs/` or `ledger/`": that
#: is about which files are *searched*, and remains true. A ledger entry's own
#: citations are provenance and are still exempt. What is checked here is
#: *source* naming an entry, which is a live pointer rather than a dated one.
CITATION = re.compile(r"(?:docs|ledger)/[A-Za-z0-9_][A-Za-z0-9_.-]*\.md")


def source_files(root: Path) -> list[Path]:
    """Every source file a citation could live in, in a stable order."""
    found = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix not in SOURCE_SUFFIXES:
            continue
        relative = path.relative_to(root)
        if SKIP_PARTS & set(relative.parts):
            continue
        if relative.parts and relative.parts[0] in PROSE_TREES:
            continue
        if relative.as_posix() in FIXTURES:
            continue
        found.append(path)
    return found


def prose_files(root: Path) -> list[Path]:
    """Every `.md` under `docs/` and `ledger/`, in a stable order.

    Separate from `source_files` rather than a suffix added to it, because the
    two want different exemptions: a fixture script is exempt whole, and a
    prose file is exempt one citation at a time. Keeping them apart is what
    makes `NOT_A_FILE`'s narrowness possible.
    """
    found = []
    for tree in PROSE_TREES:
        for path in sorted((root / tree).rglob("*.md")):
            if SKIP_PARTS & set(path.relative_to(root).parts):
                continue
            found.append(path)
    return found


def check(
    root: Path, roster: dict[tuple[str, str], str] | None = None
) -> tuple[int, list[str]]:
    """Returns how many citations were seen, and what is broken.

    `roster` is an argument, and it has to be. `NOT_A_FILE`'s rows name real
    files in this repository, so over a fixture tree every one of them is
    "exempting a citation that is gone" and the stale-row check below buries
    whatever the fixture was about. That happened on the first run: fifteen of
    sixteen cases failed, all with the same nine lines. Defaulting to `None`
    rather than to `NOT_A_FILE` makes the fixture's answer the quiet one — the
    fourth time in this session a parameter with a real default made a test
    read the repository instead of its own tree, and the first where the
    default was the *roster* rather than the root.
    """
    if roster is None:
        roster = NOT_A_FILE
    seen = 0
    broken = []
    used: set[tuple[str, str]] = set()
    for path in source_files(root) + prose_files(root):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            # A source file this cannot read is not a citation it can check,
            # and refusing the whole run over one would make the guard the
            # thing that breaks rather than the thing that reports.
            continue
        here = path.relative_to(root).as_posix()
        for line_number, line in enumerate(text.splitlines(), start=1):
            for cited in CITATION.findall(line):
                if (here, cited) in roster:
                    # Counted as used, not as seen: a rostered path is not a
                    # citation this guard checked, and folding it into `seen`
                    # would let the never-fires guard below be satisfied by
                    # the roster alone.
                    used.add((here, cited))
                    continue
                seen += 1
                if not (root / cited).is_file():
                    broken.append(
                        f"{here}:{line_number} points at "
                        f"`{cited}`, which is not a file"
                    )

    # The roster's own never-fires half, and the one that matters most here: a
    # row whose file was deleted, or whose illustrative path was quietly made
    # real, is an exemption nobody is checking any more — and an exemption is
    # the only way a fabricated citation gets through. `FIXTURES` has no
    # equivalent and should; that is recorded rather than added, because a
    # whole-file exemption going stale hides less than one path does.
    for (where, cited), why in sorted(roster.items()):
        if (where, cited) not in used:
            broken.append(
                f"NOT_A_FILE exempts `{cited}` in {where}, which no longer "
                f"cites it.\n      The reason was: {why}\n      "
                "Delete the row — an exemption for a citation that is gone is "
                "one nobody reads before adding the next."
            )
    return seen, broken


def main(root: Path = ROOT) -> int:
    # `root` is an argument so the never-fires guard below can be tested. It
    # could not be: the tests call `check` directly, `main` read `ROOT`, and a
    # mutation deleting the guard entirely changed no verdict — this tree
    # always has citations, so the branch never ran in a test. Found by
    # `scripts/mutate.py` while the ledger half was being added.
    seen, broken = check(root)
    for problem in broken:
        print(problem)
    # The never-fires guard. A pattern that stopped matching — a rename of the
    # `docs/` directory, a regex edited wrong — would report zero problems and
    # read exactly like a clean tree. Every other check in this directory
    # carries one of these for the same reason.
    if seen == 0:
        print(
            "no `docs/*.md` or `ledger/*.md` citation was found anywhere in "
            "the source or the prose, which means this check is looking in "
            "the wrong place rather than that nothing cites anything"
        )
        return 1
    if broken:
        print(f"\n{len(broken)} broken of {seen} citations")
        return 1
    print(
        f"ok    {seen} `docs/*.md` and `ledger/*.md` citations in source and "
        f"prose, all openable; {len(NOT_A_FILE)} rostered as not files"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
