# 196 entries carry 762 recorded caveats, and nothing knew which were still true. Asked "what is left", the only honest answer was to read 196 files.

- **Date:** 2026-09-24
- **Author:** Claude Code, working #301 (F7b)
- **Touches:** `scripts/caveats.py`, `scripts/test_caveats.py`, `docs/caveat-status.json`, `scripts/check.sh`
- **Kind:** making the ledger's own backlog answerable

## What changed

`scripts/caveats.py` extracts every **What this does not do** bullet from
`ledger/` and joins it to a verdict in `docs/caveat-status.json`:

- **open** — still true, still work.
- **closed** — done since; `by` names the entry that did it.
- **deliberate** — a decision with reasoning written down, and `by` names where
  that reasoning lives. Not a backlog item.
- **untriaged** — the default.

The first run: **762 caveats across 196 entries.** Today's 39 are triaged — 21
open, 17 deliberate, 1 already closed. The other 723 are untriaged and say so.

## Why

The discipline of ending each entry with its limits has held for a month, and
it produced a backlog nothing could read. A bullet written on the 14th saying
"no client SDK has a window surface" was closed on the 22nd, and the entry
still says it — correctly, because an entry states what was believed on its
date and `ledger/README.md` forbids rewriting one.

So the honest answer to "what is left to do" was 196 files, and the practical
answer was whatever the last session happened to remember. Asked exactly that
question this week, I answered from the three days I had in context and called
it an inventory. It was 12% of one.

## Alternatives rejected

**Edit the entries, striking closed caveats through.** What the strikethrough
convention already does for withdrawn *claims*, so it has precedent. Rejected:
`ledger/README.md` makes entries append-only because a dated record that is
rewritten when the world changes is not a record. The status of a claim is not
the claim, and belongs beside it.

**A hand-maintained TODO.md.** #267 is an entry about three lists this
repository maintained by hand and the guards it took to stop them drifting. A
fourth would drift the same way: the bullets are generated from the ledger, and
only the verdicts are written by a person.

**Key a verdict by entry and bullet index.** Simpler, and wrong the first time
somebody inserts a bullet — every verdict below it would silently shift onto a
different claim. Keying on the bullet's opening text means an *edit* orphans
the verdict loudly instead.

## Evidence

Eleven tests over written trees, and **nine mutation cases across two runs**.

The first run — `ledger/mutations/20260924T071747-scripts-caveats-py.json`
— was six cases and came back `problems`: four caught, one survivor, one that never
ran.

`KEY = 10000` **survived**, and it was a real finding. Every fixture bullet was
shorter than the 60-character cut, so truncation never happened and the
mutation changed nothing — the equivalent-mutation trap `CLAUDE.md` warns
about, pointing at a genuinely missing test. The suite is eleven and not nine
because two cases now bracket that boundary: an edit *past* the cut keeps its
verdict, an edit *before* it orphans one.

The case that never ran was an invalid mutation rather than a survivor.
Removing the unknown-verdict branch made the counter raise `KeyError`, so the
suite died before reporting; `mutate.py` said `NOTHING RAN` instead of scoring
it, which is the behaviour #294 gave it. Rewritten to delete only the
reporting, it is caught.

The second run — `ledger/mutations/20260924T071856-scripts-caveats-py.json`
— reran the survivor, added its mirror and replaced the invalid case. That
record is `clean`: three cases, each one caught by a named test.

`scripts/check.sh` at **53 passed, all of them** (51 before).

## What this does not do

**723 caveats are untriaged**, which is 95% of them. The tracker makes the
backlog answerable; it does not answer it. Nothing here estimates how many of
the 723 are already closed, and the one closed caveat found in today's 39
suggests the real open count is far below 762 — but that is an impression from
a 5% sample, not a measurement.

**A verdict is a judgement, and nothing checks it.** `closed` must name
something, and that name is not verified to exist or to say what the verdict
claims — `check_cited_docs.py` would catch a dead path in a source file but
does not read this JSON. A wrong `closed` is invisible.

**It reads one section heading.** A caveat written under a different heading,
or in prose rather than a bold lead, is not seen. The 762 are the ones that
followed the template.
