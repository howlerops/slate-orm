# Three mechanical routes into the open backlog, all measured and all rejected, so the 140 are real

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`
- **Kind:** docs

## What changed

Nothing in the tree. One verdict refreshed, and a null result recorded that
was worth an hour: **there is no cheap way to shrink the `open` list, and it is
not carrying filler.**

## Why

The session's standing instruction is to work until nothing is open. Before
spending further sessions on it, it is worth knowing whether the 140 rows are
140 gaps or an artefact — a list that has accumulated duplicates, stale rows
and things filed `open` that the entries themselves argue against. Every
previous cheap route into a caveat bucket has been worth measuring: two were
tried and rejected against `deliberate` earlier today, and the reading that
replaced them found a rate of 1.5%.

The answer is that this list is clean, which is the opposite of convenient.

## Alternatives rejected

**Reclassify the thin ones and move on.** This is the failure
`ledger/2026-09-29-three-verdicts-that-were-wrong-not-three-gaps.md` is about,
committed three times before anybody checked, and it is the one move available
that would make the count fall on demand. The measurement below is what a
reclassification pass would have to justify itself against, and it does not.

**Take the count on trust from the last triage.** The backlog has been triaged
several times and each pass was honest about being a sample. Three complete
sweeps over all 140 cost less than one more feature and settle the question
rather than narrowing it.

**Leave the null result unwritten.** A route measured and rejected is worth as
much as one taken, and more here: without this, the next session spends the
same hour discovering the same three answers. That is the argument
`ledger/2026-09-29-the-deliberate-verdicts-priced.md` makes for writing up a
rejected shortcut, and it applied within the day.

## Evidence

**Route 1 — duplicates.** All 140 open rows compared pairwise, normalised, with
`difflib` at 0.72. **One real duplicate in 9,730 pairs**: `A conditional update
is one round trip per row.`, in two entries a day apart, both open and both
true. Written up in
`ledger/2026-09-29-the-duplicate-sweep-and-what-an-entry-costs.md`. So the list
overstates the gap count by one.

**Route 2 — a guard for the above.** 24 key texts appear in more than one entry
across 65 rows, and the three clusters where a `closed` row and an `open` row
share a key are all different gaps sharing a generic sentence ("No
measurement.", eleven times). 24 findings, 0 real. No guard; the 60-character
key is what makes short sentences collide and a better matcher does not fix a
key.

**Route 3 — rows filed `open` that the entry argues against.** The tracker's own
rule is that `deliberate` means the entry argues against doing the thing and
`open` means ordinary undone work, so a misfiled stratum would be findable by
the argument language a `deliberate` reason carries. Searching all 140 `by`
fields for it — "not taken", "would be a", "rejected", "deliberately", "by
decision", "is the wrong" — returns **4 rows, and reading all four they are
genuinely open**: a real hole in the wire's decimal scale, an estimate named as
the direction and not built, a latency measurement blocked on MinIO, and a
calibration against an unpinned HTTP/2 window. The language appears because the
reasons *explain* the gap, not because they refuse it. **Zero misfiled.**

**And the list is current.** Every one of the 140 carries a `checked` date of
2026-09-28 (59) or 2026-09-29 (81) — each was read against the tree within 48
hours, and 118 carry a reason. 60 predate today.

So: **140 rows, about 139 distinct gaps, all verified inside two days, none
filler.** Reaching zero means building 139 things, and the rate at which entries
add rows — 1.6 per entry, measured in the duplicate-sweep entry — means the
list grows while that building is done honestly.

## What this does not do

**It does not reduce the count**, and saying so is the point. The one row the
duplicate sweep found is two true statements of one gap, so even that is not a
row to delete.

**Three routes is not every route.** A fourth might find something: clustering
by the file a caveat's reason cites, say, or asking which open rows no longer
have a subject in the tree. Neither was tried, and the first is the more
promising because a gap in deleted code is a gap that closed itself.

**It says nothing about whether the 140 are worth doing**, only that they are
real. The tracker records whether a claim is still true, which
`ledger/2026-09-28-the-first-two-days-of-the-invisible-backlog.md` argues at
length is a different question from priority — and that entry's refusal to make
this file a backlog is why there is no order to work them in.
