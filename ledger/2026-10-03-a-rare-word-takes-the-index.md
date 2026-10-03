# A rare word takes the index: `analyze` counts the rows holding each term

- **Date:** 2026-10-03
- **Author:** Claude Code (session: picking up from the handoff prompt, task T3)
- **Touches:** `crates/slate-kernel/src/stats.rs`, `crates/slate-kernel/src/plan.rs`,
  `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/src/lib.rs`,
  `crates/slate-kernel/tests/fulltext.rs`,
  `crates/slate-slatedb/examples/term_crossover.rs`, `scripts/run_examples.sh`,
  `scripts/mutations.json`, `docs/full-text.md`, `docs/orm-comparison.md`,
  `docs/agent-handoff.md`, `CLAUDE.md`
- **Kind:** performance

## What changed

`analyze` now counts, for every column a text index is on, how many rows hold
each term, and the planner reads those counts in three places: the selectivity
of a `contains`, the rows an inverted-index candidate is expected to admit, and
which of a search's terms the index is ranged on — the rarest, where it used to
be the lexicographically first.

The count is `TermCounter`, a Space-Saving heavy-hitter counter of 10,000
words with a k-minimum-values sketch of the vocabulary beside it, producing a
`TermStats`. Below 10,000 distinct words every count is exact. Above, every
word's estimate is within N/10,000 of the truth, N being the column's postings.

A new example, `term_crossover`, measures where the counted planner switches
and where the two access paths actually cost the same.

## Why

A non-covering index is chosen here when it is expected to return about one
row in 8,000 (`SCAN_ROW_COST / POINT_READ_COST`). Every search was estimated at
`TERM_SELECTIVITY`, a flat thousandth, so the planner scanned for every word —
including one held by a single row, which is the case an inverted index exists
for. `docs/full-text.md` §6 named the missing statistic as the first of two
things that would change the verdict, and `docs/orm-comparison.md`'s full-text
row named it as the one item still open.

## Alternatives rejected

- **Cost text specially**, cheaper per entry. The handoff said that if this
  were done, `the_planner_costs_a_text_index_like_any_other` must break
  deliberately. It was not done, so the test holds unchanged: counts change the
  *estimate* a search starts from, as a column's distinct count changes an
  equality's, and never the cost of the rows the estimate implies.
  `ascending_walk` had already measured that an inverted index's walk costs the
  same per row as an ordinary index's, so there was no measured basis for a
  separate cost.
- **A sample of rows**, as the histograms are built. A sample finds the common
  words and misses the rare ones. At a one-percent sample, a word in three rows
  is usually absent, and the counts would not tell a rare word from an absent
  one — the exact distinction a search needs.
- **Count every word exactly.** A text column's vocabulary routinely runs to
  millions, and `analyze` already bounds its distinct counts at 10,000 for that
  reason. Space-Saving degrades to exact counting whenever the vocabulary
  fits, so the bound costs nothing on the tables where exactness was possible.
- **Read the inverted index's own entries** instead of tokenizing during the
  scan. Exact, in bounded memory, from a walk already in term order. But
  `analyze` is an ordinary read under the caller's policy, and a raw index walk
  reads past it. The counts would describe rows the context cannot see, which
  is a different defect from the one the docstring warns about.
- **Use the counter's upper bound for a tracked word.** The conservative
  choice, since underestimating a common word is the expensive mistake. It was
  the worst of four rules measured against an exact count — mean absolute error
  10.3 rows on a 5,000-word stream, against 6.2 for the rule taken. That rule
  takes what the word was counted since it got its counter, plus the tail's
  mean if it inherited an error.
- **Divide the evicted mass by the untracked words only.** This was the first
  draft, and a mutation surviving was what showed it wrong. The evicted mass
  also pays for the earlier occurrences of every tracked word that inherited an
  error, so dividing by the untracked words alone counted the mass once and
  paid it out twice. On a 1,500-word vocabulary through 1,000 counters the
  draft estimated the tail at 34.7 rows against a true 15.8. The divisor now
  excludes only the words held since their first occurrence, and estimates
  15.9.
- **Floor the tail estimate at one row.** The first draft had this. Both
  consumers already floor at one row, so a mutation removing it changed no
  verdict. It was deleted rather than given a test it could not deserve.
- **Recalibrate `SCAN_ROW_COST`** to move the line from 24 rows to the measured
  32–48. One fixture, one size, one machine; `stats.rs` records why moving that
  constant on a single measurement is the mistake it has already been. The
  measured line is reported below and left alone.

## Evidence

**The crossover**, `cargo run --release -p slate-slatedb --example
term_crossover`, three runs at 200,000 rows over the in-process S3 server.
Term `k<n>` is in exactly `n` rows, spread evenly (the index's worst case).
Every arm opens its own store:

| | run 1 | run 2 | run 3 |
| --- | --- | --- | --- |
| planner without counts takes the index for | no rung | no rung | no rung |
| planner with counts takes the index up to | 24 rows | 24 | 24 |
| cost constants predict the index wins below | 25 | 25 | 25 |
| forced index walk made fewer GETs up to | 32 | 32 | 32 |
| at 48 rows, index against scan GETs | 66 / 66 | 66 / 65 | 67 / 66 |
| GETs over the cheaper arm, always scanning (12 rungs) | 318 | 250 | 226 |
| the same, the counted planner choosing | 40 | 59 | 46 |

The last two rows are derived from each run's own table of index, scan and
chosen GETs; the example prints the counted planner's figure, and the
always-scan figure is the sum of `scan − min(index, scan)` over the same
columns. Scan GETs ranged from 51 to 73 between arms of one run. Index GETs
moved by a few. The ~20 GETs a freshly opened store makes before its first row
are in both arms alike. A smoke run at 2,000 rows passed the example's
row-count assertions.

On the cost model, `the_crossover_is_where_the_cost_model_puts_every_index`
finds the verdict flipping at 124 rows of 1,000,000, against a prediction of
125.

**The statistic against an oracle.** Each test compares `TermCounter` with an
exact `HashMap` count of the same deterministic Zipf stream:
`past_capacity_the_counts_keep_space_savings_guarantees` (every word within
N/C, and words held since their first occurrence exact),
`a_word_that_inherited_an_error_is_estimated_better_than_its_bounds` (6.168
against 7.017 and 10.339), `the_tail_estimate_is_near_the_tails_true_mean`
(3.22 against a true 2.72 on a 50,000-word tail, 15.91 against 15.84 near
capacity) and `the_vocabulary_is_estimated_to_within_two_standard_errors`.

**Mutations**, with the records in the order they were run:

- `ledger/mutations/20261003T190724-crates-slate-kernel-src-stats-rs.json`:
  nine mutations, **five survivors**. `contains` ignoring the counts; a tracked
  count not raised; the tail uncapped; the tail unfloored; the hash
  unfinalised. Each was followed up rather than excused. The `contains` case
  got a selectivity assertion. The tracked-count rule was replaced by the one
  measured best. The cap got a test. The floor was deleted. The vocabulary bar
  went from 10% to 6%, after FNV-1a without the finaliser measured 8.5% and
  9.9% off on two vocabularies where the finalised hash was 1.7% and 5.0%.
- `ledger/mutations/20261003T191113-crates-slate-kernel-src-stats-rs.json`:
  the five caught, and two new mutations survived. One was the `error > 0`
  guard, now covered by the exactness assertion. The other was the
  denominator, which led to the accounting fix above.
- `ledger/mutations/20261003T191413-crates-slate-kernel-src-stats-rs.json`:
  every mutation caught except the cap. With the corrected accounting, the cap
  provably cannot bind while the vocabulary estimate is exact, and a search of
  148 near-capacity streams found none where it did.
  `ledger/mutations/20261003T191647-crates-slate-kernel-src-stats-rs.json`
  then caught it with a test of the only condition the cap exists for: the
  sketch reading the vocabulary low.
- `ledger/mutations/20261003T191714-crates-slate-kernel-src-plan-rs.json`:
  ranging on the most common term, ranging on the first, and the candidate
  ignoring the count were each caught.
- `ledger/mutations/20261003T191729-crates-slate-kernel-src-record-rs.json`
  had one survivor: counting the words of an ordinary string index. It is now
  caught by `an_ordinary_index_on_text_has_no_words_counted`
  (`ledger/mutations/20261003T191758-crates-slate-kernel-src-record-rs.json`).
- The final set, against the code as committed after clippy's restructuring of
  `add`, plus a twelfth (a repeated word counted as new): all twelve caught,
  `ledger/mutations/20261003T192715-crates-slate-kernel-src-stats-rs.json`.

## What this does not do

- **The line sits a rung or two left of the measured one.** The counted planner
  takes the index up to 24 rows of 200,000. A forced walk was cheaper up to 32
  and tied at 48. The error is on the safe side — a scan where an index would
  have saved about a third of the GETs — and it comes from the cost constants,
  not from the counts.
- **Spread rows only.** The fixture places each term's rows evenly, which is the
  index's worst case. A term whose rows cluster, measured by `ascending_walk`
  at about a twentieth of a GET per row, would move the true line far to the
  right, and the planner does not know about density for any index.
- **Independence across terms is unchanged.** A two-word search is estimated
  as the product of its words' fractions, which understates correlated words,
  as the code comment on `Expr::Contains` already says. The counts make each
  factor right, not the product.
- **Counts are per column, across every tenant.** On a tenant-scoped table, a
  word common in one tenant and absent in the rest is estimated at its global
  rate. This is the same choice the histograms make, for the same reason the
  `analyze` docstring gives.
- **One measurement machine, and GETs against an in-process S3 server.** The
  GET counts do not depend on the machine. The latency a GET costs on real
  object storage does, and the wall-clock column is not a finding.
