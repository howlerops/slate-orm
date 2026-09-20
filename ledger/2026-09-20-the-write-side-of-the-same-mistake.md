# The write side of the same mistake

## What changed

`scripts/codegen.py` now emits an **encoder** per table beside each decoder:
`to_row()` in Python, `Row()` in Go, `encode<Table>()` in TypeScript. It also
computes the Python module's `slate.values` imports from the catalog instead of
hard-coding two names.

All three demo adapters build their predicate-write seed rows through the
encoder instead of as a positional list, so the encoders are load-bearing on a
path the conformance runner already exercises.

Each language's decoder suite gained encoder cases: a per-column tag assertion,
a null case, a decode(encode(row)) round trip per table, and a coverage guard
that fails when a generated encoder has no case.

## Why

`ledger/2026-09-19-a-row-with-names-on-it.md` shipped the typed rows and named
this: *"No write side. There is no `to_row`, so building a row to insert is
still a positional list, which is the same failure in the other direction."*

It is the same failure and in Python and TypeScript it is worse. A decoder
protects a read by checking that ordinal 3 really is `year`; on the way in
there was nothing. Worse because the *tag* is lost too:

- Python decodes both `u64` and `i64` to a plain `int`, so a caller assembling
  a row has to remember which columns want `u64(...)` and which want `i64(...)`.
- TypeScript makes both `bigint`, so the wrong one typechecks perfectly.
- Go alone keeps them apart, and still gives no help with the order.

A wrong tag is refused by the server. That is safe and it is a long way from
the mistake, with nothing naming the column.

## Alternatives rejected

**Emit the encoder and leave the adapters alone.** Half the work and it
reproduces the defect this repository keeps meeting: `2026-09-19-every-generated-
decoder-runs.md` and the entry before it both exist because something was
generated, compiled, typechecked and never called. Converting a real write path
is what makes the encoder a tested thing rather than a plausible one.

**A new demo route for encoded writes.** Cleaner to point at, and it would have
been a route that exists to be tested. The predicate-write handler already
built a books row by hand, already runs in conformance, and is exactly the code
the gap described — converting it costs nothing and covers more.

**Validate in the encoder** — refuse a `year` outside `i64`, a `status` outside
its enumeration. Rejected: the server already refuses all of it, `docs/validation.md`
argues that is where it belongs, and a client-side copy is a second
implementation of the catalog's rules that can disagree with it. The encoder
attaches tags and order; it does not have opinions.

**Hard-code the Python value imports.** What the first version did, and it was
wrong within minutes — see below.

## Evidence

**The generator emitted code that could not run, and every static check passed
it.** The first version of the Python encoder emitted `u64(...)` and `i64(...)`
while the module still imported only `Null, Units`. `ruff`, `ty`, the codegen
tests and `scripts/check.sh` were all clean; the failure appeared the first time
a row was actually encoded:

```
NameError: name 'u64' is not defined
```

The imports are now computed from the column types present in the catalog. Found
by calling the thing rather than by checking it, which is the point the decoder
entries keep making, arriving on the generator itself this time.

**Five mutations, each caught by a named test**, restored and re-verified:

| mutation | caught by |
| --- | --- |
| Go `Books.Row()` emits `AuthorId` then `Id` (same type, adjacent) | `TestEveryRowSurvivesARoundTrip/Books` |
| Python encodes `books.year` as `u64` instead of `i64` | `test_to_row_reattaches_the_tag_the_column_declares` |
| TypeScript encodes `books.year` as `uint` instead of `int` | 2 tests, including the round trip |
| a generated encoder with no round-trip case | `every generated encoder is exercised` |
| Python `Books.to_row` swaps `id` and `author_id` | **the conformance run** — see below |

**The last one is the one that matters.** With the Python encoder writing
`author_id` into `id`, all four seeded rows collapsed onto id 1 and the second
insert was refused:

```
python  {"error": {"kind": "already-exists",
         "message": "table `books` already has a row with this primary key",
         "reason": "DUPLICATE_PRIMARY_KEY"}}
```

against `go`/`node` answering normally — several cases red, including the
must-differ pair. A same-typed ordinal swap on the write side is now visible to
the three-SDK comparison, which it was not before, because before there was no
encoder to get wrong and the hand-written list was simply assumed correct.

**The three agree with the encoders load-bearing:** `./run.sh --conformance` —
99 cases, all three SDKs agree.

**Static:** `codegen.py --check` — all three generated files match the catalog.
`sh scripts/check.sh` — 20 of 20. Unit suites: Python 24, TypeScript 23, Go ok.

## What this does not do

- **The encoders check nothing about values.** A `status` outside its
  enumeration encodes happily and is refused by the server, which is the
  division `docs/validation.md` argues for. The generated `Literal` /union type
  catches it at compile time in Python and TypeScript, and not in Go.
- **Only one write path uses them.** The predicate-write seed in three
  adapters. Every other write in the demo is still a positional list, and I did
  not convert them — one converted path makes the encoders live; converting all
  of them is churn with no new coverage.
- **No `to_row` for a join or a chain**, for the same reason there is no decoder
  for one: their rows belong to no single table.
- **Nothing generates the *test* cases.** The round-trip tables are hand-written
  in three languages, with a guard that fails when a generated encoder is
  missing from one — the same trade the decoder suites made, and the same
  reason: a test the generator emitted would agree with the generator.
- **No measurement.** An encoder is an allocation and a few tag constructions
  per row on a demo path; I did not time it and would not expect to see it.
