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
  * `moment` &mdash; a statement about one run, one commit or one moment,
    which cannot be "still true" because it was never a standing claim.
    "It does not prove CI is green", "the deployed job's steps have run
    exactly once each", "the panel no longer exists". There is nothing to
    do and nothing to reverse, so counting it as either work or a decision
    misreads it. `by` is not required: the entry's own date is the context.
  * `untriaged` &mdash; the default. Not yet read.

The fifth verdict was added while triaging, because 677 caveats could not be
sorted into the first four without lying about roughly one in twelve of them.
A caveat that says a check had run once is not debt, not a decision, and not
closed by anything — it simply stopped being current, and `open` would have
put it on a backlog where it would be read as work forever.

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
#: A caveat leads its *paragraph*: a blank line, then `**`. Emphasis that
#: merely happens to start a wrapped line is not a caveat, and counting it
#: as one inflated the first run — `**12**` in the middle of a sentence in
#: `2026-09-21-the-demo-ui-is-a-subset-on-purpose.md` was read as a bullet.
#: A struck-through paragraph is a *withdrawn* caveat and is already
#: excluded, because `~~` opens it rather than `**`.
#:
#: A withdrawal can also be written as prose — `**Withdrawn, 2026-09-14.**`
#: followed by what was wrong — and `WITHDRAWN` drops those. Found by the
#: triage: it was the one caveat of 772 that could be given no verdict,
#: because it is not a caveat. `open` would have made it work that was
#: retracted eleven days earlier, and `moment` would have called a
#: correction a passing observation.
BULLET = re.compile(r"(?:\A|\n[ \t]*\n)[ \t]*\*\*(.+?)\*\*", re.S)
#: A bullet whose lead opens a withdrawal rather than a limitation. Matched
#: on the first word so that the date and the reasoning after it are free
#: text, which is how the ledger writes them.
WITHDRAWN = re.compile(r"^~*\s*Withdrawn\b", re.I)
VERDICTS = ("open", "closed", "deliberate", "moment", "untriaged")
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
        # Prefixed with a blank line because `SECTION`'s `\s*$` eats one of the
        # two newlines after the heading, leaving the section's *first* bullet
        # without the paragraph boundary `BULLET` requires. Found by two
        # orphaned verdicts when the pattern was tightened: normalising the
        # boundary here is safer than loosening the pattern, which is what let
        # mid-paragraph emphasis in as a caveat in the first place.
        for bullet in BULLET.findall("\n\n" + section.group(1)):
            if WITHDRAWN.match(bullet.strip()):
                continue
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


def unread(days: int, root: Path = ROOT, today: str | None = None) -> list[str]:
    """Every `open` caveat whose verdict has not been re-read in `days`.

    # Why a date rather than a check

    `2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md` recorded
    "Nothing re-triages": a caveat stays `open` whether or not the thing it
    describes still exists, and the orphan mechanism catches a *reworded*
    bullet rather than a claim that has quietly become false.

    Nothing here can decide whether a claim is still true — that is reading
    code and it is what a person does. What it can do is stop the reading
    being invisible. A verdict carries an optional `checked` date; this lists
    the open ones nobody has stamped lately, so "re-triage" becomes a thing
    with a worklist and a finish line instead of an intention.

    The stamp is a claim about a person's attention, not a proof, and
    `2026-09-25-the-open-caveats-nobody-re-reads.md` measures what that
    attention is worth: fourteen open caveats read against the tree, two
    stale, and both stale ones were caveats whose own text named what they
    were waiting for. A list is right roughly six times in seven, and the
    "waiting for" shape is where the seventh is.

    `checked` is absent on every verdict written before this existed, so the
    first run lists all of them. That is correct and it is also why this is
    reported rather than enforced: turning it red today would mean stamping
    285 caveats to get a green build, which is the pressure that produces a
    rubber stamp.
    """
    from datetime import date, timedelta

    now = date.fromisoformat(today) if today else date.today()
    cutoff = now - timedelta(days=days)
    status = load(root)
    out = []
    for c in caveats(root):
        k = f"{c['entry']}::{key(c['claim'])}"
        entry = status.get(k, {})
        if entry.get("verdict") != "open":
            continue
        stamp = entry.get("checked")
        if stamp:
            try:
                if date.fromisoformat(stamp) >= cutoff:
                    continue
            except ValueError:
                out.append(f"{c['entry']}: {c['claim']}  [unreadable checked: {stamp!r}]")
                continue
        out.append(f"{c['entry']}: {c['claim']}")
    return out


def listing(verdict: str, root: Path = ROOT) -> list[str]:
    """Every caveat with `verdict`, as `entry: claim` lines, entry order.

    The counts alone could not answer the question this file's header poses
    — "what is left to do" — because a number is not a list. Reading the
    JSON by hand was the workaround, which is the same workaround as
    reading 196 entries, only shorter.
    """
    status = load(root)
    out = []
    for c in caveats(root):
        k = f"{c['entry']}::{key(c['claim'])}"
        if status.get(k, {}).get("verdict", "untriaged") == verdict:
            out.append(f"{c['entry']}: {c['claim']}")
    return out


def main(root: Path = ROOT) -> int:
    argv = sys.argv[1:]
    if argv and argv[0] == "--unread":
        # Days, because the useful question is "what has nobody looked at
        # lately" and the useful answer changes with how long ago "lately" is.
        days = int(argv[1]) if len(argv) > 1 else 30
        lines = unread(days, root)
        for line in lines:
            print(line)
        print(f"{len(lines)} open and not re-read in {days} days")
        return 0
    if argv:
        want = argv[0].removeprefix("--")
        if want not in VERDICTS:
            print(f"usage: caveats.py [--{' | --'.join(VERDICTS)} | --unread [days]]")
            return 2
        lines = listing(want, root)
        for line in lines:
            print(line)
        print(f"{len(lines)} {want}")
        return 0
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
