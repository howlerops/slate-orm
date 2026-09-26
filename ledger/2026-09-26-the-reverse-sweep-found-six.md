# The reverse sweep: 322 `deliberate` verdicts re-read against the stricter criterion, and six of them were wrong — a 1.9% error rate, which is the null result the forward sweep needed

- **Date:** 2026-09-26
- **Author:** Claude, working from the standing instruction to address every caveat
- **Touches:** `docs/caveat-status.json`, `ledger/`
- **Kind:** process

## What changed

The forward sweep moved 101 caveats out of `open` in one direction across
thirteen batches. Its own closing caveat said what was wrong with that:

> **The reverse direction was not swept.** … a decision that has stopped being
> defensible looks exactly like one that has not. The sweep ran one way because
> that is the way that shortens the backlog, which is a reason to be suspicious
> of it.

This is the other direction. Every `deliberate` verdict that the original
triage wrote and no later pass had re-read — **322** of them — was read again
against the criterion the forward sweep used: *the alternative named, and why
it was not taken, written down in the caveat or its entry*.

**Six moved.** Five to `open` and one to `moment`. The other 316 held and now
carry a `checked: 2026-09-26` stamp, so the reading is in the data and not only
in this paragraph.

Two caveats that asked for exactly this pass became `narrowed`, because the
pass happened and half of what each asked for is still missing.

## Why

A one-way sweep is a filter, not a reading. Running it in the direction that
shortens the backlog and never in the direction that lengthens it produces a
number that falls whether or not anything is true, and the forward sweep's own
entry said so before this one existed.

The six that moved share one grammar, and it is worth naming because it is the
tell:

| entry | the caveat's own words |
|---|---|
| `a-command-that-cannot-finish-here.md` | "**I did not try** to find the minimal number of groups" |
| `generate-the-rows-from-the-table.md` | "the next thing a user will want and **it is not built**" |
| `generate-the-rows-from-the-table.md` | "the generator reads the *type*, and the type is all it reads" |
| `a-scan-gets-cheaper-per-row…` | "**A third point could easily show** it flattening, or reversing" |
| `the-mutation-runner-could-not-read-a-doctest.md` | "libtest has others **I did not provoke**" |

First person, past tense, and no clause saying why. Each *describes* a limit
fully and *argues* for it not at all. Beside them, the 316 that held read like
`the-check-the-gitignore-asked-for.md`'s — "because a `Cargo.toml` says nothing
about where its output lands" — where the reason is a clause in the same
sentence.

The sixth is a different error. `the-timing-flake-was-fixed-once…` records
"The third mutation I tried was a no-op … Fourth of the day", which is a
statement about one run on one afternoon: `moment`, not a decision. It was
filed as `deliberate` because it sits in a `## What this does not do` section
and reads like a limit.

**The original triage predicted this, and the prediction is now measured.**
`2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md`:

> A second reader would move some of them, and **the ones most likely to move
> are where an entry explains a limit without quite arguing for it.**

Five of the six are exactly that shape. The rate is 6 in 322, **1.9%**. That
caveat is now `narrowed`, and what is left of it is the part I cannot supply:
I am not a second reader. I am the same one, a day later, holding a criterion
that had not been written down when the verdicts were made. An independent
reading would find more, so 1.9% is a floor.

## Alternatives rejected

**Re-read only a sample and extrapolate.** Thirty verdicts would have taken
a tenth of the time and put a confidence interval on the rate. It was the
tempting option and it is the wrong one here: the *value* of the sweep is the
six corrections, not the rate. A sample would have found roughly one of them
and left five wrong verdicts in the file, which is the same trade as sampling
a test suite. The rate came out of the sweep for free; it was not what the
sweep was for.

**Re-read the 195 `closed` verdicts in the same pass.** A wrong `closed` is
strictly worse than a wrong `deliberate` — it removes a caveat from every
listing rather than reclassifying it —
and `2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md` records that
167 of them "rest on evidence of very uneven strength". It is also a different
job: checking `closed` means going to the tree and confirming the thing exists,
not re-reading prose, so it is a day's work at the reading pass's measured
rate rather than an afternoon's. Folding it in would have meant one commit
where two kinds of evidence could not be told apart. It is recorded below.

**Widen the criterion so fewer verdicts are wrong.** If "the entry describes
the limit clearly" counted as `deliberate`, all six would be right and the
verdict would mean nothing — every caveat in this ledger describes its limit
clearly, because that is what the template asks for. The criterion has to be
the one thing a caveat can fail, and "why it was not taken" is that thing.

**Stamp nothing, and record the sweep only here.** The `checked` field exists
and `unread()` ignores it on a `deliberate` row, so stamping 316 of them is
inert to every listing. It is not inert to the next reader: without it, the
only way to know these were re-read is to find this entry, and the file is
what a tool reads. The cost is that `checked` now means two things — see the
caveat below.

## Evidence

Counts from `python3 scripts/caveats.py`, before and after:

| | open | narrowed | closed | deliberate | moment | total |
|---|---|---|---|---|---|---|
| before | 184 | 10 | 195 | 421 | 63 | 874 |
| after | 189 | 12 | 195 | 413 | 64 | 874 |
| …after this entry was written | 192 | 12 | 195 | 415 | 64 | 879 |

322 read; 6 moved (5 → `open`, 1 → `moment`); 316 confirmed and stamped; 2
`deliberate` → `narrowed` for asking for this pass. 421 − 6 − 2 = 413.

The last row is read off a run made *after* this file existed, not computed
from the one above it. Subtracting moves from an old total is the arithmetic
`2026-09-26-the-verdict-sweep-finished-and-corrected-itself.md` corrects the
entry before it for, and it is wrong the same way twice in one day if the
five caveats this entry adds are not counted.

The 99 `deliberate` verdicts already carrying a stamp were not re-read: they
were written by the forward sweep earlier today under this exact criterion, so
re-reading them would be reading my own afternoon's reasoning back an hour
later, which finds nothing and reports a number.

`python3 scripts/caveats.py` reports no problem and no orphan. `--unread 30`
reports 0. `sh scripts/check.sh`: 59 of 59 pass.

**No mutation run.** Nothing executable changed. The tracker's behaviour is
covered by `scripts/test_caveats.py` (29 cases) and is untouched.

**A null result, stated plainly:** I expected this sweep to find a dozen or
more, on the reasoning that the original triage read 677 caveats in one sitting
and any bar applied that fast drifts. It found six. The bar the original triage
wrote down — "`deliberate` where the entry's own prose gives a reason for not
doing the thing that reads as a decision" — turns out to be the same bar the
forward sweep formalised, applied consistently 316 times out of 322. That is
the useful finding and it is the opposite of what I went looking for.

## What this does not do

**The 195 `closed` verdicts have never been re-read by anything.** They are the
only class where a wrong verdict is invisible in every listing, and the entry
that wrote 167 of them says their evidence is of very uneven strength. This
sweep makes that the largest unread surface in the tracker by a wide margin,
and the caveat it closed half of
(`a-sixth-verdict-for-a-caveat-half-done.md`'s) names it as the residual.

**`checked` now means two different things.** On an `open` or `narrowed` row it
means somebody read the caveat against the tree and believes it is still true.
On these 316 `deliberate` rows it means somebody read the caveat against *its
own entry* and believes the verdict is right — a weaker and different claim,
about prose rather than about code. Nothing in the file distinguishes them and
`unread()` cannot, because it filters `deliberate` out before it looks. A
second field would say it; one field with two meanings is what is there.

**It is the same reader.** The caveat this closes half of asked for a second
one, and a second reader is the only thing that measures how much of the 1.9%
is the criterion and how much is me applying it the same way twice. Nothing in
this repository can supply that.

**Five caveats went back on the backlog and none of them got done.** Moving a
verdict to `open` is bookkeeping. The mutation runner still reads four libtest
shapes, the generator still does not know that a column called `email` is an
email, and the scan curve is still two points.

**No pass has looked for `narrowed` among the `deliberate` rows.** I read 322
asking "is this actually open?" and found five. "Is this actually half-done?"
is a different question over the same text and I did not ask it, because the
distinction earns its keep on the open list — where it changes what somebody
picks up next — and settles nothing on a decision.
