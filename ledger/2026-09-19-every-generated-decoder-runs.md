# Every generated decoder is executed by a test, and a check fails if one stops being

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `examples/explorer/backends/go/schema/rows_test.go`,
  `examples/explorer/backends/node/src/rows.test.ts`
- **Kind:** fix

## What changed

A case for `authors`, `sales`, `editions` and `shipments` in both the Go and
the TypeScript decoder suites, alongside the five `books` already had. Then a
test in each that reads the generated source, finds every `ScanX` /
`decodeX` it declares, and fails naming any that the case table does not run.

## Why

The previous entry closed by admitting it: "`authors`, `sales` and `editions`
have generated decoders that no test calls." Since then `shipments` arrived and
made it four. A generated decoder that nothing runs is a decoder whose ordinals
are asserted by nobody, and a wrong ordinal is the specific mistake this whole
generator exists to prevent — it compiles, it type-checks, and it returns the
neighbouring column.

`shipments` is the sharpest case: it is the only table in either language with
a nullable column, so until now the `*int64` / `bigint | null` branch of the
generator's output had never been executed at all, in either direction.

The coverage check is the other half and is the part worth arguing for. "Add a
case when you add a table" is an instruction, and this repository's record with
instructions of that shape is five drifted copies of the demo's table list and
a test written to stop the sixth. Reading the generated file costs four lines
and turns the instruction into a failure with the missing name in it.

## Alternatives rejected

**Give each of the four the same five cases `books` has.** Twenty cases instead
of four, testing one thing twenty times: the generator emits *one* decoder
shape, so transposition, short rows and nulls behave identically in all of
them. What differs per table is the column list, which is exactly what one case
asserts. The shape cases stay on `books`, and the comment above the new block
says so rather than leaving a reader to wonder why the coverage is uneven.

**Generate the tests.** Then they agree with the generator by construction and
prove nothing, which is the first line of both files and the reason they are
hand-written.

**Count the decoders instead of naming them.** `assert len(declared) == 5`
is one line and fails on the right event — but its message is "5 != 6", and the
fix is to work out which of six is new. Naming costs a loop.

**Skip the coverage check and rely on review.** The gap this closes lasted from
the day the decoders were generated until today, through two sessions that both
read these files.

**Use `go/ast` rather than a regular expression.** Correct, and the pattern
`^func Scan(\w+)\(` over generated output whose shape is fixed by a template in
`scripts/codegen.py` is not going to meet a case the parser would handle
differently. The `len(declared) < 5` guard is there for the version of that
judgement that turns out to be wrong: a pattern that matches nothing reports
success, which is how a "check them all" test checks none.

## Evidence

Go: `go test ./schema/` in the demo's Go backend, 11 tests. TypeScript:
`npm test` in the node adapter, 13. Four mutations, two per language:

| mutation | result |
| --- | --- |
| Go: `ScanEditions` removed from the case table | caught — "ScanEditions is generated and nothing runs it" |
| Go: `shipments.book_id` written into `out.Id` | caught — `TestEveryGeneratedDecoderRuns/Shipments`, with the wrong struct printed |
| TS: `decodeSales` removed from `DECODERS` | caught — "every generated decoder is exercised" |
| TS: `editions.format` reads ordinal 1 instead of 2 | caught — "decodeEditions decodes its own ordinals" |

The second and fourth are the mutations that matter: they are the real defect
class, applied to generated code that no test had ever run, and before this
change both were silent.

`authors` has two adjacent string columns and the case gives them different
values, because a case that used `"x"` for both would pass with the ordinals
swapped. Stated rather than left to be noticed.

`sh scripts/check.sh` is 19 passed.

## What this does not do

**Still nothing decodes a row that came from the server.** Both suites build
values by hand, so a disagreement between what the server sends and what the
decoder expects — an `Int` where the schema says `Uint` — is invisible here.
The conformance suite exercises the real wire and does not use these decoders.
Unchanged from the previous entry, and the larger of the two gaps it named.

**The Python decoders have no equivalent coverage check.** `scripts/codegen.py`
generates Python too and `scripts/test_codegen.py` compiles and executes a
module built from a *synthetic* catalog — good coverage of the generator, no
coverage of the demo's generated file the way these two now have. The two
suites do not share a corpus.

**A new table still needs a hand-written case.** The check makes forgetting
loud; it does not write the case. That is the intended trade: a generated case
would be worthless.
