# The column the catalog knew about

## What changed

`slate-serverd --print-schema` publishes `soft_delete` as an ordinal, beside
`tenant_column`. `scripts/codegen.py` reads it and emits, for a soft-deleting
table only, a way to ask whether a row is retired: a `retired` property in
Python, a `Retired()` method in Go, an `isRetired<Table>()` function in
TypeScript. Two tests in the Python suite — the positive and, more usefully,
the negative.

## Why

`ledger/2026-09-19-a-row-that-is-gone-but-still-there.md` shipped soft delete
and named this: *"`scripts/codegen.py` does not know the column is special,
because `--print-schema` does not publish `soft_delete`."*

The catalog knows `shipments.deleted_at` is the retirement stamp. The client
saw a nullable `i64` called `deleted_at` and had to be told, out of band, that
*this* one means gone. Everything downstream inherited the ignorance: a client
could set `include_deleted` and then not know which column decided anything,
and no generated type could offer `restore` without hard-coding a name.

Publishing an ordinal rather than a name, because that is what
`tenant_column` publishes and what every other positional thing here uses — a
name in the JSON would be a second spelling of the same fact.

## Alternatives rejected

**Key the generator on a column named `deleted_at`.** No server change at all,
and the convention would then fire on any table with a column of that name for
its own reasons — an audit log recording when *somebody else's* record was
deleted is the obvious one. This is the same argument the derive attribute made
two entries ago, and the negative test here is its twin: `books` has no soft
delete and must not claim one.

**Publish the whole `TableDef`.** `--print-schema` is deliberately a projection:
it publishes what a client needs to declare and to generate from, not the
server's internal shape. `soft_delete` qualifies on both counts; the planner's
statistics, say, do not.

**Generate a `restore()` as well.** The obvious next step and not taken here.
Un-deleting is an ordinary update that writes null back, so a generated
`restore` would be a write helper — and this repository has just learned, in
`2026-09-20-the-write-side-of-the-same-mistake.md`, that a generated write
surface wants an encoder, a caller, and a round-trip test in three languages.
That is its own change. What it needed first was for the client to know which
column to write, which is this one.

**A boolean column in the published JSON per column** (`"soft_delete": true` on
the column) rather than an ordinal on the table. Equivalent information, and it
scatters a table-level fact across the column list where two columns could
claim it and nothing would object.

## Evidence

**The ordinal is published and only where it applies.**
`--print-schema` over the demo catalog: `authors`, `books`, `sales` and
`editions` report `soft_delete: None`; `shipments` reports `3`, which is
`deleted_at`.

**The generated code follows it, and only there.** One `retired` in the Python
module, one `Retired()` in Go, one `isRetired…` in TypeScript — all on
`Shipments`. `examples/retention/schema.py`, generated from a different catalog
whose single table soft-deletes, gets one too.

**A mutation of the publisher changes the generated output.** Making
`--print-schema` always emit `null` for `soft_delete` produced a module with
**0** `retired` properties where the committed one has 1 — so
`codegen.py --check`, which CI runs before the conformance suite, fails on the
difference. The Python test `test_a_soft_deleting_table_says_which_rows_are_
retired` fails too, once the file is regenerated.

**The negative test is the one worth having.**
`test_only_a_soft_deleting_table_gets_the_property` asserts `Books` has no
`retired` attribute. Without it, an emitter that keyed on the column *name*
rather than the published ordinal would pass everything else here.

**Suites:** Python decoder suite 26 passed. `gofmt`/`go vet` clean, `tsc`
clean. `codegen.py --check` — all three generated files match the catalog.
`sh scripts/check.sh` — 20 of 20. `./run.sh --conformance` — 99 cases, the
three SDKs agree. `examples/retention/run.sh` — exit 0 with its regenerated
module.

## What this does not do

- **There is still no `restore`.** Named above as its own change, and it is now
  the last item left in the entry this closes half of.
- **No Go or TypeScript test for the new method.** The Python suite covers the
  positive and the negative; the other two compile it and the conformance
  corpus does not exercise it, because no adapter route calls it yet. That is
  the "generated, compiled, never called" shape this repository keeps meeting,
  and I am naming it rather than claiming otherwise — the honest position is
  that Python's coverage is real and the other two are typechecked.
- **Nothing uses it in the demo.** The retired-row route reads `deleted_at`
  directly, as it did before. Converting it would be a one-line change per
  adapter and would make the property load-bearing; I stopped at the generator
  because the recorded gap was about the generator.
- **`--print-schema`'s output is not versioned.** A consumer parsing it gets a
  new key without warning. True before this change and unchanged by it, but
  worth saying now that the shape has grown twice in two days.
