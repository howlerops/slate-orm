# Triaging the 468, part four: the last 130, and the number that is now real

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** process

## What changed

The last 130 verdicts — 2026-09-20 to 2026-09-27 — and with them the whole of
the backlog that
`ledger/2026-09-28-the-tracker-could-not-see-a-third-of-the-caveats.md`
surfaced. This batch: 20 closed, 6 narrowed, 22 moment, 44 deliberate, 38 open.
Nine new witnesses.

**`scripts/caveats.py` now reads `1548 caveats: 118 open, 59 narrowed, 360
closed, 878 deliberate, 0 untriaged`** — counting this entry's own three. Every caveat this repository has
written down has a verdict, and for the first time that sentence is about all
of them rather than about the 69% with a bold lead.

## Why

Because "0 open" was the headline for a fortnight and it was a statement about
1065 of 1533 caveats. Making the tracker see the other 468 was one commit;
making the number mean something was four.

What the whole pass found, over 468 caveats in 150 entries:

  * **111 were already closed** by work nobody had connected back to them.
    That is 24% — nearly a quarter of what read as standing limits had been
    answered, some of it within a day of being written.
  * **118 are open**, which is the real backlog and is now visible as one.
  * **878 are deliberate** — decisions with reasoning, not debt. That is 57%,
    and it is the number that most justifies the `deliberate` verdict existing:
    counting them as open would have described this repository as carrying
    twelve hundred outstanding items when it carries a hundred and sixteen.

## Alternatives rejected

**Stop at the extractor fix and leave 468 untriaged.** Defensible — the recall
hole was the defect, and the backlog it revealed is somebody's next task. It
was rejected because a tracker reporting 468 untriaged is not much better than
one that could not see them: both leave the honest answer to "what is left"
unavailable, and the second at least fails loudly. Four commits of reading is
what the number costs.

**Batch the verdicts by verdict rather than by date.** Faster — do all the
`moment`s, then all the `deliberate`s — and it would have produced verdicts
written without the entry around them. Reading an entry's whole
*What this does not do* section at once is what made the groupings visible: the
five copies of the scale hole, the four unmeasured round-trip claims, the three
`restore` caveats closed by one function.

## Evidence

Four commits, 468 verdicts, counted from the tracker at each step rather than
from memory:

| after            | open | narrowed | closed | deliberate | untriaged |
|------------------|-----:|---------:|-------:|-----------:|----------:|
| the extractor    |    1 |       36 |    249 |        703 |       468 |
| part one         |   19 |       45 |    289 |        737 |       364 |
| part two         |   33 |       49 |    311 |        766 |       283 |
| part three       |   78 |       53 |    340 |        833 |       130 |
| part four        |  118 |       59 |    360 |        878 |         0 |

`scripts/check_closed_caveats.py`: 340 of 360 closures carry a witness in the
tree, 20 are exempt with a reason each. Every witness needle was verified with
`git grep` before its row was written; the helper refuses one it cannot find.

`sh scripts/check.sh`: 71 passed, all of them.

Fourteen fabricated ledger filenames were refused across the four batches, and
two verdicts cited a path that resolved and said something false about it —
both caught by reading the file rather than by a guard.

## What this does not do

**It does not fix any of the 118.** They are visible, grouped where they
repeat, and untouched. The largest is a client's declared decimal scale that
nothing checks against the server's, which three entries record independently
and which renders money a hundred times wrong in silence.

**A `deliberate` verdict is only as good as the reasoning it quotes.** 878 of
1548 caveats now rest on an entry's own argument, re-read on one day by one
reader. `caveats.py --unread` is what finds the ones nobody has looked at
since; nothing finds the ones whose reasoning has quietly stopped applying.

**The 22 `moment` verdicts in this batch are the weakest.** A statement about
one run is easy to recognise and easy to over-apply: "nothing was measured" is
a moment when there was nothing to measure and an open caveat when there was.
Each was judged one at a time and some of those judgements will be wrong.
