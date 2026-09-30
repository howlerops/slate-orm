# Reading the open caveats instead of grepping them

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`,
  `ledger/2026-09-28-the-limits-page-omitted-the-sharpest-limit.md`
- **Kind:** docs

## What changed

A first pass over the open caveats, reading each candidate's subject in the
tree rather than grepping for a word in it. Four verdicts moved:

- **Closed.** "No client exposes `ExplainAggregate` inside a transaction except
  Go." False. `clients/typescript/src/client.ts` has
  `Transaction.explainAggregate` and `explainAggregateJoin`, both routed
  through `explainAggregateIn`; Python's `Transaction` inherits
  `_Ops.explain_aggregate`, whose request carries `self._transaction_id()`,
  and `Transaction._transaction_id` returns the transaction's id. All three.
- **Closed.** "The quickstart check does not verify the prose around the
  snippets." False. `check_install_lines` in `site/check/quickstarts.py` reads
  the install line under each snippet and fails when it names a local path that
  is not there — written against `pip install slate-client` and
  `npm install @slate-orm/client`, the two lines this caveat named.
- **Narrowed.** "Nothing checks a document's claims against the repository."
  `scripts/check_site_claims.py` does, and its `claim_pages` covers every page
  under `site/` — `site/docs/roadmap.html` included, which the companion caveat
  names — plus `README.md` and every `docs/*.md` design note. The residual is
  that its roster is hand-maintained.
- **Narrowed.** "It fixes neither", on the limits page. The scale half was
  closed; the latency half stands.

And one correction, which is the part of this that matters.
`ledger/2026-09-28-the-limits-page-omitted-the-sharpest-limit.md` still said
"the fingerprint deliberately does not hash a scale" — the claim I withdrew the
same evening in `7b00dbe` and wrote up in
`ledger/2026-09-28-the-scale-hole-was-closed-ten-days-ago.md`. The page was
rewritten and the verdicts were corrected; the entry that published the claim
was not. Its paragraph is now struck, in this repository's own idiom
(`~~…~~ **Wrong: …**`, as in
`ledger/2026-09-14-frontend-tests-and-configurable-ports.md`), with the commit
and the file that refute it.

## Why

Five open caveats were found false yesterday by accident, twice removed: a
triage pass marked the scale hole open, the docs site published it, and only
writing the correction turned up `fingerprint.rs` hashing the scale since
`35d9182`. That is a precision failure in the tracker, and the counterpart to
the recall failure `2026-09-28-the-tracker-could-not-see-a-third-of-the-caveats.md`
fixed. Nothing had re-examined the population those five came from.

The method matters more than the four verdicts. Seven candidates were first
tested by `git grep` for a word from the claim, and **four of the seven came
back with hits that meant nothing**: the `outer|Left|Right` hits in
`crates/slate-wasm/src/lib.rs` are a comment saying `JoinSpec` carries no join
type, so every join the binding runs *is* inner; the `contains` hit in
`examples/explorer/conformance/conformance.py` is a comment about the absence;
the `as_of` hits in `crates/slate-sql/src/sql.rs` are `alias_of`; and the
`tracing` hits in `crates/slate-serverd/src/observe.rs` are a "Why not
`tracing`" docstring. All four claims hold, and a grep-driven pass would have
closed all four. That is the same mistake that produced yesterday's "`plan.rs`
costs a grouped join as grouped" and "`sql.rs` refuses `WITH` by name", both of
which resolved to a real path and a false claim.

So the rule this pass ran under, and the one worth keeping: a hit is a place to
start reading, never an answer. The four verdicts above each rest on a file
read to the point where the claim is decided — the method body, not the symbol.

## Alternatives rejected

**Grep the whole population and triage the hits.** Fast, and it is what
produced 4/7 false positives in the sample above. The failure is systematic
rather than unlucky: a caveat says "nothing does X", the tree contains the word
X in a comment *explaining why nothing does X*, and the grep reads as a
refutation. Caveats attract their own vocabulary into the comments around them,
so grep is worse than random here, not better.

**Write a guard that checks a verdict's claim.** The Stop hook naming this
session's remaining work put it as "citation guards do not verify semantic
correctness of claims", which is true — `check_caveat_citations.py` resolves a
path and stops. Rejected as unbuildable in the general case for the reason
`ledger/2026-09-19-the-guard-i-did-not-build.md` argues at length and this
session's own experience confirms: deciding whether "no client exposes X"
holds means understanding Python's inheritance, which is not a regex. The
tractable version is `check_site_claims.py`'s: a hand-written roster pairing
one claim with one mechanical check, plus an `UNCHECKED` list. Extending that
idiom from the site to the caveat tracker is plausible and is a piece of work,
not a line.

**Re-read all 112 now.** Correct, and not what this is. Each verdict above cost
a file read and two of the four needed a second file; at that rate the whole
population is several sessions. Doing a quarter of it badly would put the
tracker back where it was, with verdicts that look checked.

## Evidence

- Four verdicts changed, each from a read: `clients/typescript/src/client.ts`
  lines 1401 and 1741, `clients/python/src/slate/client.py` lines 1481 and 1667
  (`_Ops.explain_aggregate` against `Transaction._transaction_id`),
  `site/check/quickstarts.py` `check_install_lines`,
  `scripts/check_site_claims.py` `claim_pages`.
- Four claims tested and **not** changed, each after reading the file the grep
  hit: `crates/slate-wasm/src/lib.rs`,
  `examples/explorer/conformance/conformance.py`,
  `crates/slate-sql/src/sql.rs`, `crates/slate-serverd/src/observe.rs`.
- `python3 scripts/mutate.py`, record
  `ledger/mutations/20260928T213949-scripts-check-closed-caveats-py.json`:
  both new witnesses broken in turn — the needle renamed, the path pointed at
  a sibling file — and both caught by "the real roster: every witness is
  still in the tree". Baseline and restore each reported 1 suite, none
  failing.
- `python3 scripts/check_closed_caveats.py`: 371 closed, 347 witnessed in the
  tree, 24 exempt.
- `sh scripts/check.sh`: 71 passed, all of them.
- `python3 scripts/caveats.py`: 1562 caveats, 114 open, 61 narrowed, 371
  closed, 883 deliberate, 0 untriaged — 116 open before this, four verdicts
  moved off `open`, and this entry's own two open caveats added back.

## What this does not do

**It reads eleven of 116, and changes four.** The other 105 are unexamined.
Two of the four it changed were false and one was half false, which is a rate
worth knowing and is not an estimate of the rest: the eleven were picked for
being cheap to check, and cheap to check correlates with being about a file
that somebody has since touched.

**It adds no guard.** The false-verdict class this keeps meeting is caught by a
person reading, and this entry argues above that the general case cannot be
caught otherwise. `check_site_claims.py`'s roster is the shape of the version
that could be built and nothing here builds it for the tracker.

**It does not check whether other entries carry the withdrawn scale claim.**
The one struck here was found because it is the entry this session wrote. Two
other entries published the same wrong reasoning on 2026-09-18 and were the
*source* of the five caveats; whether either states it as present-tense fact,
and whether `check_retired_claims.py` would see it if they do — it excludes
`ledger/` deliberately — is unexamined.
