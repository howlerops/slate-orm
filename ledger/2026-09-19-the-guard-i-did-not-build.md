# The guard I did not build

## What changed

Nothing outside this file. This records a guard I designed, tested against the
two defects it was meant to catch, and abandoned — so the next session that has
the same idea can start from the measurement instead of the idea.

## Why

Three documentation claims went stale or turned out wrong today, all in
`docs/orm-comparison.md` and two of them in one section: the "sugar over a
default" reasoning, the "codegen would turn a run-time check into a
compile-time one" prediction, and a design note's *Stops at* line. Each was
found by somebody who happened to be editing nearby. Three instances of one
shape in one day is this repository's usual threshold for building a check,
and the previous entry ended by saying a checker "would need the claims marked
up" — which was an assertion, not a finding. So I tested it.

The design: the document already marks a closed gap-table row by striking it
through and writing **Built**. A check could take every Built row's distinctive
words and refuse any of them appearing in the **What neither plan does**
section, which is where a claim of absence lives. No new markup, no prose
parsing, about sixty lines.

## Alternatives rejected

**Build it anyway and accept the noise.** The measurement below is why not: two
of its three hits are correct prose, and the ratio gets *worse* as the document
behaves better, because this document's convention is to leave withdrawals in
place rather than delete them. A check whose false-positive count grows every
time somebody does the right thing is a check that gets switched off — which is
the argument `CLAUDE.md` makes about `--no-verify` from the other direction.

**Add a marker so withdrawals are exempt,** e.g. every correction becomes a
block quote and the check ignores block quotes. This is the "marked up" version,
and it is the one I got furthest with. It fails on a paragraph that is neither a
live claim nor a withdrawal: *"Automatic timestamps and soft-delete conventions
were left out of both, and the ordering argument for that held"* — a true
historical statement about what the plans contained, naming two built features,
in its first sentence. Exempting it means marking up ordinary prose, and a
marker a writer must remember is the same discipline as remembering to update
the prose. It fails louder, which is worth something; it is not worth sixty
lines plus retrofitting three paragraphs written three different ways.

**Check the gap table against the code instead of against the prose** — the
version with real teeth, since it would catch a row claiming something is
missing when it is not. Every row's subject is a sentence of English ("Generated
migrations from a schema diff"), and deciding whether the repository implements
one is the reading a person does. Not mechanizable without the markup problem
returning in a harder form.

**Say nothing and move on.** The cheapest, and it leaves the previous entry's
"a checker would need the claims marked up" standing as a guess. It is now a
tested claim with a specific reason, and the next person to have this idea gets
the fifteen minutes back.

## Evidence

I implemented the token check as a throwaway and ran it against
`docs/orm-comparison.md` as it stood **before** today's codegen correction —
the state in which one real defect was present. Distinctive words were those
over three letters, lowercased, minus a stop list of the document's own
vocabulary (`catalog`, `client`, `table`, `built`, `plan`, and so on).

Three Built rows were flagged:

| flagged row | shared words | verdict |
| --- | --- | --- |
| ~~Generated *types* from the catalog~~ | `codegen`, `value` | **the real defect** |
| ~~Automatic `created_at` / `updated_at`~~ | `automatic`, `created` | correct prose — the withdrawal written this morning |
| ~~Soft delete as a first-class concept~~ | `delete`, `deleted`, `soft`, `kernel`, … | correct prose — the same withdrawal's second half |

So it does catch the thing, which surprised me: I had expected the gap-table row
title ("Generated types from the catalog") to share nothing with the prose
("client codegen"), and it was the row's *body* naming `scripts/codegen.py` that
carried the match. One true positive, two false, on a section of six
paragraphs.

Checking whether a syntactic convention separates them: the three corrections in
that section are a bolded lead-in (`**The reason given here … was wrong**`), an
inline bold word (`that one is **withdrawn**`), and a block quote (the one I
wrote an hour ago). Three shapes, no convention — `grep -nE "^> |^\*\*|^#"` over
the section shows each correction starting differently.

I did not measure a false-negative rate. With three known instances and one
document there is nothing to measure it against, and saying "it caught the one
case I built it for" would be the manufactured finding this repository's
standards warn about.

## What this does not do

- **It builds nothing**, so the recurrence it describes is still unguarded. The
  three instances stand as three instances.
- **It tested one design on one document.** A different approach — claims
  carrying a machine-readable reference to the gap-table row they depend on,
  checked both ways — was not prototyped, only reasoned about above. If someone
  wants to attack this class properly, that is the direction I would start
  from, and my reasoning against it is cheaper than a measurement.
- **The two false positives were not counted over the whole document**, only
  over the one section. The section was chosen because both of today's stale
  claims lived in it, which makes the sample favourable to the guard rather
  than against it.
