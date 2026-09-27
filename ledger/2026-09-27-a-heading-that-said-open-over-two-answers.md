# The array note's "Open, and deliberately not decided here" sat above two questions the build had already answered

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-20-an-array-is-a-terminator-not-a-count.md`
- **Touches:** `docs/arrays.md`
- **Kind:** docs

## What changed

`docs/arrays.md`'s section heading **Open, and deliberately not decided here**
is struck through and replaced with **Left open here, and answered by the
build**, and each of the two questions gains its answer in its opening line:

- *Whether the element type may be nullable* — **answered: no, and reversibly.**
- *What `SUM` and `COUNT` do with an array column* — **answered: `SUM` refuses,
  `COUNT` counts.**

The two `>` blocks under them, which carry the reasoning and were already
there, are untouched.

## Why

Both answers were in the note. Both were in the tree. The heading above them
still said the questions were open, and a heading is what a reader navigates
by — somebody scanning `docs/arrays.md` for what is settled would have skipped
the section, and somebody looking for whether a null element is accepted would
have concluded nobody had decided.

That is the same failure as the *Stops at* lines corrected an hour ago and the
"what is not here" bullets corrected before those, in a third shape: not a
claim that went stale, but a **label** that went stale over content that did
not. The content was updated by whoever did the work; the heading was nobody's.

Keeping the questions as questions with answers attached, rather than
rewriting them as decisions, because the reasoning is the valuable part and it
is *about the question*. The first was decided on reversibility rather than on
semantics — refusing a null element can be relaxed later without breaking a
stored row, and accepting is a promise three clients would have to keep from
the day it ships. The second was decided by running it rather than reasoning
about it, and the note says so against itself: its own guess used the word
"presumably" two sentences after warning about that word.

## Alternatives rejected

**Delete the section.** Both questions are answered, so the section is
arguably just history. Rejected because the *reasoning* for each is the record
of why the build chose what it chose, and neither choice is obvious from the
code: `Row::validate` refusing a null element looks like an oversight unless
you know it is reversibility, and a wildcard producing `NotSummable` looks
accidental unless you know it was checked.

**Rewrite both as decisions under a "Decided" heading.** Cleaner to read and
it loses the shape of the argument, which is *question, then answer, then why
that answer and not the other*. A decision stated flat invites the next reader
to re-open it from scratch.

**Leave the heading and add a line saying both are answered.** What the `>`
blocks already do, and it did not work — they have said so since the day the
work landed and the heading outlived them.

## Evidence

- Both answers are backed by named tests that exist:
  `an_element_of_the_wrong_type_is_refused` and
  `an_array_cannot_be_summed_but_can_be_counted`, in
  `crates/slate-kernel/tests/arrays.rs`.
- `python3 site/check/docs.py`, `python3 scripts/check_cited_docs.py` — 651
  citations openable; `sh scripts/check.sh` — 71 of 71.
- No mutation run: this changes prose and no code, so there is nothing to
  break. Said rather than omitted, because a ledger entry with no mutation
  section is usually one that skipped it.

## What this does not do

**One question in that section really is open, and it is now the only one.**
The second `>` block names it: whether an array column should get an
array-shaped aggregate — `ARRAY_AGG`, a union, a concatenation. Nobody has
asked for one, so nothing is designed and this entry does not design it. It is
visible now in a way it was not when two answered questions sat under the same
"open" heading.

**Nothing stops a heading going stale again.** The three corrections today —
a *Stops at* line, a "what is not here" bullet and this heading — are all the
same class and none has a guard. What they have in common is that the
*content* under them was updated by the person doing the work and the *frame*
around it was not, which is not something a string match can see.

**The other design notes were not checked for the same thing.**
`docs/views.md`, `docs/ctes.md`, `docs/full-text.md`, `docs/validation.md`,
`docs/paging-a-join.md` and `docs/persisting-the-schema.md` all follow this
one's structure and any of them could have an answered question under an open
heading. This is one file, found because a caveat named it.
