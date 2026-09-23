# The accessor three adapters now call

## What changed

`/api/typed` in all three demo adapters reports `shipment.retired`, taken from
the **generated** accessor rather than by testing `deleted_at` at the call
site. Two cases each in the Go and TypeScript decoder suites, matching the two
Python already had.

## Why

`2026-09-20-the-column-the-catalog-knew-about.md` published `soft_delete` and
generated an accessor from it, then closed by naming exactly what it had left:

> **No Go or TypeScript test for the new method.** … That is the "generated,
> compiled, never called" shape this repository keeps meeting, and I am naming
> it rather than claiming otherwise.

Naming it is not closing it. The repository's history here is five entries
long — a workflow that never fired, decoders that compiled and never ran,
foreign keys published and never generated from, a counter nobody scraped, a
purge nothing called — and in every one the thing existed, typechecked, and was
wrong or would have been wrong without anybody knowing. An accessor with a
Python test and two compile checks is two-thirds of that pattern already.

The route it went into is the one that already reads a `shipments` row through
the generated decoder, so the accessor now runs on a path the three-SDK
comparison covers. That is what makes the conformance corpus, and not only the
unit suites, able to see it.

## Alternatives rejected

**Unit cases only, in the two missing languages.** Brings the three to parity
and leaves the accessor uncalled by anything that talks to a server — the half
of the gap that matters. The Python case passed happily while the Go one did
not exist; neither would have caught an adapter that never used it.

**A new route for it.** A route that exists to be tested. `/api/typed` already
decodes the row this asks about.

**Report the stamp instead** — `deleted_at` is already in that response, so a
reader can compare it to null themselves. That is precisely the knowledge the
published ordinal exists to remove from the caller, and shipping it as the only
answer would have made the accessor decorative.

## Evidence

**One mutation, caught twice, by two independent mechanisms.** Inverting Go's
generated `Retired()` — `r.DeletedAt != nil` → `== nil`:

- `TestRetiredFollowsTheStamp` fails in the Go unit suite;
- and the conformance run goes red on *"two rows through the generated
  decoders"*, with Go's `retired` disagreeing with node's and python's.

The second is the one that did not exist before this change. A unit test proves
the function is right; the conformance case proves the three *adapters* agree
about a value a real server produced, which is a different claim and the one
the corpus is for.

**The negative half is covered in all three now.** Each suite asserts that
`Shipments` is the *only* type with the accessor — Python by `hasattr`, Go and
TypeScript by reading the generated source, since neither has a runtime
equivalent. An emitter keying on a column named `deleted_at` rather than on the
published ordinal passes everything else and fails these.

**Suites:** Go `./schema/` ok; node 25 pass, 0 fail; Python 26 passed.
`./run.sh --conformance` — 99 cases, the three SDKs agree, with the accessor
restored. `codegen.py --check` — all three generated files match the catalog.
`cargo fmt --all -- --check` clean. `sh scripts/check.sh` — 20 of 20.

## What this does not do

- **The case count did not change.** 99 before and after: the accessor widened
  an existing case's answer rather than adding one. Worth saying because "99
  cases agree" is the same sentence as yesterday and covers more than it did.
- **Still no `restore`.** The last item of the entry this finishes, and the one
  that needs a write surface rather than a read.
- **Nothing outside the demo calls it.** The three clients' own suites do not,
  because the accessor is on a *generated* type and those suites test the
  hand-written client. That is the right division and it means the accessor's
  only live caller is the demo.
- **No measurement.** A null comparison.
