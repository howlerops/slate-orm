# I read fourteen open caveats against the tree to find out how many were still true. Twelve were; two had been closed for days by work that named itself as the blocker. The tracker now has a worklist for the reading, because the reading is the only thing that can decide.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/caveats.py`, `scripts/test_caveats.py`, `docs/caveat-status.json`
- **Kind:** process, and one measurement

## What changed

A verdict may carry a `checked` date. `caveats.py --unread [days]` lists every
`open` caveat nobody has stamped inside that window — 277 of 284 today, which
is the honest starting position.

And one caveat moved from `open` to `closed`, having been verified stale.

## Why

Seven stretches of this session closed caveats and the `open` count did not
fall: 284 → 286 → 284 → 285 → 284. Every entry that closes one writes two or
three of its own, which is a property of the discipline and not a problem with
it — but it does mean the count cannot answer "is this list still true?".

`2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md` recorded exactly
that as **"Nothing re-triages"**: a caveat stays `open` whether or not the
thing it describes still exists, and the orphan mechanism catches a *reworded*
bullet rather than a claim that has quietly become false.

Three claims *of mine* turned out false today by that route — a disjunction no
client could send, a wire with no equivalent, a conformance runner with no case
— each written about a layer I had not opened. If a claim can be false the day
it is written, a claim four days old deserves a re-read.

## The measurement

**Fourteen open caveats, read against the current tree.** Eight sampled by
taking every nineteenth line of the open list, so they were not the ones I
remembered; six more chosen deliberately from the class the first eight
suggested — a caveat whose own text names what it is waiting for.

| | |
|---|---|
| still true | 12 |
| stale | 2 |

**The first stale one** is `2026-09-21-a-view-the-demo-can-show.md`: *"The Go and Node
adapters read through the view unchecked."* Its own text said the fix waited on
the generator — "the generator change above is the right fix and the reason
this waits for it" — and the generator landed the same day in
`ledger/2026-09-21-a-generated-view-declaration.md`. `main.go` now merges
`schema.Views` into its declarations and says so in a comment about `classics`;
`main.ts` merges the generated `VIEWS`. The caveat outlived its blocker by four
days.

**The second** is `2026-09-15-a-value-belonging-to-the-join.md`: *"The wasm
binding's `JoinSpec` still pins compute to the left table and aggregates to the
right."* Both halves are false. `JoinSpec::compute` names its side with
`ComputeSpec::input` and the field's own comment says "it used to be able to
read only the left, which was a limitation of this spec rather than of the
kernel"; `JoinSpec::aggregates` names its input with `AggregateSpec::input`.
Ten days.

**Two in fourteen.** Small enough that the open list is mostly honest, large
enough that ~40 of 284 are probably closed and nobody knows which. Fourteen is
a small sample and the interval on it is wide; it is what I observed and I am
not going to dress it up.

**Both stale caveats are the same shape**, and it is a shape worth naming: the
caveat says what it is *waiting for*, the thing it waits for ships, and nobody
walks back to the entry. Six of the fourteen were chosen for that shape after
the first one turned up, and it produced the second — so the shape is worth
more than a random draw, which is the practical advice this measurement
yields. Neither was detectable without reading: both are "X does not do Y"
where Y later shipped, and no property of the tree says so.

## Alternatives rejected

**Re-triage all 284 now.** It is the work that would actually move the count,
and at the rate the eight took it is hours of reading — most of it to confirm
what is already recorded. Worse, doing it under pressure to make a number fall
is how a caveat gets closed because the closing feels plausible, which is the
failure this whole tracker exists to catch and which I have committed five
times today. A dated worklist makes the reading resumable and auditable; doing
it in one sitting makes it a sprint nobody repeats.

**Detect staleness mechanically.** The tempting version: flag a caveat whose
claim names a path or symbol that no longer exists. None of the three stale
claims found today would have matched — they are all "X does not do Y" where Y
later shipped, and no property of the tree says so. A detector that catches
none of the observed cases is a detector that reports its own coverage.

**Expire a verdict automatically — `open` becomes `untriaged` after N days.**
It would force the reading, and it would also destroy the reasoning: an
`open` caveat carries no `by`, but a `deliberate` one does, and a scheme that
expires verdicts either loses that text or has to treat the two kinds
differently for no reason a reader would guess.

**Make `--unread` a failing check in `scripts/check.sh`.** Red today on 277
caveats, and the only way to green is to stamp 277 in an afternoon. That is the
pressure that produces a rubber stamp, and a stamp that means nothing is worse
than no stamp because it reads like attention.

## Evidence

**Mutations**, four run, four caught:

| mutation | caught by |
|---|---|
| closed and deliberate verdicts are listed as re-triage work | two cases |
| a recent stamp does not drop the caveat off the list | two cases |
| the window is ignored, so everything looks recently read | `a stamp older than the window is listed again` |
| an unreadable stamp is treated as a read | its own case |

**Tests.** `scripts/test_caveats.py`: **25 passed, 0 failed** (18 before), with
seven new cases covering both directions of the list — one read yesterday must
drop off, one nobody read must not.

**Against the repository.** 819 caveats: **284 open, 180 closed, 306
deliberate, 0 untriaged.** `--unread 30` lists 270, down from 285 as caveats are read and stamped.

## What this does not do

**It decides nothing.** A stamp says a person looked; whether they looked
carefully is exactly as checkable as whether the caveat was true when it was
written, which is to say not at all. The measurement above is the only thing
that says what a stamp is worth, and it says seven in eight.

**270 open caveats are still unread.** This is a worklist with one entry
crossed off and a rate to plan by, not a re-triage.

**Nothing makes the stamp expire on a change to the code the caveat is
about.** A caveat read today and invalidated tomorrow reads as current for a
month. Tying a stamp to a commit range would be better and needs the caveat to
name what it is about, which it does in prose and not in a field.

**The sample is fourteen, and six of them were not random.** Eight came from
every nineteenth line of the open list; six were chosen for the "waiting for"
shape after the first stale one suggested it. That makes 2/14 an *upper*
estimate of the rate rather than an unbiased one — the enriched half was picked
because it looked more likely to be stale, and it was. The honest reading is
1/8 from the random sample, with the shape as a way to find the rest faster.
