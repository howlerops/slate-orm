# Sixty of nine hundred deliberate verdicts read, and the population turns out to be the cleaner one

- **Date:** 2026-09-29
- **Author:** Claude Code, task K2
- **Touches:** `docs/caveat-status.json`
- **Kind:** docs

## What changed

The `deliberate` sample was extended from fifteen to sixty, continuing the same
recorded draw rather than starting a fresh one. **Forty-five read, none moved.**

With the one the first fifteen found, that is **1 of 60 — 1.67%, 95% interval
0.04% to 8.94%** (Clopper-Pearson, computed here). Six open caveats across five
entries asked for exactly this and close with it.

The result is the opposite of what three entries predicted. `deliberate`
verdicts outnumber open ones seven to one and were repeatedly called "the
largest unexamined population in the tracker" — with the implication that the
risk scaled with the size. Measured, they are **at least as sound as the open
ones**: 1.67% [0.04, 8.94] against the open population's 4.04% [1.11, 10.02].
The intervals overlap almost entirely, so the honest claim is not that
`deliberate` is cleaner; it is that **nothing supports the fear that it is
worse**, which is what three entries assumed.

## Why

"They outnumber the open ones seven to one and nobody has looked" is a real
argument for looking and not an argument about the rate. Two mechanical routes
into the population were designed and measured and both were rejected — the
path-dependency flag and the citation flag — each because it raised roughly a
third of the backlog to find a few percent. What was left was reading, and the
only open question was how many reads buy a usable bound.

Sixty does. At 1/60 the interval's top is 8.94%, which over 900 verdicts is
about 83 — an upper bound somebody can act on. The alternative reading, that
the number is near 1.67%, puts it at 15. Either way it is not the hundreds the
"largest unexamined population" framing implied.

## Alternatives rejected

**Fifteen more, to sixty from the existing forty-five.** The marginal
information is small and the entries have said "this is the third to say so
without doing it" three times. Ninety reads would move the upper bound from
8.9% to about 6%, which does not change what anybody does about it.

**Drawing a fresh sample rather than continuing the recorded one.** The seed
and the shuffle are in
`ledger/2026-09-28-the-first-sample-of-the-deliberate-verdicts.md` precisely so
the next pass is a continuation. A fresh draw would re-read some of the
fifteen and make the combined denominator an argument rather than an addition.
The population is reconstructed from `git show 90731fc^:docs/caveat-status.json`
— 848 verdicts — because today's file has more, and shuffling a different list
gives a different order.

**Moving anything on a thin reading.** Thirty of the forty-five have no
mechanical subject: "the container's allowance is what it is", "prose has
nothing to mutate", "an untested path is worse than a refusal". They were read
for whether the reasoning still applies, and it does. A verdict of that kind
cannot be falsified by a grep, and pretending otherwise is how a sample
produces a number that means nothing.

**Proposing a third guard.** Two have been measured and withdrawn in two days.
The design that would work — a `depends` field naming the paths a verdict's
reasoning rests on — is unchanged and still costs a backfill of 900 rows, each
of which is the reading it would replace. Repeating the proposal without new
evidence would be the third entry to name it.

## Evidence

- The draw, continuing the recorded one exactly:
  `git show 90731fc^:docs/caveat-status.json`, `deliberate` verdicts whose
  entry predates 2026-09-28, **848 of them** — the number the previous entry
  reported, which is how the reproduction was verified rather than assumed.
  `random.seed(20260928)`, `random.shuffle`, then `order[15:60]`.
- **Fifteen were settled mechanically**, by a `git grep` over the thing the
  reasoning names: the legacy-table roster still in
  `scripts/check_table_provenance.py`; no two-node scrape test; one
  `warnings.push` site in `crates/slate-sql/src/sql.rs`, so the warning is
  still the only warning; `check_cost_prose.py` still reading both constants;
  `PYTHONPYCACHEPREFIX` in `mutate.py` and no general stale-cache detection; no
  local-timestamp type in `slate-schema` or `slate-kernel`; no recomputation of
  the fingerprint from the stored schema; `crates/slate-server/tests/multi.rs`
  present; no `MUST_DIFFER` pair for the restore case; `to_row` on `Record` and
  nowhere for a join or a chain; the 403-bytes-a-row figure still in
  `docs/performance.md`.
- **Thirty were reasoning with no mechanical subject** and were read for
  whether the reasoning still applies.
- Rates, exact binomial: the first fifteen **1/15 = 6.67% [0.17, 31.95]**; this
  pass **0/45 = 0% [0, 7.87]**; together **1/60 = 1.67% [0.04, 8.94]**. The
  open population, for comparison, **4/99 = 4.04% [1.11, 10.02]**.
- `python3 scripts/caveats.py` after this entry's own caveats are triaged, and
  `sh scripts/check.sh` — both reported in the commit.

## What this does not do

**840 remain unread.** Sixty of 900 is a sample, and the interval above is what
a sample buys. What it rules out is the high rate; it does not rule out the
moderate one, and at 8.9% there would be eighty-three false verdicts in there.

**The sample is of verdicts, not of harm.** A `deliberate` that has stopped
being defensible is a decision nobody would make again, and whether that
matters depends entirely on which decision. The one the first fifteen found —
`check_cited_docs.py` reading `.html` when a verdict said it did not — was
harmless. Nothing here estimates how bad the next one would be.

**Reading remains the only method.** Two guards have been designed, measured
and withdrawn. This entry adds a third data point about the population and no
new way to search it, which means the next pass costs what this one cost.

**A verdict read for "the reasoning still applies" is read by the same kind of
reader who wrote it.** Thirty of the forty-five were settled that way. That is
weaker than a grep returning nothing, which is itself weaker than reading the
function, and the sample is two thirds of the weakest kind.
