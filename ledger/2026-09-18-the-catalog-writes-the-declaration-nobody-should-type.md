# Generating the client's schema declaration, and the two things that found

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `scripts/codegen.py`, `scripts/test_codegen.py`, `crates/slate-serverd/src/main.rs`, `examples/explorer/backends/{python/adapter/schema.py,go/schema/schema.go,go/main.go,node/src/schema.ts,node/src/main.ts}`, `.github/workflows/ci.yml`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`scripts/codegen.py` reads `slate-serverd --config <file> --print-schema` and
writes the schema declaration for the Python, Go and TypeScript clients. The
explorer's three adapters use the generated files, and a CI step regenerates
and diffs them before the conformance suite runs.

`--print-schema` now emits a decimal column's `scale`, which it did not.

## Why

A client in another language holds its own copy of the schema, because the
protocol publishes none. Getting that copy wrong is the one client mistake that
is *quiet*: a wrong ordinal points every reference past it at a different
column, and the server cannot object, because the reference is legitimate. The
fingerprint catches it on the first request — a good backstop, and still a
backstop, because by then the wrong declaration has been written, reviewed and
shipped.

Every client here typed that copy by hand. The catalog that would have written
it correctly was one flag away.

## Alternatives rejected

**Read the TOML instead of the resolved catalog.** No `slate-serverd` needed,
so the generator would run anywhere. Rejected because the TOML is a *source*
the schema layer interprets: ordinals come from declaration order, a primary
key is named and resolved, a decimal's scale is validated, a `[[tables]]`
attaches keys to whatever came before it. A generator reading it would be a
second implementation of that resolution, free to disagree with the first —
which is the disagreement this exists to end.

**Generate at build time rather than checking the output in.** No generated
files in the tree. Rejected on all three targets: Go and TypeScript would need
the Rust toolchain present at build time, the Python package would need it at
install time, and a drift would then be a *build* failure on somebody's laptop
rather than one line in CI. Committed output plus `--check` puts the failure
where a reviewer sees it.

**Generate the fingerprint as a constant too.** It is the value the check
sends, and it is right there. Rejected because the whole worth of the check is
that the client computes it from the declaration it actually holds — a
generated constant would make the check agree with itself and catch nothing.

**Generate a typed row.** `Book` instead of `list[Value]`, which is what
Drizzle and Prisma are actually known for. Not rejected, deferred, and the
comparison document says so rather than claiming the row is closed: it needs a
decoder per table in three languages, against a data literal for this.

**Leave the Go and TypeScript adapters undeclared.** They sent no schema check
before this, so wiring them is a behaviour change beyond "generate a file".
Done anyway, because a generator whose output two of three targets ignore is
not a feature, and because the conformance suite is exactly the thing that can
prove the wiring works.

## Evidence

**The generated Python declaration fingerprints identically to the hand-written
one it replaces**, on all four tables, decimal included:

```
authors 10443684941089384801   books 10683740821384460626
sales   10866677610055169381   editions 15663658928632129283
```

That is the oracle for this change. The hand-written declaration was known to
work against the live server; matching its fingerprint exactly means the
generated one does too, without needing to trust the generator's reasoning.

**`./run.sh --conformance`: 92 cases, three SDKs agree** — with all three
adapters sending a schema check, which two of them never did before.

**Eight mutations, all killed.** The four expensive ones run the whole
conformance suite:

| # | mutation | outcome |
|---|---|---|
| G1 | two columns transposed in the generated Python declaration | killed — `authors` refused, "the ordinals it would have sent name different columns here" |
| G2 | the same in the generated Go declaration | killed — the Go adapter fails at seeding |
| G3 | the same in the generated TypeScript declaration | killed |
| G4 | `--print-schema` stops emitting a decimal's scale | killed — `books` refused in every client |
| C1 | a column renamed in the checked-in file | killed — `--check` |
| C2 | the scale edited out of the checked-in file | killed — `--check` |
| C3 | the checked-in file deleted | killed — `--check` |
| C4 | a blank line appended, i.e. somebody hand-edited a generated file | killed — `--check` |

G2 and G3 are the ones that matter most, and not for the reason they were
written. They were meant to show the generated file is the one in use. What
they actually showed first is that **the Go and TypeScript adapters had been
sending no schema check at all** — the clients grew `Declaring`/`declaring`
when SchemaCheck shipped, and nobody ever handed these two a declaration to
send. Transposing their columns changed nothing until they were wired up.
Afterwards it fails at the first request.

**G4 is the other finding, and it blocked the feature outright.**
`--print-schema` omitted a decimal column's scale. The scale is the one column
property that never crosses the wire — `Units` is a count of the smallest unit
and carries no exponent — and it is in the schema fingerprint. So a declaration
built from that output is *refused* against any table holding a decimal, which
is what G4 reproduces by removing the fix. The flag whose documented purpose is
"what a client in another language has to restate by hand" was omitting the
only field nothing else could supply, and the only way to find that was to try
to use it.

**`scripts/test_codegen.py`: five tests on synthetic catalogs**, for the paths
no catalog in this repository reaches — a dropped column (which keeps its
ordinal and must still be declared, or everything after it shifts by one), an
ordinal gap, an unknown type in each of the three languages, a scale on a
decimal and on nothing else, and a composite key whose *order* is not column
order.

`gofmt -l`, `go vet ./...`, `npx tsc --noEmit`, `python3 site/check/docs.py`,
`ruff check .`: clean.

## What this does not do

- **It generates a declaration, not a row type.** A query still answers
  `Value`s. The ordinals are right now; they have not gone away.
- **Only the fingerprinted fields.** Not indexes (a client names one by name,
  and a wrong name is an error the server reports), not `added_in`/`dropped_in`
  (a client pinned to a schema version stops working at the next migration).
- **One catalog is generated from.** The explorer's. `examples/deployed` and
  `examples/batchbench` have their own `head.toml` and hand-written or absent
  declarations; nothing stops the generator being pointed at them and nothing
  currently does.
- **`--check` proves the file matches the catalog, not that anybody imports
  it.** What proves that is G1–G3, which are mutations rather than tests: no
  check in CI would notice if an adapter stopped calling `declaring`. Closing
  that means asserting a *refusal* — a deliberately wrong declaration that must
  fail — and the conformance harness has no way to express "this case should
  break".
- **Nothing generates the catalog itself.** The gap-table row above this one,
  migrations from a schema diff, is still open and is the harder half: this
  reads a catalog somebody wrote.
