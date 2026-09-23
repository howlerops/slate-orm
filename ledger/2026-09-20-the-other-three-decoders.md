# The other three decoders

## What changed

`/api/typed` in all three demo adapters now reads and decodes `authors`,
`sales` and `editions` alongside `books` and `shipments`. Five generated
decoders, five live rows, three languages — and one conformance case comparing
what all three make of them.

## Why

`ledger/2026-09-19-decoders-over-the-servers-own-rows.md` built the route that
first put a *server-sent* row through a generated decoder, and closed by naming
what it had left: *"Three of five decoders per language are still only run
against fixtures. `authors`, `sales` and `editions` have unit cases from the
previous entry and no live row."*

Two of five is the shape of coverage that reads as done. The claim people take
from it is "the generated decoders agree with the server", and what was true
was "the two decoders somebody picked agree with the server". Every other table
was still a hypothesis dressed as a tested path — the same distinction the
entry before it drew between a decoder that compiles and one that runs.

## Alternatives rejected

**Leave it, on the hypothesis the entry itself offered.** That entry argued the
remaining three "add a column list rather than a kind" — the two covered tables
already exercise u64, str, i64, decimal, vector, nullable and enumerated
between them — and labelled it a hypothesis. It is true about *value shapes*
and it is the wrong thing to measure: the failure a decoder exists to catch is
a wrong **ordinal**, and an ordinal is per-table. The evidence below is a
same-typed ordinal swap in `authors` that no existing case could see. The
hypothesis was right about what it said and wrong about what mattered, which is
why it was labelled one.

**A separate route per table.** Five routes, five conformance cases, and a
clearer failure message when one breaks. Rejected: the cases already name the
table in their output, the runner prints the whole JSON on a disagreement, and
five routes is five things to keep in step with the seed for no information
gained.

**Cover them in each language's unit suite instead.** They already are, and
that is precisely the coverage this closes the gap *in*: a hand-built row is
the test agreeing with its own idea of what the server sends. The point of the
route is that the row comes from the server.

**Include `rating`, now that more columns are in play.** No: a float's spelling
is the one thing three languages will not agree on without a shared formatter,
which is why the original case left it out and why the corpus pins it elsewhere.
Adding it here would turn a decoder case into a rendering case.

## Evidence

**A same-typed ordinal swap is now caught, and was invisible before.** Making
Go's generated `ScanAuthors` read `row[2]` for `name` — `name` and `country`
are adjacent and both `String`, so nothing about the types objects — turned the
conformance run red on exactly one case:

```
two rows through the generated decoders (app): the adapters disagree
    go      ... "name": "US" ...
    node    ... "name": "Ursula K. Le Guin" ...
    python  ... "name": "Ursula K. Le Guin" ...
```

Before this change that mutation produced **no disagreement at all**, because
no adapter route called `ScanAuthors`. That is the whole finding: the decoder
was generated, compiled, vetted, unit-tested against a fixture, and unreachable
from anything that talks to a server.

It is caught here only because the seeded author's `name` and `country` differ
("Ursula K. Le Guin" and "US"). A fixture using the same string for both would
pass the swapped decoder, which is why each adapter's new block carries a
comment saying so.

**The three agree:** `./run.sh --conformance` — 99 cases, all three SDKs agree,
with the restored decoder.

**Static:** `sh scripts/check.sh` — 20 of 20, which includes `gofmt`, `go vet`,
both `tsc` runs, both `ruff` runs, both `ty` runs and the Python decoder suite.
The generated Go file was restored and `git diff` on it is empty, so the
codegen diff in CI has nothing to object to.

## What this does not do

- **Still nothing decodes a value of the wrong *kind* from a live server.** The
  case proves the decoders agree with the rows the server actually sends. An
  `Int` where the schema says `Uint` is the failure they exist to catch and the
  server does not produce it through any surface. Unchanged, and still the
  weaker half of this coverage.
- **No new table is covered by construction.** A sixth table added to
  `head.toml` gets a generated decoder and no live row until somebody edits
  three handlers. The three unit suites each have a coverage guard that fails
  on an unrun decoder; this route has none, and building one would mean the
  adapters reading their own generated module reflectively, which is a bigger
  change than the gap.
- **`rating` and the float formatter are untouched**, as above.
- **No measurement.** Three extra point reads per call to one demo route.
