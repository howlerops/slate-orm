# The `deliberate` sample carried from 72 to 168, and what the second half found

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

`ledger/2026-09-29-seventy-two-deliberate-verdicts-read.md` read 72 of the
`deliberate` verdicts and found two false. This carries the same seeded sample
to **168** — ninety-six more, in four batches of twenty-four, from the same
draw in the same order, so the two together are one sample and not two.

The second half found **no false claim** and **one stale reason**:

`2026-09-19-one-command-for-the-checks.md` :: *"It does not run the suites, and
the suites are where the defects are."* The claim is true. Its `by` said *"the
closing summary names the nine it skips"*, and that summary now names eleven.
The `by` is corrected and the verdict stays `deliberate`.

## Why

72 is enough to say the bucket rots; it is not enough to say how fast. The
first entry put the rate at 2 in 72 with an interval running from about 0.3% to
9.7%, which is the difference between "three verdicts across the bucket" and
"a hundred". Doubling the sample was the cheapest thing that narrows it.

**It narrows it downward, and that is the finding.** 2 in 168 is about 1.2%,
interval roughly 0.1% to 4.2% — so the bucket is very likely holding tens of
false verdicts rather than the hundred the first interval admitted, and the
first half's 2.8% was the high end of a small sample rather than the rate.

The second finding is a third failure mode, which the first entry did not have
a name for. The two false verdicts were *claims* overtaken by later work. This
one is a **reason** overtaken while its claim stayed true: the sentence
justifying `deliberate` counts something, the count moves, and the verdict is
now resting on an argument that is off by two. Nothing catches it, because
`caveats.py` reads the verdict and the dates and never the arithmetic inside a
`by`.

That is the same class CLAUDE.md records about itself — its "twenty-two jobs"
sentence said *seventeen* for as long as it did, for want of anything counting.
A number written into prose beside a thing that grows is a number that will be
wrong, and the ledger now has an instance of it inside the tracker that is
supposed to police the ledger.

## Alternatives rejected

**Stop at 72 and quote the rate.** What the first entry's own "What this does
not do" invited. Not taken because the interval it reported was wide enough to
be quoted in two incompatible ways, and the second half cost four batches of
reading against a number that will be repeated.

**Re-file the stale reason as `open`.** The claim — `check.sh` does not run the
suites — is true, deliberate, and argued. Only the supporting count drifted.
Moving it to `open` would put a non-task on the open list to record that a word
in a sentence needs changing, which is how an open list stops being read.

**Write a guard that counts the items in that sentence.** It is one specific
number in one specific `by`, and the general version — checking arithmetic
inside free prose — is not a program. `scripts/test_check_sh.py` already holds
`check.sh` to `ci.yml` step by step, which is the part a tool can hold; the
sentence is narration beside it. Recorded as a limit rather than automated
badly.

**Carry on to 336.** The next doubling narrows the interval by about a third
and costs what this one did. Not taken here because the decision this feeds —
"is `deliberate` worth re-reading as a habit" — is already answered yes, and
further precision changes nothing anybody would do.

## Evidence

168 verdicts read in fourteen batches, from a fixed seed over the 993 with a
live bullet: **2 false claims and 1 stale reason**, all three in the first 144.

| sample | false claims | rate | 95% interval |
| --- | --- | --- | --- |
| first 72 | 2 | 2.8% | ~0.3% – 9.7% |
| all 168 | 2 | 1.2% | ~0.1% – 4.2% |

Across 993 that is about **12 verdicts** at the point estimate and about 42 at
the top of the interval. "Tens, not hundreds" survives the doubling; "a few per
cent" does not, and is withdrawn in favour of about one.

Fourteen more claims were checked mechanically rather than read, all holding:

| claim | checked against | holds |
| --- | --- | --- |
| `self.table(` is deliberately out of the handler rule | `scripts/check_handlers.py`, the absence is documented and measured | yes |
| the workflow guard reads only the top-level `env:` | `scripts/test_check_sh.py` | yes |
| `Optional[X]` and `Union[None, X]` occur nowhere in the tree | the only match is inside `scripts/check_none_last.py`'s own docstring | yes |
| `trips` has one secondary index | `crates/slate-wasm/src/taxi.rs`, `by_pickup_zone` alone | yes |
| no window over a join or a chain | `crates/slate-server/proto/slate/v1/records.proto`, `repeated Window` is on `Query` only | yes |
| the citation rule checks no `#anchor` | no `.md#section` citation exists in the tree | yes |
| a mutations cron exists but no *purge* scheduler ships | `.github/workflows/mutations.yml` schedules mutations, not a purge | yes |

The last row is worth separating out, because it is the one that looked like a
second instance of the first entry's finding and is not. That entry's false
verdict said *"there is no cron, no queue, nothing that runs between
sessions"*, which `mutations.yml` falsified. This one says an operator wanting
a nightly purge writes their own cron against the RPC, and a workflow that runs
this repository's mutation suite is not a purge scheduler shipped to an
operator. Two claims a word apart, one false and one true; checking the second
against the artefact rather than against the memory of the first is the only
thing that separated them.

## What this does not do

**It does not re-read the first 72.** They were read once, by the same reader,
on the same day. A second pass by the same kind of reader is not independent
evidence, which `2026-09-28-the-invisible-backlog-is-triaged.md` already says
about this whole exercise.

**Most of the 168 were read for plausibility.** Twenty-six across the two
entries were checked against the tree; the remaining 142 are judgement calls
whose `by` is an argument, and finding one still reasonable is weak. The rate
is a lower bound.

**It did not look for stale reasons systematically.** The one found was noticed
because it happened to count something and the count was easy to redo. A `by`
that argues rather than counts cannot be checked this way at all, and those are
the majority.

**The interval is a normal approximation and 2 successes is few.** 0.1%–4.2% is
the shape of the uncertainty rather than a rigorous bound; an exact binomial
interval on 2/168 runs slightly wider at the top. The conclusion — tens, not
hundreds — does not turn on which is used.

**Nothing schedules the next sample.** This is the second reading of the bucket
in one day and there is no third planned, no `checked` field on a `deliberate`
row, and nothing that will notice when a year has passed.
