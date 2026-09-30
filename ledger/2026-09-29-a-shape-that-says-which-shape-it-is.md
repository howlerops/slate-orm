# `--print-schema` says which shape it is, and the one program that reads it refuses a shape it does not know

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-serverd/src/main.rs`, `scripts/codegen.py`,
  `scripts/test_codegen.py`, `docs/caveat-status.json`,
  `scripts/check_closed_caveats.py`
- **Kind:** feature

## What changed

`--print-schema` publishes `"format": 1`, and two things hold it honest:

- **`the_published_shape_and_its_version_move_together`** pins every key the
  output can emit, in both directions, against a fixture built to reach every
  branch. Adding a key fails it until `SCHEMA_VERSION` moves in the same commit.
- **`codegen.py` refuses a format it was not written against.** It is the one
  program that parses this output, and it emits positional row types for three
  languages.

Three cases in the generator's suite, one of them through the real subprocess
path. 43 tests, up from 39; the daemon's 204 still pass.

## Why

`ledger/2026-09-20-the-column-the-catalog-knew-about.md` left it `open`:

> **`--print-schema`'s output is not versioned.** A consumer parsing it gets a
> new key without warning. True before this change and unchanged by it, but
> worth saying now that the shape has grown twice in two days.

Two days, two keys, one consumer that generates decoders **by ordinal**. A
server that renamed or dropped a key `codegen.py` reads would produce three
languages' row types that compile and are wrong about which column is which —
the worst kind, because nothing fails until a value is read.

**A version alone would have been worse than nothing**, because a consumer
branches on it and is wrong. The two halves above are what make it a contract
rather than a constant somebody is supposed to remember, and the second is what
stops it being the "generated, compiled, never called" shape this repository has
met five times.

## Alternatives rejected

**Call it `version`.** Each table already publishes `schema_version` — its own,
from the catalog — and two keys one level apart with near-identical names is a
consumer reading the wrong one. `format` says what it versions: the document,
not the data.

**Let `codegen.py` accept anything up to the version it knows.** That is this
file guessing at a compatibility rule the server does not offer. `SCHEMA_VERSION`
says a *removed* key bumps it too, so "later is a superset" is false by
construction; `KNOWN_FORMATS` is a set for that reason.

**Treat an absent `format` as 1.** Tempting and wrong in the one direction that
matters. The shape grew twice before anybody counted, so a document without the
key is *some shape from before the counting started*, not the first one.
Defaulting would read those as current, which is the case the version exists
for. A test pins the refusal.

**Derive `PUBLISHED_KEYS` from one run of the fixture.** It would never fail,
because the roster and the output would be computed from the same thing — the
defect `ledger/2026-09-28-five-column-types-no-pinned-table-carried.md` met,
where a coverage guard computed its own subject and always agreed with itself.
The roster is written out, and the second assertion catches a fixture that
stopped reaching a branch.

**Use the demo's `head.toml` as the fixture.** It is edited to show a product,
so a branch would stop being covered the day somebody simplified it.
`EVERY_BRANCH` has one job and its comment says so.

**Pin key paths rather than key names.** Stronger, and it means a second
description of `describe` to keep in step. A key that moved between nesting
levels passes today — named below as the hole it is.

## Evidence

`cargo test -p slate-serverd --bin slate-serverd`: **204 passed**.
`python3 scripts/test_codegen.py`: **43 passed**, from 39.
`sh scripts/check.sh`: **87 passed**. `ty` against the CI virtualenv: clean.

The roster holds **36 keys** across five levels — the document, a table, a
column, an index, a check, a foreign key and a view's lowered spec.

Mutations against the daemon, record
[`ledger/mutations/20260929T212111-crates-slate-serverd-src-main-rs.json`](mutations/20260929T212111-crates-slate-serverd-src-main-rs.json):

| mutation | outcome |
| --- | --- |
| the shape grows a `row_count_hint` key and the roster is not told | caught, `the_published_shape_and_its_version_move_together` |
| the `format` key stops being published | caught, same |
| the fixture stops reaching the foreign-key branch | caught, same |

The third is the one worth having: it proves the *second* assertion works, so a
fixture that quietly stopped covering a branch cannot leave the roster pinning
a subset.

And against the generator, records
[`…212323-scripts-codegen-py.json`](mutations/20260929T212323-scripts-codegen-py.json)
and
[`…212524-scripts-codegen-py.json`](mutations/20260929T212524-scripts-codegen-py.json):

| mutation | outcome |
| --- | --- |
| an absent format is read as the first one | caught, `test_a_print_schema_with_no_format_is_refused` |
| the generator stops checking the format at all | **survived at first** |
| an unknown format warns instead of refusing | caught, all three |

**The survivor is the finding.** Deleting the *call* from `main` and leaving
`refuse_unknown_format` intact passed every test, because all three called the
function directly. That is exactly the shape the function's own docstring names
— correct, compiled, never called — reproduced inside the change that cites it.
`test_the_generator_checks_the_format_before_it_reads_a_table` drives `main`
end to end against a three-line script standing in for `slate-serverd`, and the
mutation is caught.

That test is not a monkeypatch: `ty` refuses to assign anything to a module's
function attribute, which was the first attempt and is a better outcome — the
subprocess call and the JSON parse are now exercised too, which is what a wrong
`--serverd` on a real machine would hit.

### A `cargo fmt` lie, met again

The roster was written as a multi-line list, `cargo fmt` collapsed it to one
line, and a later scripted edit to replace it matched nothing — silently,
because that one replacement was the one where I had not asserted the count.
The test then reported `tables` and `views` as unpublished, which is impossible,
and the two minutes spent not believing it found the stale placeholder. This is
lie #1 in `scripts/mutate.py --help`, arriving outside a mutation run.

## What this does not do

**It pins key names, not paths.** A key that moved between nesting levels — a
column's `scale` appearing on the table instead — passes. Modelling the nesting
means a second description of `describe`, and the failure this exists to catch
is an added key.

**The demo's three adapters do not check the format.** `codegen.py` refuses, and
`examples/explorer/backends/{go,node}` and `web/src/api.ts` read the generated
output rather than the printed schema, so they are downstream of the refusal.
Nothing checks that stays true.

**Version 1 is where the shape is today, not a promise about tomorrow.** The
semantics are deliberately weak — added, removed and renamed keys all bump it —
because anything stronger is a compatibility guarantee this project is too young
to keep. A consumer cannot infer from `format: 2` that what it read at 1 is
still there.

**Nothing publishes the format anywhere a human reads it.** `docs/validation.md`
and `docs/views.md` describe what the output contains and neither mentions the
version. A reader learns about it from the refusal or from the source.
