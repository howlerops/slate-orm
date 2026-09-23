# A foreign key's parent table is generated, because it is the one thing about a relationship a client cannot work out

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `scripts/codegen.py`, `scripts/test_codegen.py`,
  `clients/go/slate/schema.go`, `clients/typescript/src/{schema,index}.ts`,
  the three demo adapters and their generated declarations
- **Kind:** feature

## What changed

`scripts/codegen.py` emits a table's foreign keys alongside its checks:
`SalesForeignKeys` in Go and TypeScript, `SALES_FOREIGN_KEYS` in Python, each
key carrying its name, its child table, its **parent table** and its
`on_delete`. The parent arrives from `--print-schema` as a table *id* and is
resolved to a name by the generator. A `ForeignKey` type in the Go and
TypeScript clients holds it, with `Children()`, `Parents()` and `Answers(way)`
in Go and `answers(key, way)` in TypeScript. All three demo adapters now build
their relations and their two-step path from the generated keys instead of
nine string literals.

## Why

The V3 entry ended: "Foreign keys are published and nothing generates from
them." Publishing something nothing reads is the shape of thing this repository
has been burned by twice this week — a decoder that compiled and never ran, a
workflow that existed and never fired.

The substance is narrower than "relations in three languages" and is the part
that was actually missing. `Relation` names a relationship by the child table
and the key's name and deliberately stops there: a client that *described* the
relationship could describe it differently from the next client, which is the
divergence the conformance runner exists to catch. But `Related` also takes the
table its rows decode as, separately, because the client does not hold the
catalog — and for a `parents` read that is the parent, which appears nowhere in
a `Relation`. It was a string the caller typed. The generator holds the
catalog, so it can stop being one.

## Alternatives rejected

**Generate a ready-made `Relation` per key per direction.** `SaleBookParents`
and `SaleBookChildren` as values, no new type. It reads well until the table
name is needed — which is always, because `Related` takes it — and then there
are four names per key instead of two and nothing pairs them. `Answers(way)` is
the pairing, and it needs the key rather than the relation.

**Put the parent on `Relation` itself** and let the client send it. Then the
client is describing the relationship, which is exactly what `Relation`'s own
comment refuses, and two clients could describe it differently. The server
resolves the relationship; the parent here is only for *decoding* the answer,
and keeping it off the wire keeps that distinction visible.

**A plain constant per key** — `const SaleBookParent = "books"` — with no type.
Least machinery, and it scatters: the caller assembles three constants into a
call and can still pair the wrong two. A struct is one thing to pass.

**A Python `ForeignKey` dataclass in the client, for parity with Go and
TypeScript.** Rejected because the generated Python emits checks as plain
dicts, so a typed foreign key would be the odd one out *within its own file*.
The `_answers` helper lives in the demo adapter instead. This is a real
divergence in the surface and is recorded rather than smoothed over: same three
lines, different home.

**Generate the `on_delete` and stop there.** It is the field a caller is least
likely to want and the easiest to justify omitting. Included because it costs
one string and because leaving it out would make the generated key a *subset*
of the published one, which invites the next person to wonder what else was
dropped.

## Evidence

Four mutations of the generator, against `scripts/test_codegen.py`:

| mutation | result |
| --- | --- |
| child and parent swapped | caught — 2 tests, both printing the wrong line |
| the parent id passed through unresolved | caught — 3 tests |
| a dangling parent generated as `""` rather than refused | caught — the refusal test |
| Go emits no foreign keys at all | caught — 2 tests |

Four new codegen tests, on a synthetic two-table catalog — two tables, because a
foreign key is the one generated thing that needs a *second* table to be right
about.

Eleven Go tests and fifteen TypeScript tests in the demo's schema suites, three
of them new, plus a check in each that reads the generated file and fails if a
foreign-key map is declared that the test does not look at.

Then the wiring probe, which produced a **correction**. I wrote, in five
comments, that typing the wrong table "decodes a row of one table against
another's ordinals". Making `ForeignKey.Answers` return the child either way
and running the three-SDK conformance suite showed otherwise:

    a two-step path, with the middle level kept (app): the adapters disagree
        go      {"error": {"kind": "invalid-request", "message": "the schema
                 check on table `books` does not match this catalog…"}}
        node    {"through": [[[…]]], "trees": […]}
        python  {"through": [[[…]]], "trees": […]}

Four cases red, and **the server refuses it** — `Related` sends the named
table's declaration, so the schema check sees `sales`' columns claimed for
`books` and says so at length. That is the good failure. So this is a
convenience and a removal of nine literals, not a fix for a silent bug, and the
five comments now say that. It is silent only where two tables would fingerprint
alike, which is possible and is not the case here.

The probe is also what says the adapters *use* the generated keys rather than
merely importing them: all three read the same relation and only Go was
mutated, so only Go moved.

Unmutated: 97 conformance cases agreeing, `sh scripts/check.sh` 19 passed, the
Go client suite, and the demo's Go and node suites.

## What this does not do

**No relation is generated for the reverse direction as a named thing.** A key
is one object with two readings, which is how the catalog models it and how
`Relation`'s own comment argues it should be. A caller wanting "a book's sales"
writes `SalesForeignKeys["sale_book"].Children()` — correct, and not as
readable as `Books.Sales`. An accessor per direction per table would be, and it
needs a naming scheme that survives two keys between the same pair of tables,
which this catalog does not have an example of.

**The referencing columns are not generated.** `--print-schema` publishes them
as ordinals and nothing here reads them, because nothing in the client surface
takes them: the server resolves the key by name. Generating an unused list is
what this entry exists to stop.

**Python's helper is in the adapter, not the client.** Said in the alternatives
and repeated here because it is the kind of asymmetry that is invisible until
somebody ports code between the two.

~~**A composite foreign key is untested.**~~ **Closed** —
`test_a_composite_foreign_key_generates_like_any_other` builds a two-column key
over a two-column primary key and pins the shape of the answer: *one* entry,
with no ordinal in it, exactly as a single-column key gives. The hypothesis
this paragraph labelled was right, and a mutation that emitted one entry per
referencing column is now caught rather than argued against. The demo's own
three are still all single-column.
