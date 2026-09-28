# The first sample of the `deliberate` verdicts, and it found one

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

Fifteen `deliberate` verdicts drawn at random from the 848 predating today, and
read. **One is false**, and it was falsified by this session's own work six
hours earlier.

`ledger/2026-09-27-the-guard-for-two-copies-could-not-read-the-second-one.md`
recorded a decision not to fix a gap:

> The one latent gap it found — `check_cited_docs.py` not walking `site/` — is
> recorded rather than fixed because no page cites a ledger entry today, so
> widening it would add a check with nothing to check, which is the never-fires
> shape this repository treats as a liability rather than a win.

H7, this morning, widened `check_cited_docs.py`'s `SOURCE_SUFFIXES` to eleven
including `.html`. It now walks 22 files under `site/` and reports 782
citations. The decision the verdict records no longer stands, so the verdict is
`closed` against the witness that already existed for its sibling caveat.

The other fourteen hold.

## Why

Three entries today named the `deliberate` population as the largest unexamined
thing in the tracker and none sampled it. It outnumbers the open verdicts seven
to one, and its failure mode is quieter than an open caveat's: an open caveat
that has been fixed shows up as a gap somebody could close twice, which is
wasteful but visible. A `deliberate` that has been reversed shows up as
**nothing at all** — it reads as a settled decision, and a reader who consults
it is told the current design is something it is not.

The one found is the purest possible instance. It is not a decision that aged
out over weeks; it is a decision recorded on the 27th and reversed on the 28th
by a session that never looked at it. H7's task was "widen the four citation
guards past source and prose", and the previous day's entry had argued in
writing against exactly that widening for exactly one of those guards. Nothing
connected them. The argument may well have been wrong — H7 had its own reason,
that a guard which reads nothing today still catches the eighth invented
citation tomorrow — but the point is that the disagreement was never had.

**This is the same shape as the decimal scale**, which this session got wrong
three times: a claim recorded on one day, answered by a commit on the same or
the next day, and read a week later as current. The scale case took two
sessions and a docs-site publication to notice. This one took a random sample
of fifteen.

## Alternatives rejected

**Treat it as a `narrowed` rather than a `closed`.** The entry's wider point —
that only some guard scripts read the site, and a sweep found which — is still
true and is witnessed elsewhere. But this verdict is on the specific sentence
about `check_cited_docs.py` being left alone, and that sentence is now false
rather than partially true. A `narrowed` with an empty residual is a `closed`
with extra words.

**Amend the 2026-09-27 entry to say the decision was reversed.** The entry is
dated and append-only, and it was right when written — the same rule that
settled the four scale entries this morning
(`ledger/2026-09-28-the-four-entries-that-were-true-when-they-were-written.md`).
One day is a shorter interval than four and a half hours was there, but the
rule does not have a threshold and should not acquire one.

**Build a guard that notices a `deliberate` whose subject somebody later
changed.** It is the mechanical fix and it is the one this repository cannot
have: the verdict's subject is a sentence, not a symbol, and deciding whether a
commit reversed it is the reading. What *is* buildable is smaller and worth
noting for whoever wants it — a `deliberate` verdict could carry the paths its
reasoning depends on, the way a `closed` one carries a witness, and a guard
could report when one of those paths changed since the verdict's date. That
would have flagged this in H7's own commit. It is a schema change to the
tracker plus a roster of paths for 848 verdicts, and it is not this entry.

## Evidence

- The draw: `random.seed(20260928)`, `random.shuffle` over the 848
  `deliberate` verdicts whose entry predates 2026-09-28, first fifteen.
- The one that moved: `scripts/check_cited_docs.py` line 89 carries `.html` in
  `SOURCE_SUFFIXES`, and `source_files(ROOT)` returns 531 files of which 22 are
  under `site/` — measured by importing the module, not read off the source.
- Eight of the other fourteen checked mechanically and confirmed: six `--smoke`
  invocations still in `ci.yml`; `FIRST_DAY` still in
  `scripts/check_mutation_claims.py`; `authorized_table` still in
  `crates/slate-server/src/service.rs`; no local-timestamp type anywhere in
  `slate-schema` or `slate-kernel`; `crates/slate-kernel/tests/windows.rs`
  present; no replica restart in `examples/deployed/run.sh`; `compute_scalar`
  and `base_of` called nowhere outside `slate-sql` — the `slate-wasm` hit is a
  comment naming the function, not a call.
- Six were reasoning with no mechanical subject — "the container's allowance is
  what it is", "one fixture, stated" — and were read for whether the reasoning
  still applies. It does.
- `python3 scripts/check_closed_caveats.py`: 374 closed, 349 witnessed, 25
  exempt.
- `sh scripts/check.sh`: 72 passed, all of them.
- `python3 scripts/caveats.py`: 1585 caveats, 122 open, 65 narrowed, 374
  closed, 891 deliberate, 0 untriaged.

## What this does not do

**One in fifteen is not a rate.** The 95% interval around 1/15 runs from about
0.2% to 30%, which is almost the whole plausible range. What this establishes
is that the population contains at least one false verdict, which nobody knew
an hour ago, and that the cheapest way to find another is to look at decisions
whose subject somebody has since touched.

**It does not check the 833 others.** At fifteen per pass this is fifty-six
passes, which nobody should do. The path-dependency idea in the alternatives
above is the version that scales, and it is unbuilt.

**It does not re-litigate the reversed decision.** Whether `check_cited_docs.py`
*should* walk `site/` was argued one way on the 27th and the other way on the
28th, and both arguments are in the ledger. This entry records that they
disagree and that the second one won by being implemented, not that it was
right.
