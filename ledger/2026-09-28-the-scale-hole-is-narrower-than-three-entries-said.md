# Withdrawing an overstatement I published an hour ago about decimal scale

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `site/docs/limits.html`, `docs/caveat-status.json`
- **Kind:** docs

## What changed

The paragraph
`ledger/2026-09-28-the-limits-page-omitted-the-sharpest-limit.md` added to
`site/docs/limits.html` said a client's decimal scale is unchecked. It is
checked, for every client that uses the generator, and the paragraph now says
what is actually true. The five caveats it was drawn from are narrowed to
match.

## Why

**`scripts/codegen.py` writes the scale out of the catalog, in all three
languages.** Lines 1364, 1495 and 1619: `scale=`, `Scale:` and `scale:`, each
read from `column["scale"]`. And `.github/workflows/ci.yml` runs
`codegen.py --check` twice, so a generated declaration that disagrees with the
catalog it came from is a red build.

**Five** entries say the scale is written by hand and checked by nobody —
`2026-09-18-arithmetic-over-money-and-the-expressions-that-are-refused.md`,
`2026-09-18-decimals-and-conditional-updates-in-three-clients.md`,
`2026-09-18-nineteen-ninety-nine-is-a-decimal-not-a-float.md`,
`2026-09-18-the-three-sdks-compared-on-a-decimal.md` and
`2026-09-18-three-clients-and-the-integer-they-would-all-have-reached-for.md`.
Three is what today's earlier entry counted and five is what a query of the
tracker returns, which is the second miscount in two entries and the reason
this one counted rather than remembered. On the day they were written
that was true; the generator landed the same day, in
`2026-09-18-the-catalog-writes-the-declaration-nobody-should-type.md`, and
nothing connected the two. I read all five during today's triage, gave them
the same `open` verdict five times, and repeated the claim onto the docs site
without checking it.

What is left is real and narrower: a **hand-written** declaration is unchecked,
and so is a generated one against a server whose catalog has changed scale
since it was generated. `--check` is a build-time comparison against a local
catalog, not a handshake against the server's.

## Alternatives rejected

**Leave the page and fix only the caveats.** The page is the copy a reader
meets and the entries are the copy nobody but an agent reads. Leaving the
overstatement where a reader would find it, to correct it where they would not,
is the inversion this repository's retired-claims guard exists to prevent.

**Close the five caveats outright.** The remaining hole is not nothing: it is
the one every hand-written client has, and the one every client has across a
scale migration. `narrowed` with the residual spelled out is the verdict the
tracker has for exactly this, and using `closed` because most of a claim went
away is how a residual gets lost.

**Hash the scale into the fingerprint and close it properly.** The right
conversation and not a documentation change. It makes a scale change a breaking
schema change for every client, it touches four independent fingerprint
implementations and their pinned constants, and the argument against it — a
scale addresses no column, so hashing it refuses a request that reaches exactly
the right row — is written down and has not been rebutted. Recorded as the open
question rather than settled in passing.

## Evidence

`git grep -n -i scale scripts/codegen.py` — the three emitters, at 1364, 1495
and 1619, each taking `column["scale"]` from the catalog.

`git grep -n codegen .github/workflows/ci.yml` — `codegen.py --check` at 560
and 576.

`python3 site/check/docs.py`: the ten pages render and every relative link
resolves.

No measurement: this is a reading of two files that contradicted a claim, and
the claim is withdrawn rather than defended.

## What this does not do

**It does not check a scale at connect time**, which is the thing that would
close the class. The fingerprint question above is the decision that would.

**It does not re-read the other 117 open caveats for the same mistake.** Five
entries said the same false thing and I repeated it; the odds that this is the
only claim in the backlog overtaken by work on its own day are not good, and
nothing here looks. The triage's own closing caveat already records that a
verdict resting on an entry's reasoning is only as good as that reasoning, and
this is the first worked example of it going wrong in the *open* direction
rather than the deliberate one.
