# A row with names on it

## What changed

`scripts/codegen.py` now writes a typed row per table beside each schema
declaration: a frozen dataclass with `from_row` in Python, a struct with
`ScanBooks` in Go, an interface with `decodeBooks` in TypeScript. The three
checked-in files are regenerated and CI's existing `--check` covers them.

## Why

The comparison document called this "the remaining half" of the codegen row and
"a bigger and better change than anything in the second six". The first half —
generating the *declaration* — made a client's ordinals right. It did nothing
about what happens after the rows arrive, which is a loop indexing a list by
position, written once per call site, in three languages.

That loop is where the failure the generator exists to prevent actually lands.
A declaration that is one column out is caught by the fingerprint. A *caller*
that is one column out is caught by nothing: `values[2]` is a legal expression
whatever sits there, and if the neighbouring column happens to be the same type
the result is populated, plausible and wrong.

So the decoders check rather than cast. Every field asserts its tag and raises
naming the table and the column, which turns the silent case into a loud one at
the point of decode instead of somewhere downstream. That check, not the type
annotation, is what the generated decoder buys — a hand-written struct literal
would have the types too.

## Alternatives rejected

**De-pluralise the table name.** `books` → `Book` reads better and is a guess
about English. It is wrong on `data`, `series`, `status` and every domain noun
already plural, and a generator that renames a table behind the caller's back
is worse than one whose names are dull. `books` becomes `Books`.

**Go's initialism convention.** Go style wants `ID`, not `Id`. Following it
needs a list — ID, URL, API, UUID, HTTP — that is never complete and is a
second thing to keep in step with; a field whose name depends on whether `api`
made the list is worse than one that is uniformly mechanical. Title-cased per
underscore-separated part, and said so in the generated comment.

**Skip dropped columns entirely, in both outputs.** Consistent and wrong. The
declaration must keep them because the fingerprint hashes which columns are
dropped; the row type must not, because a field nobody can read is noise. The
two genuinely differ, and the columns *after* a dropped one must still decode
from their real ordinals — which is the bug `row_columns` could most easily
have, so it is the one the test targets.

**Return the typed row from the query.** What Prisma does, and it is a client
change in three languages rather than a generator change: the decoder would
have to be threaded through every read, including joins and chains, whose rows
belong to no single table. Left undone and said plainly below.

**`cast(str | None, …)` in Python.** Builds a union object per row at run time.
`Optional[...]` instead leaves an unused import on any catalog with no nullable
column, which ruff then strips out of the generated file — and a generated file
a linter edits is drift by Tuesday. The cast target is a string.

## Evidence

Six mutations of the generator, all caught by `scripts/test_codegen.py`, which
CI already runs:

| mutation | caught by |
| --- | --- |
| a row type keeps dropped columns as fields | `…skips_a_dropped_column_but_not_its_ordinal` |
| **decode by position in the filtered list, not the real ordinal** | the same test |
| a nullable column is not optional | `…optional_in_every_language` |
| the per-column type check is skipped | `…refuses_a_transposed_one` |
| a null in a non-nullable column becomes `None` | `…comes_back_null_is_refused` |
| the table name is de-pluralised | `…not_de_pluralised` |

The Python decoder is **executed**, not pattern-matched: the test compiles the
generated module, decodes a good row, and then decodes a transposed one and
requires it to raise naming `t.id`. Without the per-column check that
transposition returns a populated object.

All three outputs were verified the way CI verifies them: `ruff check` and
`ty check` against a clean virtualenv holding only `clients/python[dev]`,
`gofmt -l`, `go build` and `go vet` on the explorer's Go backend, and
`tsc --noEmit` on its Node backend. `codegen.py --check` reports no drift.

Two things came out of running the checks rather than reading them. The first
Go output was not `gofmt`-clean — struct fields unaligned and a double blank
line — so the generator now pads to the longest field name. The first Python
output tripped `RUF022` and `UP037`: ruff groups `AUTHORS` and `Authors` as two
runs rather than interleaving them, and the quoted self-referential return type
is redundant under `from __future__ import annotations`. Both were the
generator's fault, and both would have been a red CI job on a file nobody had
touched.

## What this does not do

Nothing returns a typed row. A query still answers `Value`s and the caller
passes them to the decoder, so this is a tool picked up rather than a return
type handed over — the Prisma experience is a client change in three languages,
not a generator change, and it is not started.

Joins and chains get nothing. Their rows belong to no single table, so there is
no table to name a type after, and the generator does not try.

No write side. There is no `to_row`, so building a row to insert is still a
positional list, which is the same failure in the other direction and was not
in this change's scope.

The Go and TypeScript decoders are **compiled but not executed** by any test.
Python's is exercised directly; the other two are covered only by
`go build`/`go vet` and `tsc`, which prove they type-check and not that they
decode correctly. A conformance case that reads a row through each generated
decoder is the obvious next step and is not here.
