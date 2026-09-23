#!/usr/bin/env python3
"""A claim this repository has withdrawn is not stated anywhere as current.

The other provenance guards check that a citation *resolves* — that a named
document or test exists, that a measurement kept its build line. None of them
checks the thing that actually went wrong three times in one session: a claim
was corrected in one place and left standing in the others.

#298 re-measured what the example suite costs, corrected `CLAUDE.md`, and wrote
the entry. It left the same false claim in the doc comment on
`assert_wrote_something`, and left the mutation record that carries it in
machine-readable form unlinked to its correction. `run_examples.sh` described linking eight
binaries in two of its comments; there are nine, and there had been since one
was added. NOT_A_CLAIM — that sentence quotes the retired wording deliberately,
as the paragraph below quotes the other one.
Each was found by reading, afterwards, on the third look — and the one in the
source had been written by the same session that was at that moment reasoning
about stale documentation.

`CLAUDE.md` says stale documentation is worse than none, because it is read as
current. A withdrawn claim that survives somewhere is the sharpest form of
that: the correction exists, is findable, and is not what the reader hits.

HOW IT WORKS, AND WHY THAT SHAPE

`scripts/retired_claims.json` lists phrases this repository has withdrawn, each
naming the ledger entry that withdrew it. This reads the live tree and fails on
any occurrence that is not marked.

The registry is the whole mechanism, and its cost is honest: **a claim nobody
declares is a claim this cannot see.** Detecting "these two sentences assert
the same thing" is the undecidable problem again, and the approximations are
worse than nothing — the same reasoning `mutate.py` gives for not detecting
equivalent mutations. What a declaration buys is not the first catch, which is
the read that prompted it; it is every later one. A phrase deleted today and
reintroduced in a new file next month is caught, and that is the failure this
class actually takes: not writing the claim twice at once, but writing it again
after forgetting it was retired.

**Text is normalized before matching, and this is the part that matters.** The
claim that prompted this was written

    /// reach — and on this container the runner that *does* reach it cannot be
    /// built, because nine debug example binaries exhaust the disk.

NOT_A_CLAIM — the block above quotes the withdrawn comment verbatim, which is
the point of the example.

A line-by-line grep for "cannot be built" does not find that, because the
sentence wraps and the claim wraps with it. Comment markers, Markdown emphasis
and line breaks are stripped, and runs of whitespace collapse to one space, so
a phrase is matched against the sentence a reader sees rather than the lines a
file happens to hold. `check_cost_prose.py` reached the same conclusion from
the same evidence: reading line by line saw neither half of its claim.

TWO WAYS TO KEEP A RETIRED PHRASE, both deliberate:

  * `~~strikethrough~~` around it, preferred in prose that also states the
    correction, because it leaves the sentence legible and the correction
    beside it. This is the convention `ledger/README.md` already asks for.
  * `NOT_A_CLAIM` anywhere in the same paragraph, for the cases strikethrough
    cannot serve — a code comment, a test fixture, a quotation of the old
    wording inside the very entry that withdraws it.

WHERE IT LOOKS

The live tree: `crates/`, `clients/`, `docs/`, `examples/`, `scripts/`,
`site/`, and the two Markdown files at the root. Not `ledger/` — an entry
states what was believed on its date, and a dated record that gets rewritten
when the belief changes is not a record. That exclusion is the reason the
registry can name a ledger entry as the authority for a retirement: the entry
keeps the old wording, legibly, forever.

Two files are excluded by name rather than by a pattern, following
`check_cited_docs.py`'s reasoning that an exclusion with a name is one somebody
has to justify:

  * `scripts/retired_claims.json`, which by construction contains every
    retired phrase and would otherwise fail on all of them.
  * `scripts/test_check_retired_claims.py`, whose fixtures are written to be
    found. A fixture is not a claim.

This file is not excluded. The phrases quoted in this docstring carry
`NOT_A_CLAIM` markers, because a guard that has to exempt itself is a guard
with a blind spot exactly where its author was last thinking.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = "scripts/retired_claims.json"

# The live tree. `ledger/` is absent on purpose — see the docstring.
ROOTS = ("crates", "clients", "docs", "examples", "scripts", "site")
ROOT_FILES = ("README.md", "CLAUDE.md")
SUFFIXES = {".rs", ".py", ".go", ".ts", ".tsx", ".js", ".sh", ".md", ".toml"}
SKIP_DIRS = {
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".git",
    ".venv",
    "vendor",
    "gen",
}
EXCLUDED = {REGISTRY, "scripts/test_check_retired_claims.py"}

# Leading comment and markup noise, stripped so a sentence reads as a sentence.
LEADER = re.compile(r"^\s*(///|//!|//|#+|\*/|/\*+|\*|<!--|-->|--)\s?")
# Markdown emphasis and inline code, which this repository wraps claims in.
EMPHASIS = re.compile(r"[*_`]+")
MARKER = "NOT_A_CLAIM"
STRUCK = re.compile(r"~~.+?~~", re.DOTALL)


def normalize(text: str) -> tuple[str, list[int]]:
    """Return the text as one whitespace-collapsed line, plus a line number
    for each character in it.

    The line numbers are what lets a match report where it is. Without them a
    failure says only that a phrase is somewhere in the file, which for a file
    the size of `check_handlers.py` is not a finding anybody can act on.
    """
    out: list[str] = []
    lines: list[int] = []
    for number, raw in enumerate(text.splitlines(), start=1):
        stripped = EMPHASIS.sub("", LEADER.sub("", raw))
        for piece in stripped.split():
            if out:
                out.append(" ")
                lines.append(number)
            out.append(piece)
            lines.extend([number] * len(piece))
    return "".join(out), lines


def source_lines(text: str) -> list[str]:
    return text.splitlines()


def exempt(flat: str, start: int, end: int, raw_lines: list[str], lines: list[int]) -> bool:
    """Is this occurrence marked as deliberate?

    `NOT_A_CLAIM` is looked for in the source lines the match spans, widened by
    two either side. A code comment puts the marker on its own line, so a
    marker required on the matching line itself would never be found where it
    is actually written.
    """
    for struck in STRUCK.finditer(flat):
        if struck.start() <= start and end <= struck.end():
            return True
    if not lines:
        return False
    first = lines[start] if start < len(lines) else lines[-1]
    last = lines[min(end, len(lines) - 1)]
    lo = max(0, first - 3)
    hi = min(len(raw_lines), last + 2)
    return any(MARKER in raw_lines[i] for i in range(lo, hi))


def files(root: Path) -> list[Path]:
    found: list[Path] = []
    for name in ROOT_FILES:
        path = root / name
        if path.is_file():
            found.append(path)
    for top in ROOTS:
        base = root / top
        if not base.is_dir():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in SUFFIXES:
                continue
            if any(part in SKIP_DIRS for part in path.relative_to(root).parts):
                continue
            found.append(path)
    return [p for p in found if str(p.relative_to(root)) not in EXCLUDED]


def check(root: Path = ROOT) -> tuple[int, int, list[str]]:
    """Return (phrases, files scanned, problems)."""
    problems: list[str] = []
    registry = root / REGISTRY
    if not registry.is_file():
        return 0, 0, [f"{REGISTRY} is missing, so nothing is being checked"]
    try:
        retired = json.loads(registry.read_text(encoding="utf-8"))["retired"]
    except (json.JSONDecodeError, KeyError, OSError) as exc:
        return 0, 0, [f"{REGISTRY} cannot be read as a registry: {exc}"]

    # Registry hygiene. A phrase of one or two common words matches prose that
    # means something else entirely, and the guard's whole value is that a
    # failure is worth reading. Refusing it here is cheaper than a reviewer
    # learning to ignore this check.
    for entry in retired:
        phrase = entry.get("phrase", "")
        if len(phrase.split()) < 3:
            problems.append(
                f"{REGISTRY}: `{phrase}` is fewer than three words, which will "
                f"match prose that is not this claim"
            )
        cited = entry.get("retired_by", "")
        if not cited or not (root / cited).is_file():
            problems.append(
                f"{REGISTRY}: `{phrase}` names `{cited}`, which is not a file"
            )

    scanned = 0
    for path in files(root):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            # Matching `check_cited_docs.py`: a file this cannot read is not a
            # claim it can check, and refusing the run over one would make the
            # guard the thing that breaks rather than the thing that reports.
            continue
        scanned += 1
        flat, lines = normalize(text)
        lowered = flat.lower()
        raw = source_lines(text)
        for entry in retired:
            phrase = entry.get("phrase", "")
            if not phrase:
                continue
            needle = " ".join(EMPHASIS.sub("", phrase).split()).lower()
            start = lowered.find(needle)
            while start != -1:
                end = start + len(needle)
                if not exempt(flat, start, end, raw, lines):
                    where = lines[start] if start < len(lines) else 0
                    problems.append(
                        f"{path.relative_to(root)}:{where} states a retired "
                        f"claim — `{phrase}` — withdrawn by "
                        f"{entry.get('retired_by', '?')}. "
                        f"{entry.get('why', '')}".rstrip()
                    )
                start = lowered.find(needle, start + 1)
    return len(retired), scanned, problems


def main(root: Path = ROOT) -> int:
    phrases, scanned, problems = check(root)
    for problem in problems:
        print(problem)
    # The never-fires guard every check in this directory carries. An empty
    # registry, or a `ROOTS` list that stopped matching after a directory was
    # renamed, reports zero problems and reads exactly like a clean tree.
    if phrases == 0:
        print(
            "the retired-claims registry is empty, which means this check is "
            "asserting nothing rather than that no claim has been withdrawn"
        )
        return 1
    if scanned == 0:
        print(
            "no file was scanned, which means this check is looking in the "
            "wrong place rather than that the repository has no prose"
        )
        return 1
    if problems:
        print(f"\n{len(problems)} stale of {phrases} retired claims")
        return 1
    print(f"ok    {phrases} retired claims, none restated in {scanned} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
