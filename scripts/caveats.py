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
#: `checked` says somebody read the caveat *against the tree* — or against the
#: world outside it, for the handful whose subject is there — and believes it
#: is still true. `reviewed` is the weaker claim: the verdict's own reasoning
#: was re-read, which is prose against prose. The 2026-09-26 reverse sweep
#: stamped 316 `deliberate` rows that way, and
#: `ledger/2026-09-26-the-reverse-sweep-found-six.md` recorded the difference
#: as a caveat the same day.
#:
#: **`checked` was refused on a `deliberate` row until 2026-09-30**, on the
#: grounds that `--unread` reads only `open` and `narrowed`, so the stamp would
#: be "one nothing will ever look at again pretending to be one that will".
#: The reasoning was sound and the conclusion was backwards: five campaigns
#: had by then read 246 `deliberate` verdicts against the tree with no way to
#: say so, so each new pass re-drew blind and none could be pooled with
#: another. The fix is to make something look at it — `--unchecked` — rather
#: than to forbid the stamp. See
#: `ledger/2026-09-30-a-sample-that-pools-with-the-next-one.md`.
CHECKED, REVIEWED = "checked", "reviewed"

#: Which draw a `checked` stamp came out of.
#:
#: `ledger/2026-09-30-a-sample-that-pools-with-the-next-one.md` recorded the
#: hole this fills, and it is the one that makes the frame worth having:
#:
#:   > **Nothing checks that a draw actually came from `--unchecked`.** The
#:   > next pass could sample the whole bucket again and stamp what it read;
#:   > the count would move by the same amount while pooling nothing.
#:
#: A stamp with no draw behind it is indistinguishable from one with, and the
#: two mean different things: rows drawn from the frame never repeat, rows
#: drawn from the bucket repeat at whatever rate the bucket's size implies. So
#: the stamp names the draw, the draw is a file in `ledger/draws/`, and
#: `scripts/check_draws.py` reproduces it from its seed. What that buys is
#: not honesty — a determined hand can still write both halves — but that the
#: *accidental* version, a pass that samples the bucket because it forgot the
#: frame exists, cannot produce a record that reproduces.
DRAW = "draw"

#: Where a draw records itself.
#:
#: `ledger/`, not `docs/`, and not inside `caveat-status.json`. A draw is a
#: dated event, which is what the ledger is for; and a record living inside
#: the file it is evidence *about* could be written by the same edit that
#: writes the stamps, which is the loop this exists to break.
DRAWS = "ledger/draws"
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


#: A ledger entry named inside a strike, with or without its `ledger/` prefix.
#:
#: A strike usually says who answered the caveat — *"Closed by
#: `2026-09-20-the-child-that-made-the-arm-reachable.md`"* — and that name is
#: the one thing about a strike that is machine-readable. 21 of the 51 strikes
#: carry one; the rest are prose, and prose is not checked.
CREDITED = re.compile(r"(?:ledger/)?(\d{4}-\d{2}-\d{2}-[a-z0-9-]+\.md)")


def struck(root: Path = ROOT) -> dict[str, set[str]]:
    """`entry::key` for every caveat crossed out in a "does not do" section,
    and which entries that strike credits.

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

    **And the `by` has to name the entry the strike names.** The first version
    of this rule demanded a settled verdict and a `by` and read neither, which
    its own entry recorded as a gap:

    > It does not check that the `by` names the entry the strike names. The
    > rule demands a settled verdict and a `by`; it does not read the strike's
    > own text to see which entry it credits. A `closed` row citing the wrong
    > entry passes.

    Two of the three pairs were doing exactly that. Both said in prose what had
    closed them — *"a live-row case for the remaining generated decoders"* —
    which is a description of the work and not a pointer to where it is written
    down, so a reader of the tracker had no way back to the entry that did it.
    That is the failure the `by` field exists to prevent, one layer in from the
    one this rule was built for.

    So each key carries the set of entries its strike credits, and `report`
    checks the `by` against it. A strike crediting nobody is prose and yields
    an empty set, which the rule reads as nothing to check — the alternative is
    demanding a citation from every strike, which is a rule about how to write
    an entry rather than a check on the tracker.
    """
    found: dict[str, set[str]] = {}
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
            at = f"{path.name}::{key(lead.group(1) if lead else inner)}"
            # An entry that credits itself is the strike naming its own file in
            # passing, not a pointer elsewhere, so it cannot be what a `by`
            # should cite.
            credited = set(CREDITED.findall(unit)) - {path.name}
            found.setdefault(at, set()).update(credited)
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
        # `closed` and `moment` still carry `reviewed` and never `checked`: one
        # is answered and the other described a moment, so neither has a claim
        # about the current tree left to re-read. `deliberate` is no longer in
        # this list — it is a live assertion about how things are, `--unchecked`
        # reads its `checked` date, and 246 reads had nowhere to record
        # themselves while it was.
        # A read against the tree must say which draw it came out of. Without
        # it the stamp cannot be told from one a pass wrote after sampling the
        # whole bucket, and those two mean different things — the first never
        # repeats a row and the second repeats at the bucket's rate. The file
        # the name points at is checked by `scripts/check_draws.py`; this only
        # refuses the stamp with nothing behind it at all.
        if verdict == "deliberate" and row.get(CHECKED) and not row.get(DRAW):
            problems.append(
                f"{c['entry']}: `{key(c['claim'])}` is deliberate and carries "
                f"`{CHECKED}` with no `{DRAW}`. A read against the tree names "
                f"the draw it came out of, so a pass that sampled the whole "
                f"bucket cannot be mistaken for one that drew from the frame."
            )
        if verdict in ("closed", "moment") and row.get(CHECKED):
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
        # And the `by` must name the entry the strike credits. "A live-row case
        # for the remaining decoders" describes the work; it does not say where
        # it is written down, so a reader of the tracker cannot get back to it —
        # which is what `by` is for. Only checked when the strike names an
        # entry at all: a strike that credits nobody is prose.
        wanted = crossed.get(k, set())
        if wanted and verdict in ("closed", "narrowed"):
            says = row.get("by") or ""
            if not any(entry in says for entry in wanted):
                problems.append(
                    f"{c['entry']}: `{key(c['claim'])}` is {verdict}, and the "
                    f"strike beside it credits "
                    f"{', '.join(sorted(wanted))} — which its `by` does not "
                    f"name. Describing the work is not citing it: a reader of "
                    f"the tracker has no way back to the entry that did it."
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


def unchecked(days: int | None, root: Path = ROOT, today: str | None = None) -> list[str]:
    """Every `deliberate` caveat nobody has read against the tree, or not lately.

    # Why this exists, and why it is not `--unread`

    `--unread` is a worklist over 146 `open` and `narrowed` caveats: things
    known to be undone, re-read so the list does not rot. This is a worklist
    over 1038 `deliberate` ones, which is a different problem. A `deliberate`
    verdict says a caveat describes a choice rather than a gap — and five
    campaigns between 2026-09-28 and 2026-09-30 read 246 of them and found
    seven false, all of them true when written and overtaken later. That is a
    small rate over a large bucket: tens of wrong verdicts, not hundreds, and
    no way to find them but reading.

    Those five passes could not be pooled. None of the first four recorded a
    seed or a read set, and `checked` was refused on a `deliberate` row, so a
    read against the tree could only be stamped `reviewed` — which the
    2026-09-26 reverse sweep had already put on 316 rows to mean something
    weaker. Every pass therefore re-drew from the whole bucket, re-reading
    rows at random and unable to say how much was new.

    Drawing from *this* list instead makes them pool without anybody
    recording anything: a row read today leaves the frame, the next pass draws
    from what is left, and the union of all passes is `1038 - len(this)`.

    `days` bounds how long a read stays good. Passing `None` means forever —
    the right default for a bucket this size, where the first question is
    "what has nobody ever read" and re-reading on a timer would mean 1038 reads
    a period. A number is there for when the frame empties.

    # It emptied

    On 2026-09-30 the whole frame — 1016 rows by then — was drawn in one draw
    and read. `--unchecked` with no argument now lists nothing, and will list
    a `deliberate` verdict written after that day and not yet read.
    **That is the state this was built to reach and it is also the state in
    which it says least**: a worklist of zero and a worklist nobody looks at
    print the same thing. `--unchecked <days>` is the question that replaces
    it, and `main`'s summary line prints the two numbers side by side —
    `N deliberate (M read against the tree)` — so a bucket that grows shows
    even when the frame is silent. See
    `ledger/2026-09-30-the-whole-deliberate-frame-read-in-one-pass.md`, which
    also records why that pass's 2-of-1016 must not be pooled with the
    campaigns' 7 of 246.
    """
    from datetime import date, timedelta

    cutoff = None
    if days is not None:
        now = date.fromisoformat(today) if today else date.today()
        cutoff = now - timedelta(days=days)
    status = load(root)
    out = []
    for c in caveats(root):
        entry = status.get(f"{c['entry']}::{key(c['claim'])}", {})
        if entry.get("verdict") != "deliberate":
            continue
        stamp = entry.get(CHECKED)
        if stamp:
            if cutoff is None:
                continue
            try:
                if date.fromisoformat(stamp) >= cutoff:
                    continue
            except ValueError:
                out.append(f"{c['entry']}: {c['claim']}  [unreadable {CHECKED}: {stamp!r}]")
                continue
        out.append(f"{c['entry']}: {c['claim']}")
    return out


def frame(root: Path = ROOT) -> list[str]:
    """The keys `--unchecked` lists, in the order `caveats()` yields them.

    A deterministic order is the whole of what makes a draw reproducible: the
    seed picks *indices*, and an index means nothing without the order it
    indexes into. `caveats()` walks `sorted(ledger.glob("*.md"))` and reads
    each section top to bottom, so the order is the tree's and not the
    filesystem's.
    """
    status = load(root)
    out = []
    for c in caveats(root):
        k = f"{c['entry']}::{key(c['claim'])}"
        row = status.get(k, {})
        if row.get("verdict") == "deliberate" and not row.get(CHECKED):
            out.append(k)
    return out


def draw(count: int, seed: int, root: Path = ROOT, today: str | None = None) -> dict:
    """Draw `count` rows from the unchecked frame, as a record that reproduces.

    The record carries the seed, the frame's size and digest, the indices and
    the keys. `scripts/check_draws.py` re-runs the sample from the seed and
    the size and refuses a record whose indices do not come back — which is
    what makes "this came out of `--unchecked`" a checkable statement rather
    than a promise in an entry.

    The digest is over the frame's *contents*, and it is recorded rather than
    checked: the frame shrinks as rows are stamped, so a record written today
    cannot be re-derived tomorrow. What it is for is the opposite direction —
    two records claiming the same frame must agree about what was in it.
    """
    import hashlib
    import random
    from datetime import date

    keys = frame(root)
    if count > len(keys):
        raise ValueError(f"asked for {count} rows from a frame of {len(keys)}")
    indices = random.Random(seed).sample(range(len(keys)), count)
    return {
        "drawn": today or date.today().isoformat(),
        "seed": seed,
        "frame": len(keys),
        "digest": hashlib.sha256("\n".join(keys).encode()).hexdigest(),
        "indices": indices,
        "keys": [keys[i] for i in indices],
    }


def draw_path(record: dict, root: Path = ROOT) -> Path:
    """Where a draw record belongs. One file per draw, never a shared log —
    the same reason `ledger/README.md` gives for entries."""
    return root / DRAWS / f"{record['drawn']}-{record['seed']}.json"


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
    if argv and argv[0] == "--unchecked":
        # No default staleness, unlike `--unread`: see the docstring. A number
        # asks "not read since", nothing asks "never read".
        days = int(argv[1]) if len(argv) > 1 else None
        lines = unchecked(days, root)
        for line in lines:
            print(line)
        since = f" in {days} days" if days is not None else " against the tree, ever"
        print(f"{len(lines)} deliberate and not read{since}")
        return 0
    if argv and argv[0] == "--draw":
        # A seed is required, not generated. A generated one would be printed
        # and then have to be copied into the record by hand, which is the
        # step that goes wrong; asking for it makes the reproducible thing the
        # only thing you can do.
        if len(argv) < 3:
            print("usage: caveats.py --draw <count> <seed>")
            return 2
        record = draw(int(argv[1]), int(argv[2]), root)
        path = draw_path(record, root)
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists():
            print(f"{path.relative_to(root)} already exists; pick another seed")
            return 2
        path.write_text(json.dumps(record, indent=1) + "\n", encoding="utf-8")
        status = load(root)
        for k in record["keys"]:
            print(f"{k}\n    by: {status.get(k, {}).get('by', '')}")
        print(
            f"\n{len(record['keys'])} drawn from a frame of {record['frame']}, "
            f"recorded in {path.relative_to(root)}. Stamp each row you read "
            f"with `{CHECKED}` and `\"{DRAW}\": \"{path.stem}\"`."
        )
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
                "| --unread [days] | --unchecked [days] | --residual]"
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
    # The `deliberate` bucket's read count, on the summary line rather than
    # behind `--unchecked`, because five sampling passes read 246 of these and
    # the only place the total was ever written down was a ledger entry
    # nobody's next pass read. A number that moves on every run is a number
    # somebody notices standing still.
    left = len(unchecked(None, root))
    print(
        f"{total} caveats: {counts['open']} open, {counts['narrowed']} narrowed, "
        f"{counts['closed']} closed, {counts['deliberate']} deliberate "
        f"({counts['deliberate'] - left} read against the tree), "
        f"{counts['untriaged']} untriaged"
    )
    return 1 if problems or orphans else 0


if __name__ == "__main__":
    sys.exit(main())
