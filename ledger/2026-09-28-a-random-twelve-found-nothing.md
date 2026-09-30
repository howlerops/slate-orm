# A random twelve found nothing, which is the first evidence about the rate

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`
- **Kind:** docs

## What changed

Twelve open caveats drawn at random from the 99 that predate today, read
against the tree, and **all twelve hold**. No verdict moved.

The draw is reproducible: `random.seed(20260928)` over the `open` verdicts in
`docs/caveat-status.json` with today's entries excluded, `random.sample(…, 12)`.
Today's are excluded because a caveat written hours ago cannot have been
overtaken by work, so including them would pad the sample with guaranteed
passes.

| caveat | how it was settled |
|---|---|
| no `sum(price)` in the corpus | absence, confirmed twice |
| no estimate from a persisted count | absence |
| a conditional update is one round trip per row (two entries) | no batched conditional-update method on `Records` or in the proto |
| the probe covered `paged` and `returning` only | absence |
| the view guard does not check the property it relies on | absence |
| nothing bounds the number of values in an `IN` list | no such limit in `limits.rs` or the daemon config |
| the wasm join is inner | read: `JoinSpec` carries no join type |
| the regex note's other findings are unaudited | absence |
| `#[derive(Record)]` cannot declare a check's `column` or `message` | read: the derive has no `check` attribute at all, and the `column` hits are index columns |
| nothing stops the next mutation run adding unlabelled seeds | absence |
| nothing asserts the three clients agree on the group-key encoding | absence in the conformance corpus |

## Why

`ledger/2026-09-28-twenty-five-caveats-read-and-eight-greps-that-lied.md` left
a question open and said what would answer it:

> "Cheap to check correlates with recently touched" is a hypothesis fitted to
> four data points after the fact, and the obvious competing one — that the
> first pass was more careful because it was the first — fits them equally
> well. Distinguishing the two means reading a *random* twenty-five, which is
> the thing worth doing next and is not what either pass did.

Twelve rather than twenty-five, and the result is one-sided but real: **the
4-in-25 rate of the cheap sample did not reproduce in an unbiased draw.** Both
passes were done the same way by the same session within an hour, which weakens
the "more careful the first time" explanation and leaves selection as the
likelier one — the first pass went looking where the tree had moved, and found
what it went looking for.

**What this does not license is "the open list is clean."** Zero failures in
twelve is consistent with a true rate anywhere up to about 22%, at the usual
one-sided 95% reading of `1 − 0.05^(1/12)`. It rules out a *high* rate, not a
moderate one. The honest summary of both samples together is: the open list is
in better shape than the accidental discovery of five stale caveats suggested,
and nobody has bounded how much better.

The reproducible seed is the part worth keeping. A sample somebody can redraw
is a sample somebody can extend — the next session can raise 12 to 40 and get a
tighter bound without re-litigating which caveats were chosen or arguing about
whether the choice was fair.

## Alternatives rejected

**Draw twenty-five, as the previous entry proposed.** Twelve reads took most of
an hour of tool calls and twenty-five would not have fitted beside the CI work
this session also owed. Twelve with the seed recorded is extensible; twenty-five
abandoned halfway is not. The previous entry's number was a guess at what would
be conclusive and nothing made it the right one.

**Include today's caveats in the population.** They are open, so they qualify,
and they cannot be stale — the work they describe was left undone hours ago.
Including them would have inflated the pass rate by construction, which is the
same error as choosing the cheap ones and inflating the failure rate.

**Report "0/12" and leave the reader to interpret it.** A zero with no interval
reads as proof, which it is not. Stating the 22% bound costs one sentence and
stops the next reader quoting this entry as evidence the backlog is clean.

**Close the "does not explain why the four clustered" caveat.** Narrowed rather
than closed: the sample discriminates between the two hypotheses but does not
settle either, and calling a weak discrimination a closure is how a tracker
stops meaning anything.

## Evidence

- The draw, reproducible: `random.seed(20260928)` over the 99 pre-2026-09-28
  `open` verdicts, `random.sample(…, 12)`. The twelve are tabulated above.
- Nine settled by conclusive absence — a grep over every place the feature
  could live returning nothing. Three settled by reading the file: the wasm
  binding's `JoinSpec`, `crates/slate-derive/src/lib.rs` for the missing
  `check` attribute, and `crates/slate-orm/src/ext.rs` plus the proto for a
  batched conditional update.
- `python3 scripts/caveats.py`: 0 untriaged, and no verdict changed by this
  entry beyond its own three.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**It reads twelve, so the interval is wide.** 0/12 bounds the rate below ~22%
and no lower. Forty would bring that to ~7% and is the obvious next increment;
the seed is recorded so it can be a continuation rather than a new sample.

**It does not re-examine the four that moved.** They are closed and narrowed
with witnesses, and this entry's question was about the *rest* of the list, not
about them. Whether any of the four was wrongly moved is a different audit and
`scripts/check_closed_caveats.py` is the only thing currently watching them.

**It says nothing about the `deliberate` verdicts.** 889 of them, far more than
the open ones, and a `deliberate` that has stopped being a defensible decision
looks exactly like one that has not. No sample has ever been drawn from that
population and this entry does not draw one.
