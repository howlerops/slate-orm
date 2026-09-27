# `narrowed` rows carrying `reviewed` were never listed by `--unread`. Not stale — never listed at all, which is the failure a staleness report cannot show you.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/caveats.py`, `scripts/test_caveats.py`, `docs/caveat-status.json`
- **Kind:** correctness

## What changed

`caveats.py` refuses a `narrowed` verdict carrying `reviewed`. Five rows that
did now carry `checked` instead.

## Why

The tracker has two date fields and its own docstring says which goes where:

> `checked` on an `open` or `narrowed` row says somebody read the caveat
> *against the tree* and believes it is still true. That is what `--unread`
> reads.

There was a rule enforcing the other half — a settled verdict may not carry
`checked`, because that is a worklist stamp on something nobody will look at
again — and none enforcing this one. Five `narrowed` rows of twenty-four
carried `reviewed`, so `--unread` skipped them at every interval. Two of the
five were written in this session, half an hour before this, by a hand that
had just read the docstring.

The failure mode is worth naming precisely. A row with a stale `checked` date
appears in the report and nags; a row with **no** `checked` date at all never
appears, so the report says "0 not re-read in 30 days" and is telling the
truth about a set with five holes in it. A worklist that silently omits rows
is worse than one that is behind, and the report itself cannot show you the
difference — which is why this needed a rule rather than a sweep.

## Alternatives rejected

**One date field, and infer from the verdict.** The two fields exist because
the 2026-09-26 reverse sweep stamped `checked` on 316 `deliberate` rows and
made `--unread`'s answer meaningless; splitting them is what fixed that, and
merging them back would undo it. The problem was never two fields, it was a
rule written for one direction only.

**Sweep the five and move on.** What the first draft of this did, and it
leaves the sixth to be written tomorrow. The docstring already said the rule;
saying it again in an entry is the same non-enforcement with more words. The
rule costs six lines and two cases.

**Have `--unread` list a `narrowed` row with no `checked` at all.** It would
surface the five, and it would also surface every row on the day it is
narrowed, because a verdict is written before it is re-read. The refusal
happens at the moment the row is written, which is where the information is.

## Evidence

**The count, before and after.** With the five rows still wrong,
`python3 scripts/caveats.py --unread 30` reported five — and none of those
five were these, because a row with no `checked` cannot be stale. Converting
all five to `checked` took the report to **0**, which is the number that made
the problem obvious in the other direction: two of the five dates I had
written myself minutes earlier, and three I could not vouch for, and a report
of zero was now asserting that all twenty-four narrowed residuals had been
read against the tree.

So the three I could not vouch for lost their dates rather than gaining a
field. The report now says **3**, and names them:

```
2026-09-23-an-unverifiable-claim-is-refused-now.md: It guards `crates/`, `docs/` and the READMEs
2026-09-24-a-guard-said-it-did-not-read-the-tree-it-reads.md: Nothing checks a guard's account of itself.
2026-09-26-the-guard-that-only-knew-one-phrasing.md: It still cannot tell whether the numbers in the table are the record's numbers.
```

Three rows on a worklist is a worse-looking number and a truer one. The two
that kept their dates are the two this session narrowed and verified today.

The distribution that found it, over `docs/caveat-status.json`:

| verdict | `checked` | `reviewed` | neither |
| --- | --- | --- | --- |
| open | 183 | 0 | 0 |
| narrowed | **19** | **5** | 0 |
| closed | 0 | 45 | 175 |
| deliberate | 0 | 453 | 0 |
| moment | 0 | 7 | 65 |

Nineteen against five is a convention with exceptions, which is what a rule is
for. `open` at 183–0 is the same convention already holding perfectly, because
`open` has no `reviewed` spelling anybody would reach for.

**Mutations.** Three cases, no survivors:
`ledger/mutations/20260927T010554-scripts-caveats-py.json`.

| mutation | outcome |
| --- | --- |
| the rule never fires | caught |
| the rule reads `checked` rather than `reviewed` | caught, both new cases |
| the rule applies to every verdict, not only `narrowed` | caught by the settled-verdict case |

The third is the one the pair of cases exists for: a rule spelled
`if row.get(REVIEWED)` passes the refusal case and breaks every `deliberate`
and `closed` row in the file, which is 498 of 952.

**Suites.** `scripts/test_caveats.py` 37 passed 0 failed (was 35).
`sh scripts/check.sh` 69 of 69.

## What this does not do

**It does not check that a `checked` date is true.** Nothing can: the field
says somebody read the caveat against the tree, and the only evidence is that
they wrote the date. `--unread` making the claim expire is the whole
mechanism, and an agent stamping today's date on a row it did not read defeats
it completely. Deciding which three of the five dates to drop needed this
session's memory plus `git show HEAD:docs/caveat-status.json`, and a future
sweep will have neither — which is the argument for expiry rather than for a
better field.

**`closed` and `moment` rows may still carry neither date.** 175 and 65 of
them do. That is not the same defect — a settled verdict needs no worklist
stamp, and `reviewed` on one means "somebody re-read the *judgement*", which is
optional. But it means the two fields have three states between them and only
two are enforced, and whether a `closed` row with no `reviewed` has ever been
re-read is not recorded anywhere.

**Three residuals are now on the worklist and have not been re-read.**
Dropping their dates puts them where they belong; it does not do the reading.
Whoever picks them up should expect the 2026-09-24 one in particular to have
moved, since two guards have been widened since it was written.
