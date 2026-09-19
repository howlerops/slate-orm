# Running the decoders, not just compiling them

## What changed

The generated Go and TypeScript row decoders are now executed by tests, and CI
runs them. `examples/explorer/backends/go/schema/rows_test.go` and
`examples/explorer/backends/node/src/rows.test.ts`, six cases each, wired into
the job that has already checked the generated files against the catalog.

## Why

The generated-row-types ledger said plainly that Go's and TypeScript's decoders
were "compiled but not executed" and that only Python's was exercised. This is
that caveat closed.

The gap mattered more than it sounds. `go build`, `go vet` and `tsc --noEmit`
prove a decoder type-checks. The failure the whole generator exists to
prevent — reading the neighbouring column — *also* type-checks, whenever the
neighbour happens to share a type. Compilation could never have caught it, so
the checks that were running were checking the wrong property.

## Alternatives rejected

**Generate the tests.** Cheapest, and worthless: a test the generator emits
agrees with the generator by construction. If `codegen.py` decided every column
sits at ordinal 0, it would emit a test asserting exactly that and the test
would pass. Both files are hand-written and say so at the top.

**Put them in the Go and TypeScript client suites.** Those jobs run
`go test ./...` and `npm test` already, which would have been less CI wiring.
Rejected because the files under test are *generated from the catalog*, and the
job that regenerates and diffs them is the three-SDK job. A decoder tested in a
job that never saw the catalog is a decoder tested against a stale copy.

**Reach for a test framework in TypeScript.** Node 22 has `node --test` built
in and the backend already runs `tsc`. One file does not justify a dependency,
a config and a second way to run tests in this repository.

**Assert only the happy path.** It would have passed every mutation that
matters. Each file decodes a good row *and* a transposed one, a short one, and
a null in a non-nullable column.

## Evidence

Six mutations of `codegen.py`, each regenerating the real files and running the
real suites. All caught:

| mutation | Go | TypeScript |
| --- | --- | --- |
| the type/tag check is skipped | caught | caught |
| every column decodes from ordinal 0 | caught | caught |
| the row-length check is dropped | caught | — |
| a null in a non-nullable column becomes null | — | caught |

The ordinal-0 mutation is the one this task exists for: it compiles cleanly in
both languages and every previous check — `go build`, `go vet`, `tsc` — passed
on it.

## What this asserts that it cannot fix

`TestScanBooksCannotSeeASameTypedSwap` and its TypeScript twin swap `id` and
`author_id`, both unsigned integers, and assert the wrong values come back
**without an error**. The decoder checks a value's *type*, not its meaning, so
same-typed neighbours are invisible to it.

That is written down rather than left implicit because "the decoder catches
transposition" is exactly the kind of claim that grows one retelling at a time
into something false. It catches a type mismatch. Same-typed neighbours are
what the ordinals in the generated declaration are for, and what the schema
fingerprint protects — a different mechanism, already tested elsewhere.

## What this does not do

Python's decoder is still tested in `scripts/test_codegen.py` against a
synthetic catalog, not against the demo's generated file the way these two now
are. The coverage is equivalent in kind; the two suites do not share a corpus,
so a case added to one does not appear in the others.

Only `books` is decoded. It is the widest table and covers every value type the
demo uses, but `authors`, `sales` and `editions` have generated decoders that
no test calls.

Nothing tests the decoders against rows that came from the *server*. These
build `[]slate.Value` by hand, so a disagreement between what the server sends
and what the decoder expects — a value arriving as `Int` where the schema says
`Uint`, say — would not be caught here. The conformance suite exercises the
real wire and does not use these decoders.
