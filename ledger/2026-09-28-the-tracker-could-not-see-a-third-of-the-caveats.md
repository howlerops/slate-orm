# The caveat tracker read bold leads only, and 468 caveats have none

- **Date:** 2026-09-28
- **Author:** an agent session, closing the residual of
  `ledger/2026-09-26-the-backlog-is-read.md`
- **Touches:** `scripts/caveats.py`, `scripts/test_caveats.py`
- **Kind:** fix

## What changed

`caveats.py` extracted only paragraphs in a `## What this does not do` section
that open with `**`. It now reads every paragraph, splits a tight list into its
items, and drops four shapes that annotate a caveat rather than making one —
blockquoted closure notes, fenced output, a `*(Closed, …)*` parenthetical, and
struck-through text. The key is still the **bold lead where there is one**, so
no verdict written before today moved.

The count went from **1065 caveats to 1533**.

## Why

The bold lead is a convention and it was not always the convention. Measured
across the 408 entries: 1071 paragraphs carry one and **379 do not**, and every
one of the 379 was invisible. Splitting the tight lists into their items brings
the real figure to **468 caveats the tracker could not see**, because 53 of the
54 list blocks hold several items each and reading a block as one caveat would
have keyed all of them on the first one's opening words.

**129 entries had no caveat the tracker could see at all.** By day: the whole of
the 13th (1 visible, 55 invisible), most of the 14th (34 against 93), and the
19th (43 against 75). The lead firmed up around the 20th and the misses fall
away after it — 0 on the 23rd, 0 on the 24th, 1 on the 26th.

So the headline this file exists to print — "0 open" — was a statement about
**74%** of the caveats, and the missing quarter was the *oldest* quarter. That
is the worst possible skew, because an old caveat is the one most likely to have
been either quietly closed or quietly forgotten, and neither shows up in a
number that cannot see it. This session reported "0 open, 0 untriaged" several
times on that basis. It was true of what the tracker read and it was not the
claim a reader would take from it.

It is the never-fires shape wearing a headline number, which this file's own
docstring warns about one level up: a check that cannot see a thing prints the
same "clean" as a check that saw it and found nothing.

## Alternatives rejected

**Rewrite the 379 paragraphs to carry a bold lead.** Tempting, mechanical, and
forbidden by `ledger/README.md` for the reason the whole tracker is built
around: an entry is dated and append-only, and the status of a claim is not the
claim. It would also have destroyed the evidence — the date distribution above
is only legible because the old entries were left as they were written.

**Key a plain paragraph on its first sentence rather than its first 60
characters.** Prettier keys, and it would have split on the `.` inside
`` `EXPLAIN` `` , `2026-09-15` and `v0.0.1`, which appear in dozens of these.
`KEY = 60` is already the repository's answer to this question and changing it
for one class of caveat would have made two rules where there was one.

**Treat a tight list block as a single caveat.** Simpler, and wrong in a way
that hides rather than reports: 53 of 54 blocks hold several items, so every
item after the first would key on the first one's words, and a wrong key reads
as triaged. Missing a caveat is better than mis-attributing one.

**Count the annotations too and let triage sort them out.** 77 of them, and
each would have had to be given a verdict saying "this is not a caveat" — a
verdict the vocabulary has no word for. `moment` is the closest and means
something else. Excluding them by shape is one rule; 77 rows of apology is not.

## Evidence

Nine mutations, nine caught by *named* tests, no survivors:
`ledger/mutations/20260928T194638-scripts-caveats-py.json`.

Getting there took four runs and three survivors, all three of them honest:

  * **`\A` in the lead pattern.** Removing it survived. It was not a missing
    test — `re.match` anchors on its own, so `\A**` and `**` are the same
    pattern under `.match`, and the mutation was not a change. This is the
    equivalent-mutation trap `CLAUDE.md` names, met for the fourth time in this
    repository. The real hazard is `.search`, which *is* a change: 126 plain
    paragraphs here carry emphasis mid-sentence, and 26 of them would key on
    the same six words. `\A` is gone and a `.search` case is in.
  * **Stripping the list marker off a claim.** Survived, and the cause was
    redundancy: `ITEM.split` consumes every marker including the leading one,
    so the strip could never fire. Removed rather than tested.
  * **A second constant for "is this block a list".** Survived a whitespace
    loosening, because `ITEM` was what actually decided. Folded into `ITEM`.

And one survivor that *was* a missing test: dropping `~~` from the annotation
set survived every case here, because every struck fixture happened to open
with the word "Withdrawn" and the withdrawal pattern caught it anyway. Most of
the 47 real struck paragraphs do not — they open with the retracted sentence.
That case is now written, and with it the withdrawal pattern's own `~*` became
dead and came out.

Against the real tree, before anything was committed: **1065 verdicts before,
0 orphaned after, 0 colliding keys, 468 added.** That is the property that
mattered most — a widening that re-keyed even one verdict would have silently
reverted it to untriaged.

## What this does not do

**The 468 are untriaged, and that is the work this surfaced rather than the
work it did.** `check.sh` reports the count and does not fail on it, so nothing
forces the next session to act; what has changed is that the number is now
visible instead of absent.

**The four annotation shapes are recognised by their opening characters, not
understood.** A closure note written without a blockquote, a fence or the word
"Closed" will be counted as a caveat, and a genuine caveat that happens to open
with `>` will be dropped. Both were looked for and neither exists today; the
second is the dangerous direction and nothing detects it.

**A caveat outside the section is still invisible**, and so is one in an entry
with no such heading. Measured: every one of the 408 entries has the heading,
spelled exactly, so there is nothing to find today — but the tracker would not
say so if there were, because a missing section is skipped silently rather than
reported.
