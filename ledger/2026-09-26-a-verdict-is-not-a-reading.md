# Thirty-seven caveats were `open` that are `deliberate`: decisions with the reasoning written down in the caveat itself. Re-reading them for *verdict* rather than for truth moved 37 out of the backlog without touching a line of code.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`
- **Kind:** process — re-verdict, no code

## What changed

Every `open` caveat was re-read for the **verdict** rather than for whether it
is still true. 37 moved to `deliberate` and one to `narrowed`. `open` falls
from 279 to 242.

## Why

Because the reading pass answered the wrong question about a third of the list.

That pass asked "is this still true?", and for 364 of 383 the answer was yes —
so they stayed `open`. But `scripts/caveats.py` defines `open` as "still true,
**still work. The thing to do**", and `deliberate` as "a decision with
reasoning written down, not a backlog item… Reversing it is a design
conversation, not a chore, and **counting it as debt misreads the ledger**".

A caveat can be perfectly true and not be work. These were the shape:

> **The truncation is UTC, like everything else here.** `month_start` in
> another zone is `month_start(t + offset)` shifted back, which is not what a
> caller means and is not offered.

That is not a backlog item. It is a decision — the naive version would be
wrong, so it is not offered — with the reasoning in the sentence. Counting it
as debt says this repository owes 279 things when it owes 242.

The original bulk triage, which had 677 caveats to sort in one sitting, could
not read each one this closely. Nothing was wrong with it; it was the pass that
made this one possible.

## The criterion

**`deliberate` requires the reasoning to be written down, in the caveat or its
entry: the alternative named, and why it was not taken.** Everything else stays
`open`.

Applied strictly, which cost several that were tempting:

| stays `open` | why it is not a decision |
|---|---|
| "the pager built for the keyspace viewer was not reused here, **and should be**" | says outright it should be done |
| "a guard would have to be a test that runs a script in a tree with no build artifacts, which is **a real thing to want** and is not here" | names the alternative and wants it |
| "the shape is **probably** an admin RPC rather than a client call" | a guess, not a decision |
| "That is a weaker guarantee than it sounds and **I have not closed it**" | says so |
| "Not attempted, not designed." | no reasoning at all |
| "`trips` has one secondary index… correctly but unremarkably" | describes, does not decide |

The line is "the alternative was weighed" against "the alternative was named".
Both read similarly and only one is a decision.

## Alternatives rejected

**Move the ones that merely *sound* settled.** A caveat written in a calm voice
is not a decision, and this repository's caveats are all written in a calm
voice. The table above is the discipline: six that read as accepted and are
not.

**Leave them `open` and note the distinction in prose.** What every one of
these entries already does — the reasoning is right there in the bullet — and
it has not worked, because the tracker is what gets counted and the tracker
said `open`. The status of a claim is not the claim, and "this was decided" is
a status.

**Add a `wontfix`.** `deliberate` already is that, with a better name: it says
*why* rather than *no*, and `by` is required so the reasoning has to be named
rather than assumed.

## Evidence

**Counts.** Before: 279 open, 8 narrowed, 193 closed, 322 deliberate. After:
**242 open, 9 narrowed, 193 closed, 359 deliberate, 0 untriaged.** 865 caveats
in all; no caveat changed text, and every `deliberate` carries a `by` naming
where its reasoning lives — `scripts/caveats.py` refuses one that does not, and
`scripts/test_caveats.py` has a case for that.

**Yield, by batch of roughly eighteen:** 3, 7, 5, 7, 7, 6. Between one in
three and one in six, with no trend — which is what you would expect if the
original triage's misreading was uniform rather than concentrated in one era.

**`scripts/check.sh`: 59 passed, all of them.** No code changed, so nothing
here can be mutation-tested; the only edit is verdict fields in
`docs/caveat-status.json`.

**A null result worth stating:** nothing moved to `moment` in this pass, and I
looked. The bulk triage found `moment` cases readily — it is a distinctive
shape, a statement about one run — and it seems to have found most of them.
`deliberate` is the one it under-applied, which makes sense: telling a decision
from a gap needs the paragraph, and `moment` usually needs only the lead.

## What this does not do

**242 caveats are still open and they are still work.** This pass moved the
mislabelled ones; it did not close anything. The number that fell is the
overstatement.

**Every judgement here is mine and nothing checks it.**
`2026-09-24-the-ledger-records-762-caveats-and-tracked-none-of-them.md` already
says "a verdict is a judgement, and nothing checks it", and this pass is 37
more judgements resting on that. The criterion is written above so a reader can
disagree with a specific one rather than with the pass.

**The reverse direction was not swept.** 359 `deliberate` verdicts now exist
and I did not re-read them for ones that are actually `open` — a decision that
has stopped being defensible looks exactly like one that has not. The sweep ran
one way because that is the way that shortens the backlog, which is a reason to
be suspicious of it.

**`narrowed` was applied once and could fit more.** Several of the caveats left
open are half-answered in ways that need a closer look than a verdict pass
gives. That is the same work again at a finer grain.
