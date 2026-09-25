# The demo frontend's table and view lists are generated from the resolved catalog. The guard they had worked, and was itself the second implementation of catalog resolution that the generator exists to prevent.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/codegen.py`, `scripts/test_codegen.py`, `examples/explorer/web/src/catalog.ts` (new, generated), `examples/explorer/web/src/api.ts`, `examples/explorer/web/test/api.test.ts`, `.github/workflows/ci.yml`
- **Kind:** refactor

## What changed

`scripts/codegen.py` grows a fourth target, `--web`: a plain-data TypeScript
module of table and view names mapped to their column names, importing
nothing. `examples/explorer/web/src/catalog.ts` is that file, checked in and
re-checked by the CI step that already `--check`s the other three against a
built `slate-serverd`.

The web app's `TABLES` and `VIEWS` are now that module filtered by
`NOT_IN_THE_UI`, which moved from the test into `src/` because it is a UI
decision. `test/api.test.ts` loses both regex TOML parsers and `findUp`, and
keeps the assertions generation cannot make: that the roster names tables the
catalog still has, that a named table is really absent, that a view's column
list is its base table's *array object* and not a copy, and that the filter
applies to views by their own name.

## Why

Two entries recorded the same caveat — "the web app's `VIEWS` is still
hand-written" — and the honest reading of it was not "it can drift". It could
not: `test/api.test.ts` compared the hand-written maps against `head.toml`
table by table and in order, with a roster for deliberate absences. The guard
was good.

It was also thirty lines of regex over TOML, which is exactly what
`codegen.py`'s own docstring spends a paragraph refusing:

> Reading the *resolved* catalog rather than the TOML is the point: the TOML
> is a source that the schema layer interprets — ordinals come from
> declaration order, a primary key is named and resolved to ordinals, a
> decimal's scale is validated — and a generator that re-interpreted it would
> be a second implementation of that resolution, free to disagree with the
> first.

The test was that second implementation, written a week after the paragraph
warning against it, in a different package, by somebody (me) who had read the
paragraph. It agreed with the catalog because this demo's schema is simple —
no renamed column, no dropped one, nothing whose ordinal is not its position
in the TOML. The first table that exercised any of those would have had the
frontend and the test agreeing with each other and both wrong.

So the caveat was worth closing, and not for the reason it gave.

## Alternatives rejected

**Re-triage the caveat as `deliberate` and move on.** Defensible on its face:
the list is hand-written *and* guarded, and the entries that raised it both
said generating it was a larger change. It is also the move to be most
suspicious of, because it costs nothing and reads like progress. What decided
it was reading the guard instead of the caveat — a regex TOML parser is a
finding, and one nobody would have gone looking for.

**Point the web test at `--print-schema` instead of at `head.toml`.** Smaller,
same removal of the second parser. Rejected because it gives the web package a
hard dependency on a built `slate-serverd`, which it has never needed: the
whole point of that suite is that it runs in 160ms with nothing started. The
generated file moves the dependency to the job that already builds the binary
and leaves the web tests free of it.

**Have the web app import the node adapter's generated `schema.ts`.** That
file already exists and already declares every table. It also imports
`@slate-orm/client`, which is a gRPC client — into a browser bundle, to read a
list of strings. The fourth target exists precisely so that the browser's copy
can be data and nothing else; a test asserts no `import` line appears in it.

**Emit row-type columns rather than live columns.** Tempting, and wrong in the
one way that matters here. `row_columns` skips a dropped column because a
field nobody can read is noise; this module's consumer indexes a row *by
position* to put a header over it, so skipping one would shift every later
header onto the wrong value. That is the generator's founding failure arriving
through its newest target, so it is a test rather than a comment.

**Keep `NOT_IN_THE_UI` in the test.** It was there when the test *compared*
against it. The app now *filters* by it, so it has to be in `src/`, and that
is the better home anyway: a UI decision that lives in a test file is a UI
decision nobody reading the UI will find.

## Evidence

**Mutations**, all via `scripts/mutate.py`, five run, five caught, no
survivors:

| mutation | caught by |
|---|---|
| `web_module` iterates `row_columns` instead of `live_columns` | `test_the_web_module_keeps_a_dropped_column` |
| a view emits a copied column list rather than `CATALOG_TABLES["base"]!` | `test_a_web_view_shares_its_table_s_array_rather_than_copying_it` |
| the generated banner is dropped from the header | `test_the_web_module_says_it_is_generated` |
| `shown` keeps the tables `NOT_IN_THE_UI` names, rather than dropping them | three of the web tests |
| `VIEWS` is built from `CATALOG_TABLES` rather than `CATALOG_VIEWS` | `a view the UI hides is hidden by the same roster its table is` |

Each changes behaviour rather than spelling.

**A first version of `test_the_web_module_imports_nothing` asserted
`"import" not in body`** and failed — on the header comment, which says the
file imports nothing. It was failing for a reason unrelated to its subject,
which is the assertion-shaped twin of an equivalent mutation, and is written
into the test rather than quietly fixed. It checks non-comment lines now.

**Suites.** `scripts/check.sh`: 53 passed, all of them (includes
`codegen-tests`, now 37 passed 0 failed, and both web typecheckers).
`examples/explorer/web` `npm test`: **14 tests, 14 pass** — 12 before, and the
two regex parsers gone. `examples/explorer/run.sh --e2e`: **24 passed, 0
failed**, which is the check that the UI still lists the same tables: the
generated map minus `posts` is exactly the five the literal held.

**The generated file against the literal it replaced.** Identical for the five
tables the UI shows, column for column and in order — which is the null result
worth stating plainly: the regex parser had not drifted, and this change fixes
a latent disagreement rather than a live one.

## What this does not do

**The retention example's config has no web target**, because it has no web
app. `--web` is named in one CI `--check` invocation, on the demo's catalog.

**Nothing checks that `catalog.ts` is in the bundle the browser actually
loads.** The e2e exercises the rendered UI and would fail on wrong headers, so
this is covered in effect rather than directly; a test that asserted the
import graph would be asserting Vite's behaviour.

**`--web` emits no types, no checks, no foreign keys.** Names only. A browser
that wanted to validate a value before sending it would need the real
declaration, and the answer there is the adapter, which has one.

**The `EXPECTED_REFUSALS` idiom now has a fourth copy.** `NOT_IN_THE_UI` joins
the conformance runner's two rosters and `SKIP_PARTS` as a hand-maintained
list-you-are-forced-to-edit. The idiom is right and the repetition is
untracked; `2026-09-21-two-of-three-lists-were-already-guarded.md` already
carries the open caveat that nothing checks such a roster's *reasons*, and
this adds one more it does not check.
