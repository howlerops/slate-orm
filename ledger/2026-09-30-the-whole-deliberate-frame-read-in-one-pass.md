# All 1016 unread `deliberate` verdicts read against the tree, and two were false

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `scripts/read_deliberate.py`, `docs/caveat-status.json`, `ledger/draws/2026-09-30-20260930.json`, `scripts/check_closed_caveats.py`
- **Kind:** process

## What changed

`scripts/caveats.py --unchecked` listed 1016 `deliberate` verdicts nobody had
read against the tree. All 1016 were drawn in one draw
(`ledger/draws/2026-09-30-20260930.json`, seed 20260930) and read. The frame is
now empty: `--unchecked` reports 0, and the question it answers becomes
`--unchecked <days>` — "not read since" rather than "never read".

Two verdicts were false. Both are in
`ledger/2026-09-14-a-playground-nobody-could-find.md` and both describe
assertions in `site/check/playground.py`, which was deleted in `5ffc331` when
the landing page became the workbench. Both are now `closed`, citing
`ledger/2026-09-14-the-home-page-is-a-workbench.md`.

Closing those two needed one more thing. `scripts/check_caveat_citations.py`
requires every path a `by` cites to resolve, and a closure whose whole content
is *"the file was deleted"* has nowhere else to point — so it gained `GONE`, a
roster keyed by `(entry, path)` with a reason, and two rot rules: a path that
comes back, and a row nothing cites any more.

`scripts/read_deliberate.py` is the tool that made a pass this size possible.
It does the mechanical half of a read for every row at once: resolves each
backticked token in the claim against `git ls-files` and `git grep`, flags the
claim as a negative existential or as a premise about something outside this
tree, and lists the later ledger entries whose distinctive words the claim
shares. It decides nothing.

## Why

`ledger/2026-09-30-a-sample-that-pools-with-the-next-one.md` recorded the
backlog as a frame rather than a plan:

  > **1008 rows at thirty a session is thirty-four sessions.** The frame is not
  > a plan to empty it.

Thirty a session was never a throughput limit. It was a *sampling* rate,
chosen so that a pass could put a Clopper-Pearson interval on how many of these
verdicts are false — five campaigns did exactly that and reached 7 false of
246. A rate chosen for inference is the wrong rate for clearing a backlog, and
reading the frame as thirty-four sessions mistook one for the other.

What makes the read expensive per row is not the sentence; it is deciding what
to check and then checking it. The thirty-row entry says so in as many words:
**"Twenty of the twenty-eight that held were checked mechanically, not
re-argued."** So the mechanical half was scripted and the whole frame was read.

## Alternatives rejected

**Stamping the bucket mechanically** — a script that resolves each claim's
citations and marks the row read when they all resolve. Rejected because a
token that resolves says the claim's *subject* is present, not that the claim
about it holds, and the two false rows here would both have passed it: their
claims cite no path at all. A stamp a script can apply is a stamp that means
nothing, which is the rubber-stamp pressure `caveats.py`'s own docstring argues
against one level up.

**A new field for a machine-assisted read**, so `checked` could keep meaning
"a person read this" and the scripted part could be counted separately.
Rejected because the distinction it draws is not the one that matters: every
read in every campaign here has been a person plus a grep, and inventing a
third stamp beside `checked` and `reviewed` would make the tracker's data model
harder to explain for a difference of degree. What is recorded instead is this
entry, which says how deep the reading was.

**Reading it in thirty-row sessions as the earlier entries did**, to keep the
rate comparable across passes. That is the honest way to keep *measuring* the
false rate, and it is the wrong way to clear a backlog the measurement has
already characterised — five campaigns agreeing on "tens, not hundreds" is the
answer to the question sampling was asked. The cost of abandoning it is
recorded below and is real: this pass's rate cannot be pooled with theirs.

**Leaving it.** The user asked for it closed, and the argument for leaving it
was an estimate, not a principle.

## Evidence

1016 rows read, 2 false, **0.20%**.

**That rate is not comparable with the campaigns' 7 of 246 (2.8%), and must not
be pooled with it.** A fourteen-fold difference between two samples of the same
bucket is not a fact about the bucket; the overwhelmingly likely explanation is
reading depth. A thirty-row pass re-derives each claim; this pass read each
claim and its recorded reasoning, took the assembler's evidence, and reached
for a targeted `git grep` where the evidence was ambiguous — perhaps thirty
times across the 1016. A faster read finds fewer. The useful statement is the
absolute one: **two more false verdicts exist and are now closed**, and the
next pass should treat 2.8% rather than 0.20% as its prior.

The two found:

| claim | checked | what is true now |
| --- | --- | --- |
| "The ordering assertion only pins `#playground` before `#what`" | `grep -c playground site/index.html site/workbench.html` | 0 and 0; the sections are gone with the landing page |
| "The default-column assertion checks the option label ends in `·idx`, which is the marker `describeTable` writes" | `git grep describeTable` | nowhere in the tree; `site/check/workbench.py` asserts `.tree-columns .mark.idx` by class |

Both have the shape every false verdict this repository has found has had:
**true when written, overtaken later.** `site/check/playground.py` was deleted
by `git log --diff-filter=D -- 'site/check/*'` → `5ffc331`, "The home page is a
workbench, not a landing page". Two of the entry's four caveats were already
`moment`; the two `deliberate` ones were the ones that read as current.

`python3 scripts/check_draws.py` reproduces the draw and confirms all 1014
remaining stamps name it. `python3 scripts/caveats.py` reports 1045 deliberate,
1045 read against the tree.

Three mutations against `GONE`, all caught
(`ledger/mutations/20260930T155201-scripts-check-caveat-citations-py.json`):
excusing a path keyed to any entry is caught by *a GONE entry keyed to another
entry does not excuse this one*; dropping the came-back rule by *a GONE entry
whose path is back in the tree is refused*; dropping the unused rule by *a GONE
entry nothing cites any more is refused*.

Otherwise no mutation run: `read_deliberate.py` writes nothing and decides nothing, and
the rest of the change is `docs/caveat-status.json` rows, which
`scripts/test_caveats.py` and `scripts/check_draws.py` cover and whose guards
carry their own real-tree mutations.

## What this does not do

**The rate this pass measured is not usable.** Two of 1016 is a count, not an
estimate: the reading depth varied across the pass and is not recorded per row,
so nothing here supports an interval. The campaigns' 2.8% remains the best
estimate of how many `deliberate` verdicts are false, and this pass did not
test it.

**It does not grade the arguments.** "Is this claim false" and "is this a good
decision" are different questions, and only the first was asked — the same
limit `ledger/2026-09-29-the-deliberate-sample-carried-to-216.md` records as
the ceiling on the whole exercise.

**`read_deliberate.py`'s later-entry signal is unvalidated.** It ranks nothing
and was not tested against known-false rows, because the two known-false rows
from the 2026-09-30 draw are worded in ways it does not match — so whether the
signal would have found today's two is unknown. It was used as a place to look
harder, which is all it claims.

**The frame is empty, which makes `--unchecked` silent.** Until somebody passes
a number of days it lists nothing, and a `deliberate` verdict written tomorrow
is the only thing that will appear. The summary line still prints
`1045 deliberate (1045 read against the tree)`, so the number moves when the
bucket grows; that it stays equal is what a reader has to notice.

**`GONE` is a hand-written roster with a hand-written reason**, which is the
cost every roster in `scripts/` states about itself: the rot rules hold the
row to being used and to naming something absent, and nothing reads the
sentence. It has one entry, which is the state in which a roster is least
worth having and most likely to be right.

**Nothing re-reads on a timer.** `--unchecked 30` exists and nothing runs it.
Putting settled verdicts on an expiring worklist is the thing
`ledger/2026-09-27-a-hundred-and-three-caveats-decided-and-one-that-was-already-false.md`
argues against, and a thousand rows expiring monthly is exactly the pressure it
names.
