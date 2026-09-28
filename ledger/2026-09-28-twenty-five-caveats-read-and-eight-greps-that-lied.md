# Twenty-five open caveats read, and eight greps that looked like refutations

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** nothing but this entry and `docs/caveat-status.json`
- **Kind:** docs

## What changed

A second pass over the open caveats, continuing
`ledger/2026-09-28-reading-the-open-caveats-instead-of-grepping-them.md`.
Twenty-five examined across both passes. Four verdicts moved, all in the first
pass and all recorded there. **Twenty-one were re-read and hold unchanged**,
which is this entry's content: a null result over most of a sample, and the
rate that goes with it.

No verdict changed here, and no `checked` date moved — every one of the
twenty-five already carried today's, from the triage that produced them. What
this adds is that twenty-one of them have now been read against the tree by
somebody who was trying to falsify them, rather than filed by somebody
assigning verdicts in batches of forty.

## Why

Five open caveats were found false yesterday by accident. That made the rate
the interesting number: if a fifth of the open list is stale, the list is a
worse instrument than its count suggests; if it is a twentieth, the triage was
sound and the five were unlucky.

**Four of twenty-five, and all four were in the first eleven.** The second
fourteen produced nothing. That is not a contradiction — the first pass took
the cheapest-looking claims, and cheap to check correlates with being about a
file somebody has since touched — but it means the honest reading of the rate
is "somewhere under a fifth, concentrated in the parts of the tree that move",
not a number.

The method finding is worth more than the rate.

**Eight greps returned hits that looked like refutations and were not.** Every
one would have closed a true caveat if the hit had been taken as an answer:

| caveat | the grep | what the hit actually was |
|---|---|---|
| the wasm join is inner | `outer\|Left\|Right` in `crates/slate-wasm/src/lib.rs` | comments saying `JoinSpec` carries no join type, so the join *is* inner |
| no conformance case for `contains` | `contains` in `examples/explorer/conformance/conformance.py` | a comment about the absence |
| no SQL surface for a conditional delete | `as_of` in `crates/slate-sql/src/sql.rs` | four matches on `alias_of` |
| no `tracing` in the daemon | `tracing` in `crates/slate-serverd/src/` | a `# Why not tracing` docstring in `observe.rs` |
| `--print-schema`'s output is not versioned | `version` in `main.rs` | `schema_version`, a *table's* migration version, not the output's |
| the Go and TypeScript hints are untested | `warning` in `clients/go/slate/*_test.go` | prose in the protobuf-freshness test about stale stubs |
| `--check` does not prove anybody imports the generated file | `import … schema.js` in the node backend's `main.ts` | a `import type` — proves compilation, not that `declaring` is called |
| the Python client has no equivalent rename test | `renamed` in `clients/python/tests/` | a *different* rename test: a client declaring a wrong name, not a column accepted under its previous one |

The last two are the instructive ones, because the hit is in the right file and
is about the right feature and still does not bear on the claim. A `import
type` is erased at compile time; the caveat is about a *runtime* call. Python
has a rename test; the caveat is about the other rename. Neither is a
vocabulary accident — both are the claim's own subject matter, one refinement
away from the claim.

This is the counterpart to the fourth-through-seventh entries in the first
pass's table, and together they make the point that a grep here is worse than
random rather than better: a caveat that says "nothing does X" attracts the
word X into the comments *around* the absence, into a test of the neighbouring
thing, and into a docstring explaining the decision not to.

## Alternatives rejected

**Report the rate as "16%" and stop.** Four of twenty-five is 16% and the
number is meaningless: the sample was not drawn, it was chosen for cheapness,
and the four cluster in the first eleven. A percentage from a biased sample
read as a population estimate is the same error as five runs on one machine
read as a property, which this session already made once today
(`ledger/2026-09-28-a-measurement-that-reversed-under-ci.md`). Stating the
counts and the bias is the honest version and is less satisfying.

**Bump every re-read caveat's `checked` date to record the second reading.**
They all already read `2026-09-28` — the triage gave them that this morning —
so the field cannot distinguish "assigned in a batch of forty" from "read
adversarially against the tree". Adding a second field to draw that
distinction is a schema change to the tracker for a distinction only this
session has needed, and `caveats.py --unread` would then need to decide which
one it means. Left alone, with this entry as the record of which twenty-five.

**Keep grepping and read only the hits.** It is what the first pass did and the
table above is the case against it. Absence is conclusive and cheap — a grep
that finds nothing has settled the claim — but presence settles nothing, so the
method reduces to "read every file where the word appears", and the word
appears wherever the subject does.

## Evidence

- Twenty-five open caveats examined. Four moved, in
  `ledger/2026-09-28-reading-the-open-caveats-instead-of-grepping-them.md`;
  twenty-one confirmed by reading the file the claim names.
- Eight misleading hits, tabulated above, each with the file and the reason the
  hit is not the claim.
- Confirmed-holding by conclusive absence (no hit anywhere the claim could
  hide): no client expresses `NOT IN`, no `sum(price)` in the corpus, no search
  box in the demo's web UI, no `delete_if_unchanged` in the SQL front end, no
  purge-refusal case in any client suite, no bar-chart geometry assertion, no
  demo-stack teardown test, no packaging check for the Python client, no
  freshness-floor assertion in the TypeScript suite.
- `python3 scripts/caveats.py`: 1576 caveats, 118 open, 63 narrowed, 373
  closed, 889 deliberate, 0 untriaged. 116 open before this entry; the two it
  adds are its own.

## What this does not do

**It does not examine the other ninety-one.** Twenty-five of the 116 open at
the time of writing have now been read adversarially. The remainder are in the same state the triage left them:
each has a verdict and a date, and nobody has tried to falsify it.

**It does not explain why the four clustered.** "Cheap to check correlates with
recently touched" is a hypothesis fitted to four data points after the fact,
and the obvious competing one — that the first pass was more careful because it
was the first — fits them equally well. Distinguishing the two means reading a
*random* twenty-five, which is the thing worth doing next and is not what
either pass did.

**It leaves the grep table as advice rather than a guard.** Nothing stops the
next session grepping and believing the hit. A guard cannot be built for this —
deciding whether a hit bears on a claim is the reading — but the previous
entry's alternatives section already argued that, and the accumulating table is
the only mechanism either entry offers.
