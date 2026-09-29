# Forty-eight more `deliberate` verdicts, none false, and the "Nothing…" lead got weaker rather than stronger

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`
- **Kind:** docs

## What changed

The seeded sample carried from 216 to **264 of 1011**. **No false claim.** The
`by` on the tracker's "792 remain unread" row is refreshed to 747, which is what
it now is.

Nothing else moved. The verdicts read are listed below; the point of the entry
is the arithmetic and one withdrawal.

## Why

`ledger/2026-09-29-the-deliberate-sample-carried-to-216.md` closed by proposing a
lead and promising a test:

> Three of the four false claims open with a negative existential … If the rate
> among "Nothing…" claims is really several times the rest, the next 100 reads
> should show it, and the way to check is to keep drawing from the unstratified
> sample and tally the shape.

This is the first 48 of those reads, drawn the same way, and they are the
evidence the previous entry said to go and get. Reporting them only if they had
confirmed the lead is the shape of error this repository's `docs/correctness.md`
is full of.

## Alternatives rejected

**Wait until the promised 100 reads before saying anything.** The entry named
100 as the point at which the lead would be *settled*, not the point at which it
becomes reportable. Forty-eight reads that move the probability the wrong way are
a result now, and holding them until they are joined by fifty-two more is how a
number gets quoted for a week while its own author is unsure of it.

**Fold this into the 216 entry rather than writing a second one.** That entry is
committed and a ledger entry is a dated record — the argument
`ledger/2026-09-26-batch-eight-and-a-caveat-that-bit-within-the-hour.md` makes
for why its counts are allowed to go stale is the same argument against editing
it now. The tracker is where a number is kept current.

**Read another 48 first, so the entry has a rounder sample.** Two batches with
one write-up costs nothing and would have hidden that the first batch alone
already moved the estimate. The batch boundary is where the reading happened.

## Evidence

Clopper-Pearson throughout, the method the three earlier entries use:

| sample | false | rate | 95% interval | implied, over 1011 |
| --- | --- | --- | --- | --- |
| first 72 | 2 | 2.8% | 0.3% – 9.7% | 3 – 98 |
| first 168 | 2 | 1.2% | 0.1% – 4.2% | 1 – 43 |
| first 216 | 4 | 1.85% | 0.5% – 4.7% | 5 – 47 |
| **all 264** | **4** | **1.52%** | **0.4% – 3.8%** | **4 – 39** |

A quarter of the bucket has now been read. The upper bound has come down from
98 to 39 across the four passes, and the shape of the estimate has stopped
moving: every pass since 168 says *tens, not hundreds*.

### The lead, weakened

Of the 48 read here, **11 were negative existentials** — "Nothing measures what
a window costs in the browser", "No bulk path", "No end-to-end confirmation
that the new constant chooses better", "Nothing re-runs the committed specs",
"Nothing here watches the branch", among others. **None was false.**

Over the whole 264, 57 (22%) carry the shape, up from 19% at 216 because this
draw was richer in them. So the probability the previous entry computed moves
the wrong way:

| after | negatives read | P(≥3 of 4 false are negative, if spread evenly) |
| --- | --- | --- |
| 216 | 42 (19%) | 0.025 |
| **264** | **57 (22%)** | **0.034** |

**The lead is weaker than when it was written, and I am saying so rather than
waiting for a draw that restores it.** It is not dead — 0.034 is still the sort
of number that makes a hypothesis worth another 50 reads — but the one test it
has had did not support it. Eleven high-risk claims read, zero false, is exactly
the observation that ought to make an author less confident, and the 216 entry
would read as premature if this batch went unreported.

The mechanism it proposed is still plausible and still untested: a negative
existential is falsified by anyone adding one instance anywhere. What 264 reads
cannot do is distinguish that from four false verdicts that happened to be
phrased in the commonest way a caveat is phrased.

### The forty-eight

Twelve were checked against the tree rather than read for reasoning. The ones
with a mechanical subject:

| claim | checked against | holds |
| --- | --- | --- |
| `SLF001` is not enabled | neither `pyproject.toml` selects it | yes |
| nothing measures what a window costs in the browser | no timing over a window in `site/check/workbench.py` | yes |
| no bulk path in the playground | `crates/slate-wasm/src/lib.rs` exposes `insert`, singular | yes |
| an array element cannot be a placeholder or a column | `crates/slate-serverd/src/lang/pred.rs`: `element := string \| number \| 'TRUE' \| 'FALSE'` | yes |
| nothing here is measured (the demo's batch) | `examples/batchbench` is where it is | yes |
| nothing re-runs the committed mutation specs | `run_mutations.py` is named in `ci.yml` as deliberately absent | yes |
| an array element is never nullable | `crates/slate-schema/src/row.rs`: "A null element is refused" | yes |
| `backend = "memory"` still prints no plan | `crates/slate-serverd/tests/plan.rs`, `a_memory_backend_says_it_has_no_stored_state_rather_than_inventing_one` | yes |
| no test sends a duplicated header over a real connection | no `authorization` anywhere in `crates/slate-serverd/tests/` | yes |
| a single-table alias is a refusal | `crates/slate-sql/src/sql.rs`, same reading as the 216 pass | yes |
| no client can send a nested predicate as SQL | no client has a SQL surface | yes |
| nothing watches the branch from this container | CLAUDE.md carries the two API calls instead | yes |

The other thirty-six were reasoning with no mechanical subject and were read for
whether the argument still applies — "a range is the honest form for that
finding", "three rules out `take(1)` and `take(2)`", "a shared budget needs a
policy object with state". All still apply.

## What this does not do

**No mutation, because nothing executable changed.** The only edit outside
`ledger/` is one `by` string in `docs/caveat-status.json`, and the guards that
read that file are mutation-tested already —
`ledger/mutations/20260929T201837-examples-retention-seed-py.json` and
`…201851-github-workflows-ci-yml.json` from the previous batch are the two most
recent. A mutation here would be testing `json.loads`.

**747 remain unread.** A quarter is not the bucket, and the lower bound of 4
still admits that almost all the false verdicts are in the part nobody has
looked at.

**It does not settle the lead, it reports one test of it.** Another fifty
negative existentials read with no false claim among them would settle it the
other way; fifty with three false would settle it the first way. Neither has
happened, and the honest state is "weaker than it looked, not resolved".

**Twelve of forty-eight were checked mechanically, thirty-six were read.** The
ceiling named in the previous entry has not moved: for a claim with no
mechanical subject, "holds" means one more reader agreed, and the reader is the
one who has been agreeing with these all day.
