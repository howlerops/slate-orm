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
  * `narrowed` &mdash; part of the claim has been answered and part has not,
    and rewriting the claim is not allowed. `by` must name **both**: what
    closed it, and what is left. Counting it `open` overstates the debt and
    reading it as written misleads, because the sentence describes a gap
    wider than the one that remains. Reversing nothing; the residual is the
    work, and when the residual closes the verdict becomes `closed`.
  * `moment` &mdash; a statement about one run, one commit or one moment,
    which cannot be "still true" because it was never a standing claim.
    "It does not prove CI is green", "the deployed job's steps have run
    exactly once each", "the panel no longer exists". There is nothing to
    do and nothing to reverse, so counting it as either work or a decision
    misreads it. `by` is not required: the entry's own date is the context.
  * `untriaged` &mdash; the default. Not yet read.

`moment` was added while triaging, because 677 caveats could not be sorted
into the first four without lying about roughly one in twelve of them. A caveat
that says a check had run once is not debt, not a decision, and not closed by
anything — it simply stopped being current, and `open` would have put it on a
backlog where it would be read as work forever.

`narrowed` was added on 2026-09-26, after the re-triage pass met three in one
day and the second one's entry said a fifth verdict "may become worth it":

  * "Nothing prevents the next formatting failure" — `scripts/check.sh` now
    runs `cargo fmt --all -- --check`, so what a person must remember shrank
    from a specific command run last to one script; that a person must
    remember did not change.
  * "Nothing is attributed between 369 and 491" — one of the four named
    components, the commit's conflict history, has since been measured at no
    measurable bytes; the other three are still unattributed.
  * "The demo and the docs site show no array" — `site/docs/features.html` has
    a whole section on arrays; the demo still hides `posts`, deliberately.

Each was left `open` because closing it would erase a true residual, and each
then read as more missing than is missing. Three is enough: the shape recurs
whenever a caveat names two things and one gets done, which in a repository
that writes caveats as sentences rather than as tickets is often.

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
#: A paragraph in the section, bold lead or not.
#:
#: **The bold lead is a convention, and it was not always the convention.**
#: `BULLET` above reads only paragraphs that open with `**`, which is how the
#: tracker was built and how every verdict in `docs/caveat-status.json` is
#: keyed. Measured on 2026-09-28: 1071 paragraphs in this repository's
#: `What this does not do` sections carry a bold lead and **379 do not**, and
#: all 379 were invisible. **129 entries had no caveat the tracker could see at
#: all** — the whole of 2026-09-13 and most of the 14th, before the lead firmed
#: up around the 20th. So "0 open" was a statement about 74% of the caveats,
#: and the missing quarter was the *oldest* quarter, which is the worst
#: possible skew: an old caveat is the one most likely to have been quietly
#: closed or quietly forgotten, and neither shows up in a number that cannot
#: see it.
#:
#: That is the never-fires shape wearing a headline. It is exactly what this
#: file's own docstring warns about one level up — a check that cannot see a
#: thing reports the same "clean" as a check that saw it and found nothing.
#:
#: The key still comes from the **bold text** where there is one, never from
#: the whole paragraph, because every verdict written so far is keyed that way
#: and keying on the paragraph would orphan all 1065 of them at once. Measured
#: before the change: 1065 keys before, all 1065 present after, 379 added, no
#: collisions.
PARAGRAPH = re.compile(r"\n[ \t]*\n")
#: The bold lead of a unit, when it has one.
#:
#: Applied with `.match`, never `.search`, and that is the whole of the
#: restriction — the `\A` a first draft carried was redundant beside `.match`
#: and a mutation removing it survived, which is the equivalent-mutation trap
#: `mutate.py` warns about rather than a missing test. `.search` is the real
#: hazard and is a real change: 126 plain paragraphs here carry emphasis
#: mid-sentence, and a searched lead would key them on that fragment — 26 of
#: them on the same six words, `Closed on 2026-09-15`, which collides.
LEAD = re.compile(r"\*\*(.+?)\*\*", re.S)
#: A unit that is *about* a caveat rather than being one, so not counted.
#:
#: Each of the four was met when whole paragraphs were first read, and each
#: read as a claim: 26 blockquoted `> **Closed on …**` notes recording that the
#: caveat above them was answered, 3 fenced blocks of captured process output,
#: one `*(Closed, …)*` parenthetical, and 47 struck-through paragraphs.
#:
#: The struck ones were already excluded before, but by accident rather than by
#: rule — `~~` opened the paragraph so the bold-lead pattern never matched. An
#: exclusion that works because something else did not fire is one that stops
#: working the moment that something else changes, which is exactly what
#: happened here.
ANNOTATION = re.compile(r"\A(?:~~|>|```|\*?\(Closed)")
#: A list marker at the start of a line, splitting a tight list into items.
#:
#: The unit is the *item*, not the block. 53 of this repository's 54 list
#: blocks hold more than one item, each with its own bold lead, so reading a
#: block as one caveat would key every item on the first one's opening words.
#: That is worse than missing them: a wrong key looks triaged.
#:
#: The same pattern recognises a block as a list, with `.match` on a stripped
#: block. A second constant doing that job was redundant and a mutation
#: loosening its whitespace survived, because this one was what decided. The
#: `[ \t]+` is load-bearing exactly once: it keeps `**Bold.**`, which has no
#: space after the first `*`, from reading as a `*` bullet.
ITEM = re.compile(r"^[ \t]*(?:[-*+]|\d+\.)[ \t]+", re.M)
#: A bullet whose lead opens a withdrawal rather than a limitation. Matched
#: on the first word so that the date and the reasoning after it are free
#: text, which is how the ledger writes them.
#:
#: No `~*` here any more, though there was until `ANNOTATION` below existed:
#: a struck withdrawal is `~~**Withdrawn.** …~~`, `ANNOTATION` drops it before
#: this runs, and a mutation removing the `~` survived because nothing reached
#: it. What is left is the prose form — a paragraph, or a bold lead, opening
#: with the word — which is how the ledger writes a withdrawal that was never
#: struck through.
WITHDRAWN = re.compile(r"^\s*Withdrawn\b", re.I)
VERDICTS = ("open", "closed", "narrowed", "deliberate", "moment", "untriaged")

#: What a `narrowed` verdict must say is *left*, beside the `by` that says what
#: closed.
#:
#: `ledger/2026-09-26-a-sixth-verdict-for-a-caveat-half-done.md` recorded the
#: hole this fills: "the residual lives in the `by` prose, so nothing can count
#: how much work the narrowed caveats represent, and nothing stops a `by` that
#: names what closed and forgets what is left." A prose `by` beginning
#: "closed: … Left: …" is a convention a reader can break silently; a field
#: cannot be forgotten, because `report` refuses a `narrowed` row without one.
RESIDUAL = "residual"

#: Two dates, because one meant two things.
#:
#: `checked` on an `open` or `narrowed` row says somebody read the caveat
#: *against the tree* and believes it is still true. That is what `--unread`
#: lists on.
#:
#: The 2026-09-26 reverse sweep then stamped `checked` on 316 `deliberate` rows
#: to record that their *verdict* had been re-read — a weaker and different
#: claim, about an entry's prose rather than about code, which
#: `ledger/2026-09-26-the-reverse-sweep-found-six.md` wrote down as a caveat the
#: same day. `reviewed` is that second claim, so the two cannot be mistaken for
#: each other and `--unread` keeps meaning one thing.
CHECKED, REVIEWED = "checked", "reviewed"
#: How much of a bullet keys its verdict. Long enough that two caveats in one
#: entry do not collide, short enough that fixing a typo later in the sentence
#: does not orphan the verdict.
KEY = 60


def key(claim: str) -> str:
    return " ".join(claim.split())[:KEY]


def units(body: str) -> list[str]:
    """The section's caveat-sized pieces: paragraphs, and a list's own items."""
    found = []
    for block in PARAGRAPH.split(body):
        stripped = block.strip()
        if not stripped:
            continue
        if ITEM.match(stripped):
            found.extend(part.strip() for part in ITEM.split(stripped) if part.strip())
        else:
            found.append(stripped)
    return found


def struck(root: Path = ROOT) -> set[str]:
    """`entry::key` for every caveat crossed out in a "does not do" section.

    A strike is how this ledger records that a caveat was answered *later*, by
    another entry. **Three** entries also keep the original bullet standing
    underneath — `2026-09-19-the-count-a-refusal-did-not-write.md` says so in
    as many words, because the estimate it got wrong is the useful part — so
    those sections hold the same claim twice, once struck and once not.

    Three of fifty-one, counted. The first version of this docstring called
    that "the convention", generalised from the one entry that explains itself;
    48 struck caveats have no live twin at all. The rule below is unaffected,
    because it fires on the pair and the pair is what it is for — but a wrong
    reason attached to a right rule is how the next person builds the wrong
    second rule. See `2026-09-29-a-convention-i-inferred-from-one-entry.md`.

    `ANNOTATION` drops the struck copy, which is right: a withdrawal is not a
    caveat. But it leaves the standing copy as the only one the tracker sees,
    and the standing copy is *history written in the present tense*. One of the
    three pairs in this repository had been triaged from it and was recorded
    `open` two months after the arm it names was pinned by three tests.

    So the keys are collected rather than discarded, and `report` refuses a
    live verdict on one. This does not read the strike as a verdict — a struck
    caveat closed by a later entry still needs a `by` naming it, which is the
    whole point of the field.
    """
    found: set[str] = set()
    ledger = root / "ledger"
    if not ledger.is_dir():
        return found
    for path in sorted(ledger.glob("*.md")):
        if path.name in SKIP:
            continue
        section = SECTION.search(path.read_text(encoding="utf-8", errors="replace"))
        if not section:
            continue
        for unit in units(section.group(1)):
            if not unit.startswith("~~"):
                continue
            # The `~~` and any bold lead inside it, so the key matches what the
            # standing copy produces. A struck bullet is `~~**Lead.** rest~~`.
            inner = unit.lstrip("~").strip()
            lead = LEAD.match(inner)
            found.add(f"{path.name}::{key(lead.group(1) if lead else inner)}")
    return found


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
        for unit in units(section.group(1)):
            if ANNOTATION.match(unit):
                continue
            lead = LEAD.match(unit)
            # The bold text where there is one, so every key written before the
            # paragraph widening still resolves; the whole unit otherwise,
            # which is the only text there is.
            #
            # No marker strip here, though a first draft had one: `ITEM.split`
            # consumes every marker, including the leading one, and a block
            # that opens with a marker is by definition the block `units` sent
            # down that path. A mutation removing the strip survived, which is
            # what dead code looks like from the outside.
            claim = lead.group(1) if lead else unit
            if WITHDRAWN.match(claim.strip()):
                continue
            found.append({"entry": path.name, "claim": " ".join(claim.split())})
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
    crossed = struck(root)
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
        row = status.get(k, {})
        if verdict in ("closed", "narrowed", "deliberate") and not row.get("by"):
            problems.append(
                f"{c['entry']}: `{key(c['claim'])}` is {verdict} and names nothing "
                f"that closed or decided it"
            )
        if verdict == "narrowed" and not row.get(RESIDUAL):
            problems.append(
                f"{c['entry']}: `{key(c['claim'])}` is narrowed and has no "
                f"`{RESIDUAL}`. `by` says what closed; a narrowed caveat must "
                f"also say what is left, in a field, so it can be counted."
            )
        # A settled verdict carries `reviewed`, never `checked`: `checked` is a
        # claim about the tree and `--unread` reads it, so a `deliberate` row
        # wearing one is a stamp nothing will ever look at again pretending to
        # be one that will.
        if verdict in ("closed", "deliberate", "moment") and row.get(CHECKED):
            problems.append(
                f"{c['entry']}: `{key(c['claim'])}` is {verdict} and carries "
                f"`{CHECKED}`, which means read against the tree. A settled "
                f"verdict re-read for correctness carries `{REVIEWED}`."
            )
        # And the other direction, which was written down and not enforced.
        # `narrowed` is a live verdict — something is left, and `--unread`
        # exists to make somebody look at it again — so its date belongs in
        # `checked`. Five rows carried `reviewed` instead and were therefore
        # invisible to `--unread` **permanently**: not "stale and listed", but
        # never listed at all, which is the quieter of the two failures and
        # the one a staleness report cannot show you.
        if verdict == "narrowed" and row.get(REVIEWED):
            problems.append(
                f"{c['entry']}: `{key(c['claim'])}` is narrowed and carries "
                f"`{REVIEWED}`, which is the settled-verdict stamp. A narrowed "
                f"caveat still has a residual to re-read, so its date is "
                f"`{CHECKED}` — `--unread` reads that one and nothing else."
            )
        # A claim that also stands struck through in the same section was
        # answered by a later entry; the standing copy is the original text,
        # kept on purpose. Reading it as current is how one of the three pairs
        # here was triaged `open` long after it was closed — and the tracker
        # could not have shown that, because the strike is the only difference
        # and `ANNOTATION` had already dropped it.
        if k in crossed and verdict in ("open", "untriaged", "moment"):
            problems.append(
                f"{c['entry']}: `{key(c['claim'])}` is {verdict}, and the same "
                f"claim also stands struck through in that section. The strike "
                f"says a later entry answered it and the standing copy is the "
                f"original text kept for the record. Read the strike, then "
                f"record `closed` or `narrowed` with a `by` naming the entry."
            )
        counts[verdict] += 1
    orphans = sorted(set(status) - seen)
    return counts, problems, orphans


def unread(days: int, root: Path = ROOT, today: str | None = None) -> list[str]:
    """Every `open` or `narrowed` caveat not re-read in `days`.

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
        # `narrowed` too: part of it is still true, so it is still work and
        # still wants re-reading. Only the part that closed is settled.
        if entry.get("verdict") not in ("open", "narrowed"):
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


def residuals(root: Path = ROOT) -> list[str]:
    """What each `narrowed` caveat still owes, as `entry: residual` lines.

    The reason `residual` is a field and not prose in `by`: this listing. A
    narrowed caveat is partly work, and until the residual was a field the only
    way to see how much was to read eighteen `by` strings and hope each
    happened to say.
    """
    status = load(root)
    out = []
    for c in caveats(root):
        row = status.get(f"{c['entry']}::{key(c['claim'])}", {})
        if row.get("verdict") == "narrowed":
            out.append(f"{c['entry']}: {row.get(RESIDUAL, '')}")
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
        print(f"{len(lines)} open or narrowed and not re-read in {days} days")
        return 0
    if argv and argv[0] == "--residual":
        lines = residuals(root)
        for line in lines:
            print(line)
        print(f"{len(lines)} narrowed caveats, each with a residual")
        return 0
    if argv:
        want = argv[0].removeprefix("--")
        if want not in VERDICTS:
            print(
                f"usage: caveats.py [--{' | --'.join(VERDICTS)} "
                "| --unread [days] | --residual]"
            )
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
        f"{total} caveats: {counts['open']} open, {counts['narrowed']} narrowed, "
        f"{counts['closed']} closed, {counts['deliberate']} deliberate, "
        f"{counts['untriaged']} untriaged"
    )
    return 1 if problems or orphans else 0


if __name__ == "__main__":
    sys.exit(main())
