#!/usr/bin/env python3
"""Every `docs/*.md` a source file points at must be openable.

The companion to `check_cited_tests.py`, one level over. That one catches a
document naming a test that no longer exists; this one catches *code* naming a
document that no longer exists — a comment or, worse, an error message that
sends a reader to a file that is not there.

It was written after adding an error message that ends "See docs/ctes.md",
noticing that nothing would notice if the file were deleted, and writing that
down as a gap rather than closing it. Closing it is one loop.

WHY THIS IS NARROW, AND WHERE IT LOOKS

Source only — `.rs`, `.py`, `.go`, `.ts` — and never `docs/` or `ledger/`.

  * `docs/` cross-links are Markdown links, and `site/check/docs.py` already
    requires every relative link to resolve.
  * `ledger/` cites paths that were right on the day, which is what a dated
    record is for. An entry saying "moved to `docs/x.md`" is provenance even
    after `x.md` moves again, and rewriting it destroys the thing a ledger is.
  * `node_modules`, `dist`, `target` and other vendored or generated trees are
    skipped. Measured before this list was written: `playwright-core`'s type
    definitions alone cite `docs/user_data_dir.md` and
    `docs/chromium_browser_vs_google_chrome.md`, neither of which is ours and
    neither of which should exist here.

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

#: Where a citation counts. Documents and the ledger are covered or exempt for
#: the reasons in the module docstring.
SOURCE_SUFFIXES = (".rs", ".py", ".go", ".ts")

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
    "scripts/test_check_retired_claims.py": (
        "writes temporary trees containing `ledger/e.md` and `ledger/gone.md` "
        "and runs the retired-claims checker over them; the second is missing "
        "on purpose, to prove that checker refuses a retirement citing nothing"
    ),
    "scripts/test_check_cited_docs.py": (
        "writes temporary trees containing invented `docs/…` paths, for the same "
        "reason the cited-tests guard does"
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
        if relative.parts and relative.parts[0] in {"docs", "ledger"}:
            continue
        if relative.as_posix() in FIXTURES:
            continue
        found.append(path)
    return found


def check(root: Path) -> tuple[int, list[str]]:
    """Returns how many citations were seen, and what is broken."""
    seen = 0
    broken = []
    for path in source_files(root):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            # A source file this cannot read is not a citation it can check,
            # and refusing the whole run over one would make the guard the
            # thing that breaks rather than the thing that reports.
            continue
        for line_number, line in enumerate(text.splitlines(), start=1):
            for cited in CITATION.findall(line):
                seen += 1
                if not (root / cited).is_file():
                    broken.append(
                        f"{path.relative_to(root)}:{line_number} points at "
                        f"`{cited}`, which is not a file"
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
            "the source, which means this check is looking in the wrong place "
            "rather than that the code cites nothing"
        )
        return 1
    if broken:
        print(f"\n{len(broken)} broken of {seen} citations")
        return 1
    print(f"ok    {seen} `docs/*.md` and `ledger/*.md` citations, all openable")
    return 0


if __name__ == "__main__":
    sys.exit(main())
