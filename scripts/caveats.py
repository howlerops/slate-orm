#!/usr/bin/env python3
"""Every caveat this repository's ledger records, and what became of it.

`ledger/TEMPLATE.md` ends each entry with **What this does not do**, and the
discipline has held: 196 entries carry 762 such bullets. That is the honest
cost of writing limits down instead of leaving them implicit — and it created a
second problem, which this exists for.

**Nothing tracked whether a caveat was still true.** A bullet written on the
14th saying "no client SDK has a window surface" was closed on the 22nd, and
the entry still says it, correctly, because an entry states what was believed
on its date and is never rewritten. So the ledger accumulates limits with no
way to ask which ones still hold. Asked "what is left to do", the only honest
answer was to read 196 files.

This extracts the bullets and joins them to `docs/caveat-status.json`, which
records a verdict per caveat. The verdicts are:

  * `open` &mdash; still true, still work. The thing to do.
  * `closed` &mdash; done since, and `by` names the entry or commit that did it.
  * `deliberate` &mdash; a decision with reasoning written down, not a backlog
    item. `by` names where the reasoning lives. Reversing it is a design
    conversation, not a chore, and counting it as debt misreads the ledger.
  * `untriaged` &mdash; the default. Not yet read.

WHY A SEPARATE FILE RATHER THAN EDITING THE ENTRIES

Because `ledger/README.md` forbids the alternative, for a good reason: an entry
is dated and append-only, and one that gets rewritten when the world changes is
not a record. The status of a claim is not the claim. So the claims stay where
they were written and the verdicts live beside them, keyed by entry filename
and the first words of the bullet.

**The key is the bullet's opening text, which means editing a bullet orphans
its verdict.** That is deliberate: a reworded caveat is a different claim and
should be read again. An orphaned verdict is reported rather than dropped, so
the rewording surfaces instead of silently reverting the bullet to untriaged.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEDGER = ROOT / "ledger"
STATUS = ROOT / "docs" / "caveat-status.json"
SKIP = {"README.md", "TEMPLATE.md"}
SECTION = re.compile(r"^## What this does not do\s*$(.*?)(?=^## |\Z)", re.M | re.S)
BULLET = re.compile(r"^\*\*(.+?)\*\*", re.M | re.S)
VERDICTS = ("open", "closed", "deliberate", "untriaged")
#: How much of a bullet keys its verdict. Long enough that two caveats in one
#: entry do not collide, short enough that fixing a typo later in the sentence
#: does not orphan the verdict.
KEY = 60


def key(claim: str) -> str:
    return " ".join(claim.split())[:KEY]


def caveats(root: Path = ROOT) -> list[dict[str, str]]:
    found: list[dict[str, str]] = []
    ledger = root / "ledger"
    if not ledger.is_dir():
        return found
    for path in sorted(ledger.glob("*.md")):
        if path.name in SKIP:
            continue
        section = SECTION.search(path.read_text(encoding="utf-8", errors="replace"))
        if not section:
            continue
        for bullet in BULLET.findall(section.group(1)):
            found.append({"entry": path.name, "claim": " ".join(bullet.split())})
    return found


def load(root: Path = ROOT) -> dict[str, dict[str, str]]:
    path = root / "docs" / "caveat-status.json"
    if not path.is_file():
        return {}
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return {}
    return {f"{v['entry']}::{v['key']}": v for v in raw.get("verdicts", [])}


def report(root: Path = ROOT) -> tuple[dict[str, int], list[str], list[str]]:
    """Return (counts by verdict, problems, orphaned verdict keys)."""
    found = caveats(root)
    status = load(root)
    counts = dict.fromkeys(VERDICTS, 0)
    problems: list[str] = []
    seen: set[str] = set()
    for c in found:
        k = f"{c['entry']}::{key(c['claim'])}"
        seen.add(k)
        verdict = status.get(k, {}).get("verdict", "untriaged")
        if verdict not in VERDICTS:
            problems.append(f"{c['entry']}: unknown verdict {verdict!r}")
            verdict = "untriaged"
        if verdict in ("closed", "deliberate") and not status.get(k, {}).get("by"):
            problems.append(
                f"{c['entry']}: `{key(c['claim'])}` is {verdict} and names nothing "
                f"that closed or decided it"
            )
        counts[verdict] += 1
    orphans = sorted(set(status) - seen)
    return counts, problems, orphans


def main(root: Path = ROOT) -> int:
    counts, problems, orphans = report(root)
    total = sum(counts.values())
    for problem in problems:
        print(problem)
    for orphan in orphans:
        print(f"orphaned verdict, its bullet was reworded or removed: {orphan}")
    # The never-fires guard every check in this directory carries. A `ledger/`
    # that moved, or a section heading that was renamed in the template, finds
    # nothing and reads exactly like a repository with no caveats recorded.
    if total == 0:
        print(
            "no caveat was found in any ledger entry, which means this is "
            "looking in the wrong place rather than that none was ever written"
        )
        return 1
    print(
        f"{total} caveats: {counts['open']} open, {counts['closed']} closed, "
        f"{counts['deliberate']} deliberate, {counts['untriaged']} untriaged"
    )
    return 1 if problems or orphans else 0


if __name__ == "__main__":
    sys.exit(main())
