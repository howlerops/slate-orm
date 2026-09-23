# #288 found the stale sentence, declined to invent a measurement to check it against, and stopped there. The other half was never done: that phrasing was still silently accepted.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #291 (F6k)
- **Touches:** `scripts/{check_cost_prose.py,test_check_cost_prose.py}`, `README.md`
- **Kind:** converting an uncheckable class into a checkable one, by refusing the uncheckable phrasing

## What changed

A claim about a *constant's error* — `POINT_READ_COST is 13–19% low` — is now
refused rather than ignored. The message says what to do:

```
README.md:1296 says POINT_READ_COST is 13–19% low, which states an error
rather than a value, so nothing can check it.
  Restate it as the value — `a point read costs 1 request` — so the patterns
  here read it, or write `<!-- not a cost-model claim -->` above it if it is
  narrating history.
```

It is not counted in `seen`. That number says how many claims were *verified*,
and this one cannot be.

## Why

#288 was right that no pattern can verify "13–19% low": checking it needs the
measurement the error is against, and hard-coding a measurement into the file
whose job is catching stale restated numbers is the defect, not the fix. What
that reasoning does not license is *accepting* the sentence. The one that
existed sat unstruck in `README.md` while `docs/performance.md` carried the
same words struck through, and only a person reading both noticed.

Refusing it converts the class by construction: an author either writes a value
the guard reads, or marks the passage. That is the trade this repository makes
everywhere else — `expect_survivor`, `EXPECTED_REFUSALS`, the frozen table
roster. A list you are forced to edit is a list that stays true.

## Alternatives rejected

**Verify it after all**, by recording a measurement to compare against.
Rejected for #288's reason, unchanged: the measurement would go stale, inside
the file that exists to catch things going stale.

**Warn instead of failing.** A warning in a 47-step check is a line nobody
reads. The escape hatch is one comment, and writing it is the author saying
out loud that the sentence cannot be machine-checked — which is the whole
value.

**Match the general shape**, any percentage beside any word like "low".
`update_many` is "~7% off", the ClickBench page reports "30% higher
throughput", a grouped query has "~1% overhead" — most of this repository's
prose is percentages. The pattern is anchored on a constant's name within one
sentence, and the sentence boundary is load-bearing: `[^.\n]` is what stops a
constant stated checkably in one sentence and an unrelated percentage in the
next from reading as one error-claim. That case is a test.

**Mark the README sentence and move on.** This was the first attempt and it
exposed something worth recording: the marker is *chunk*-granular, and the
README bullet holding the historical narration also holds the live checkable
claim "a point read costs 1 request". Marking it would have silenced both.
The narration was removed instead — a checklist of open work is the wrong
place to re-narrate a withdrawn figure, and the ledger already holds it.

## Evidence

Five mutations, recorded in
[`ledger/mutations/20260923T084319-scripts-check-cost-prose-py.json`](mutations/20260923T084319-scripts-check-cost-prose-py.json),
and the re-run after the missing case was written in
[`ledger/mutations/20260923T084353-scripts-check-cost-prose-py.json`](mutations/20260923T084353-scripts-check-cost-prose-py.json).
Four caught immediately; the fifth — dropping the en dash from the range —
**survived**, and the reason is worth keeping: without it the
pattern still fires, on the `19%` alone, so only the text quoted back in the
message changes. Counts could not see it. A case asserting the message quotes
the whole range catches it, and a message that misquotes the sentence it is
complaining about sends the author hunting for something they did not write.

Seven cases over the shape itself: both constants, a single figure, a hyphen
range, an en dash range, a marked one, a struck one, a percentage that is not
about a constant, and the full-stop boundary.

`sh scripts/check.sh`: **47 passed, all of them.** Claims verified: 25,
unchanged — this adds no verified claims, by design.

## What this does not do

**It reads one phrasing.** `is N% low` and its neighbours, anchored on a
constant's name. "half what it should be", "out by a third", "nearly double"
are all the same unverifiable class and none is caught. The pattern was built
from the one sentence that actually went stale, not from a survey of how one
could be written.

**It cannot tell history from a live claim** — no pattern can, which is why
both resolutions are manual. A marked sentence is trusted exactly as far as
the person who marked it.

**The marker is still chunk-granular**, so a passage mixing a checkable claim
and an unverifiable one cannot exempt only the second. That is a real limit,
met in this task and worked around by rewriting rather than by changing the
marker's scope; a line-granular marker would be the fix if it happens again.

**It guards `crates/`, `docs/` and the READMEs**, the trees from #288.
`site/` still holds no cost claim and is still unread.
