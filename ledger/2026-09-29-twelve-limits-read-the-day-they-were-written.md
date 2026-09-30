# Triaging the day's own caveats as they were written cost one open row instead of eight

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

Twelve verdicts appended for the four entries written this afternoon about
`check_handlers.py` rules 8 and 10: **eight `deliberate`, three `narrowed`
with residuals, one `moment`, none `open`** — and then three more for this
entry itself, of which **one is `open`**. The untriaged count for the
repository is zero.

## Why

Earlier today this session measured the thing that makes the caveat backlog
stay still: **forty-six entries dated 2026-09-29 carried seventy-three `open`
rows between them, 1.6 per entry.** An entry that records its limits honestly
adds to the list faster than closing work removes from it, which is not an
argument for recording fewer limits — it is an argument for triaging them
when they are written, while the reasoning is in hand, rather than leaving
them to a later reader who has only the sentence.

Three of these twelve are the demonstration. The `tables()` hole was recorded
as a limit by three consecutive entries, each honestly, each one noting that
the entry before it had not closed it. Triaged the same day, the third
recording becomes what it always was: a task nobody had started. It is closed
now and the three rows are `narrowed` against a measured residual, not `open`
against a description.

## Alternatives rejected

**Leave them untriaged.** The default, and it is not neutral: an untriaged row
is absent from the tracker entirely, so the backlog looks smaller than it is
and `--unread` cannot surface it. The tracker's own design note says the
untriaged state exists to be emptied.

**Mark them `open` and batch the reading later.** Honest, and it is what the
backlog already is: a hundred and forty rows, all verified inside two days,
almost all from entries whose authors had the reasoning and did not write it
down as a verdict. Adding twelve more to be re-derived by somebody with less
context is the process that produced the hundred and forty.

**Call the three `tables()` rows `closed`.** They are not. Rule 8's pattern is
a spelling — `find`/`any`/`position` over a closure comparing `name()` — and a
map keyed by name passes it. `narrowed` with the residual spelled out is what
the tracker has that verdict for, and `closed` would need a witness in the
tree that does not exist.

**Skip the entry and just edit the JSON.** The hook would refuse the commit,
correctly. It would also lose the only part of this worth keeping, which is
the count: the honest triage of a day's own work came out zero `open`, and
that is a claim a later reader should be able to check against these four
entries rather than take on trust.

## Evidence

The twelve, by verdict:

| verdict | n | why, in one phrase |
| --- | --- | --- |
| `deliberate` | 8 | an alternative was costed in the entry and declined |
| `narrowed` | 3 | the `tables()` hole, after rule 8 widened to the workspace |
| `moment` | 1 | "the eight-file count is today's" — a snapshot, not a gap |
| `open` | 0 | |

And this entry's own three: one `deliberate`, one `moment`, and one `open` —
that `moment` and `deliberate` verdicts have nothing auditing them, which is
a gap with a method and no excuse.

`python3 scripts/caveats.py --untriaged`: **0 untriaged**, from 12.

Counts after those twelve: 1034 `deliberate`, 470 `closed`, 145 `moment`,
140 `open`, 115 `narrowed`. The `open` count is unchanged across an afternoon
that added four entries, which is the point: **1.6 open rows per entry is not
a law, it is what happens when limits are recorded and not read.**

**This entry's own three limits then cost one `open`**, and it is the right
one to pay: "`moment` and `deliberate` are not audited by anything" is a real
gap with a real method — read them — and 264 of 1034 `deliberate` verdicts
have been read while `moment` has had none. Final counts, this entry
included: 1035 `deliberate`, 470 `closed`, 146 `moment`, **141 `open`**, 115
`narrowed`. So the honest figure for the afternoon is five entries and one
open row, not zero: **0.2 per entry against the day's 1.6.**

`sh scripts/check.sh`: 87 passed, all of them.

One key was stored wrong and the tracker caught it. `caveats.py` prints a
claim in full and keys it on the first sixty characters of its normalised
text; "The `#[cfg(test)]` case is now a roster entry, not an exclusion." is
sixty-four. Appending the printed string left the row invisible to its own
claim and the count stayed at one untriaged. The lesson is narrow and worth
having: **the key is `caveats.key(claim)`, never the line `--untriaged`
prints**, and the check that caught it was the count not going to zero.

## What this does not do

**A verdict is a reading, and eight of these are mine about my own work.**
The entries and the triage have the same author and the same afternoon, which
is the condition under which a limit gets called `deliberate` because the
author remembers deciding it rather than because a reader would agree. The
audit this session ran on `deliberate` verdicts found four false in 264 read —
1.5%, Clopper-Pearson 0.4% to 3.8% — and nothing here says these twelve are
better than that rate.

**`moment` and `deliberate` are not audited by anything.** A `moment` row goes
stale by definition: "the eight-file count is today's" stops being true the
day somebody adds a file, and nothing re-reads it. `closed` has a witness
guard and `narrowed` has a residual a reader can check; the other two rest on
prose.

**It measures one afternoon.** Four entries by one author on one guard is not
a sample that says anything about whether same-day triage generalises. The 1.6
figure it is set against came from forty-six entries.
