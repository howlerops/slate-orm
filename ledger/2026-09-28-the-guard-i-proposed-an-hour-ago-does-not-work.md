# The guard I proposed an hour ago does not work, measured

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`
- **Kind:** docs

## What changed

Nothing is built. `ledger/2026-09-28-the-first-sample-of-the-deliberate-verdicts.md`
proposed, an hour ago, the mechanism that would scale:

> a `deliberate` verdict could carry the paths its reasoning depends on, the
> way a `closed` one carries a witness, and a guard could report when one of
> those paths changed since the verdict's date. That would have flagged this in
> H7's own commit.

It was going to be bootstrapped for free, because `check_caveat_citations.py`
already extracts paths from a verdict's `by` text — no schema change, no
roster. Measured before building, and the free version does not work.

| | |
|---|---|
| `deliberate` verdicts | 891 |
| carrying a date and citing at least one path | **78 (9%)** |
| of those, a cited path changed after the verdict's date | **23** |
| of the 23, citations that are a *dependency* rather than an *argument* | **0** |

## Why it fails

Read all 23 and the pattern is uniform: **a `deliberate` verdict cites paths as
evidence for an argument, not as the subject whose state decides it.**

- "equivalent-mutation detection is undecidable — `scripts/mutate.py` says so"
- "undecidable, per `scripts/check_retired_claims.py`"
- "`CLAUDE.md` requires `scripts/mutate.py`; a hand-run mutation is the thing it
  tells you not to do"
- "the coarse-witness trade is argued at length in
  `scripts/check_closed_caveats.py`"

`mutate.py` changing does not make undecidability decidable. The citation
points at where an argument is written down, and arguments do not go stale when
the file holding them gains a feature. Twenty-three of twenty-three are this
shape.

The one verdict that *was* stale — found by the random sample, not by this —
cited its path the other way: "`check_cited_docs.py` not walking `site/` is
recorded rather than fixed", where the path's behaviour **is** the decision. It
would have been in the flagged set, which is the whole of the case for the
idea. So the comparison is:

| method | read | stale found |
|---|---|---|
| random sample of `deliberate` | 15 | 1 |
| everything the flag would raise | 24 | 1 |

**The flag is no better than reading at random**, and costs more reads. That
kills it. Not "needs tuning" — the signal it uses is the wrong signal, because
the field it reads was written for a different purpose.

## Alternatives rejected

**Tune it with an exclusion roster** for the paths that churn —
`docs/caveat-status.json` changes on every triage, `scripts/check.sh` on every
new step. It would cut 23 to about 20 and change nothing about the ratio,
because the churn paths are not what makes the flags false. The argumentative
citations are, and they are in `crates/`, `scripts/` and `docs/` alike.

**Add the `depends` field properly** — a real dependency list per verdict,
separate from `by`. This is still the design that would work, and the
measurement above is now the argument for why it cannot be bootstrapped: 848
verdicts would need the field filled in by hand, by somebody deciding for each
one whether its paths are arguments or subjects, which is the reading the guard
was supposed to replace. It is worth doing *going forward* — a new `deliberate`
verdict can carry `depends` at the moment somebody writes it, when the
distinction is free — and that is a convention rather than a migration. Not
adopted here because a convention nothing enforces is one this repository has
been burned by, and enforcing it means refusing verdicts without the field,
which means the 848.

**Ship it as a report rather than a failure.** A guard that prints and does not
fail is one nobody reads; `scripts/check.sh`'s own design argues this. And a
report with a 23-to-1 false-positive rate would be ignored by its second
reader.

**Say nothing and move on.** The proposal is an hour old and in a committed
entry that calls it "the version that scales". Leaving that standing while
knowing it does not is the exact failure this session has been cataloguing all
day: a claim written confidently, contradicted by a later measurement, and left
where the next reader will act on it.

## Evidence

- The counts in the table, from a script over `docs/caveat-status.json` using
  the same path regex `check_caveat_citations.py` uses, against
  `git log -1 --format=%cs -- <path>` for each cited path.
- All 23 flagged verdicts read for their `by` text. Three also verified against
  the tree, and all three hold:
  - "`reviewed` is not enforced on a settled verdict, only permitted" —
    `scripts/caveats.py` enforces the *converse*, that a settled verdict must
    not carry `checked`. A different rule; `reviewed` is still optional.
  - "Scope is the only part of a docstring compared" — the `by` cites
    `check_cited_docs.py` as one of two guards that already exist for the
    checkable parts, and widening it today did not stop it existing.
  - "The `headbench-smoke` witness is coarser than the closure" — the witness
    is still `("crates/slate-headbench/run.sh --smoke", ".github/workflows/ci.yml")`.
- The other twenty were read for reasoning only, not verified against their
  subjects. Their `reviewed` dates are **not** bumped: a date bumped on the
  strength of reading the verdict's own prose is a rubber stamp, and
  `caveats.py`'s `unread()` docstring argues against exactly that pressure.
- `sh scripts/check.sh`: 72 passed, all of them.
- `python3 scripts/caveats.py`: 1588 caveats, 124 open, 65 narrowed, 374
  closed, 892 deliberate, 0 untriaged.

## What this does not do

**It leaves the 833 unsampled verdicts unsampled**, and now with one fewer idea
for how to reach them cheaply. The honest position is that the only method
known to work on that population is reading it, at fifteen per pass.

**It does not adopt the going-forward convention.** Writing `depends` on new
`deliberate` verdicts costs nothing at the moment of writing and would compound;
not doing it means this entry's own three verdicts below are as unreachable as
the 848. Left undone because a convention with no enforcement decays, and the
enforcement is the part that needs the migration.

**The three-of-twenty-three verification is thin.** Three is enough to show the
flags are not obviously stale and not enough to say the other twenty are sound.
If somebody wants the flagged stratum's true rate, twenty more reads is the
price, and this entry's argument is that those twenty reads would be better
spent on a random twenty.
