# I tried twice to find stale caveats without reading them. Both heuristics found nothing the reading had not already found. The answer to "how do we finish the list faster" is that there is no faster.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json` (four more verdicts stamped)
- **Kind:** process — a null result

## What changed

Four more open caveats read against the tree and stamped `checked`. Nothing
else: the two mechanisms this entry is about were written, run, and thrown
away.

`--unread 30` is 267, from 285 when the stamp was added.

## Why

`2026-09-25-the-open-caveats-nobody-re-reads.md` measured the rate — fourteen
open caveats read, two stale — and named the shape the two shared: **a caveat
whose own text says what it is waiting for**, where the thing it waits for
shipped and nobody walked back to the entry. That looked mechanisable, and a
detector would turn ~280 careful reads into a short list.

It is not mechanisable. Here is what was tried.

## Heuristic 1: the "waiting for" phrase

A regex over each open caveat's own paragraph for the language of a blocker —
`waits for`, `is the right fix`, `needs X first`, `blocked on`, `when X lands`,
`would close that`, `is one line`, `a later task`.

**Four hits out of 267, and all four false.** Every match was an incidental
`until` or `a later task` inside a sentence about something else — "nothing
says so until the `update` is refused", "a later task happened to touch it".
Neither of the two genuinely stale caveats would have matched: one said "the
generator change above is the right fix and the reason this waits for it",
which *does* match, and it is already closed; the other said nothing about
waiting at all.

So the shape is real in the two examples and not expressible as a phrase. A
person reading "the reason this waits for it" understands a dependency; a
regex finds the same words in four sentences that carry none.

## Heuristic 2: a later entry citing an older one

Better reasoning behind it: the work that closes a caveat usually writes an
entry, and that entry usually cites the one it closes. So an entry with open
caveats *and* newer entries citing it is where staleness should concentrate.

**26 such entries. Four caveats checked from the top of that ranking, none
stale.** The citations turn out to be mostly of the other kind — a later entry
citing an older one for *context*, for a rule it follows, or for a measurement
it builds on. `2026-09-19-a-purge-something-can-call.md` is cited by two later
purge entries and its remaining open caveat is "no offline tool", which none of
them touched.

The ranking is not useless — it is where I would look first, and one of the two
known stale caveats does sit in it. It is just not a filter: four from the top
gave nothing, and reading four random ones gives the same nothing seven times
in eight.

## Alternatives rejected

**Keep going: a third heuristic.** The two that failed are the two with a
plausible mechanism behind them. A third would be fishing, and a detector that
fires on nothing looks exactly like a clean tree — the failure this repository
names most often.

**Ship heuristic 2 as a `--likely-stale` flag.** It would be a list somebody
trusts. Its hit rate is indistinguishable from random in the only sample taken,
and putting it in the tool would make that indistinguishability invisible.

**Say nothing, since nothing was built.** `CLAUDE.md`: "A null result stated
plainly is worth more than a manufactured finding." The next session to have
this idea should find out here that it has been had, and what it cost.

## Evidence

| heuristic | candidates | checked | stale found |
|---|---|---|---|
| the "waiting for" phrase | 4 of 267 | 4 | 0 |
| later entries citing an older one | 26 entries | 4 | 0 |
| reading, sampled every nineteenth line | — | 8 | 1 |
| reading, picked for the "waiting" shape by eye | — | 6 | 1 |

**Eighteen open caveats have now been read against the tree; two were stale.**
Both were found by a person reading, one of them from a shape a person
recognised and a regex could not.

The practical number: at ~1 in 8, and 267 unread, roughly **33 of the open list
are probably closed** and finding them costs 267 reads. There is no cheaper
path, and this entry is the evidence for that rather than an assertion of it.

## What this does not do

**It does not prove no detector exists.** It reports two that do not work, on
a sample of eight checks between them. A third idea might be better; the
evidence here is only that the two obvious ones are not.

**Four caveats were checked per heuristic, not all of their candidates.**
Checking all 26 entries in heuristic 2's ranking would give a real hit rate
rather than "none of the four I looked at". I stopped because four consecutive
misses from a ranking that was supposed to concentrate them is enough to stop
trusting the ranking, and that is a judgement rather than a measurement.

**267 open caveats are still unread**, which is the actual state and the thing
no heuristic changed.
