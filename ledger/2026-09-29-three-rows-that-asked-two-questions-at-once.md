# Three open rows asked two questions at once, and only one of them was refused

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

Three `open` caveats say the same thing in three phrasings:

- `2026-09-18-a-mean-and-a-maximum-do-not-describe-a-latency.md` :: *"Still
  timed to the response head."*
- `2026-09-18-a-number-you-cannot-subtract-is-not-a-measurement.md` :: *"Still
  the response *head*."*
- `2026-09-18-the-failure-that-arrives-after-the-answer-has-started.md` :: *"The
  duration is still the head's."*

All three move to `narrowed`, crediting
`ledger/2026-09-29-a-slow-reader-makes-a-response-later-not-larger.md`, with the
residual being **time to the first frame** — the measurement that is neither
taken nor refused.

The open count went 130 → 127.

## Why

They were found by sweeping the open rows against *each other*, which
`ledger/2026-09-29-two-rows-for-one-gap.md` had recorded an hour earlier as not
done. The sweep before it compared open against settled and would never have
seen these, because all three are open.

Reading them together is what shows the problem: each one bundles two questions
that have different answers.

**"A streamed read's rows are in no number this node reports"** was true and is
no longer. The body is weighed now — `slate_response_bytes_total` and
`slate_response_frames_total` — because the argument against measuring a stream
was about *time* and does not reach *size*.

**"The duration should run to the last row"** is refused, and has been since the
layer was built: a slow consumer would be reported as a slow server. That is not
undone work and `open` was the wrong verdict for it.

So neither `open` nor `deliberate` was right for the rows as written, and
`narrowed` is — half answered, half refused, with the residual naming what is
actually left.

**And the residual is not either of the two halves.** Time to the *first row* is
unconfounded, because backpressure cannot apply before the first frame goes out,
and nothing measures it. That is the real remaining question and none of the
three rows asked it.

**What the head covers was checked rather than assumed**, because the first
draft of this entry filed it as unknown and it is knowable by reading. The
`Query` handler does not return as soon as it is called: it spawns the scan and
then awaits a `started` oneshot, which `scan.run` fires only after
`view.execute` hands back a cursor. So tonic sends the head after
authorisation, conversion, routing, planning and opening the scan, and before
any row. The head is therefore not "almost nothing" for a streamed read — it is
the whole of the setup — and what it misses is per-row work alone. The module
doc's sentence that "a streaming read returns its head almost immediately" is
true relative to the whole call and misleading about what it contains.

## Alternatives rejected

**Move all three to `deliberate`.** The tidier answer, and the one that lowers
the count furthest. Rejected because it would file the first-frame question as
decided when it has not been asked, and a `deliberate` row is one nobody reads
again. The count falling by three instead of by more is the cost of saying what
is left.

**Leave all three open until the first-frame measurement exists.** Defensible:
something in the neighbourhood is genuinely undone. Rejected because it keeps
three rows on the open list whose text asks for a thing that is refused, so a
reader picking one up would build the confounded histogram the module doc argues
against. A row should ask for what somebody should do.

**Rewrite the three claims in their entries so the tracker sees the real
question.** The cleanest in principle and rejected on the strike convention's
cost: striking a claim orphans its verdict, so each would need the three-entry
pair form, in three files, to reword a sentence whose meaning the `residual`
already carries. `residual` exists for exactly this.

**Fold the three into one row.** Not expressible. The tracker keys on
`entry::claim`, so a caveat lives where it was written; the same limit
`2026-09-29-two-rows-for-one-gap.md` records.

## Evidence

`python3 scripts/caveats.py`: **127 open, 107 narrowed**, from 130 and 104.

The open-against-open sweep over all 130 rows at a similarity above 0.80
returned five pairs.

| pair | verdict |
| --- | --- |
| the three response-head rows (two pairs of the group) | one gap, narrowed here |
| `A conditional update is one round trip per row.` in two entries, **identical** | one gap, **both correctly open** |
| `Nothing measures the cost.` vs `Nothing measures a sweep's cost.` | soft-delete reads vs a purge — different subjects |
| `Nothing measures the cost.` vs `Nothing measures what the layer costs.` | soft-delete reads vs the stream cap — different subjects |
| `Nothing measures what the restored floor costs.` vs `Nothing measures what the layer costs.` | freshness floor vs stream cap — different subjects |

**The exact-match pair is a null result and is left alone.** `A conditional
update is one round trip per row.` appears word for word in
`2026-09-18-a-decimal-and-a-conditional-update-on-the-wire.md` and
`2026-09-18-decimals-and-conditional-updates-in-three-clients.md`, and it is one
gap. But neither is settled, so there is no argument for one to credit the
other, and both entries are places a reader would want to meet it. Two rows for
one open gap is the tracker working as designed; two rows where one is decided
is not, and that is the case the earlier entry acted on.

Three of the five pairs are the phrasing coincidence this repository keeps
rejecting rules for: *"Nothing measures the cost"* scores above 0.80 against
itself across four unrelated subjects. Similarity finds wording, not subject,
which is why every pair here was confirmed by reading both entries rather than
by the number.

## What this does not do

**It does not measure time to the first row.** That is the residual. Reading
established *where* the head falls; it did not time anything, and a histogram
for the first row needs a decision about where to sample and 26 more series a
method.

**The sweep is one threshold on one metric.** 0.80 on a character-level
similarity over the first sixty characters of a claim. Two rows recording one
gap in genuinely different words score below it and were not found; the sixty
characters are the tracker's key length, so a pair differing only after
character sixty would score 1.00 falsely.

**Nothing re-runs it.** The sweep is fifteen lines in a scratch directory, run
twice today. The next duplicate pair arrives unnoticed, which is the standing
caveat `2026-09-29-two-rows-for-one-gap.md` already carries and this does not
improve.
