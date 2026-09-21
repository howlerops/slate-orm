# A view's declaration is now generated, which closed the one read that was going out unchecked.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #258 (F5d)
- **Touches:** `scripts/{codegen.py,test_codegen.py}`, the three generated declarations, `examples/explorer/backends/{go,node,python}`
- **Kind:** feature

## What changed

`scripts/codegen.py` reads the `views` key `--print-schema` publishes and
emits, per language, the view's declaration built from the base table's
already in the file: `CLASSICS = Table("classics", BOOKS.columns,
BOOKS.primary_key)` in Python, `named("classics", Tables["books"])` in Go,
`{ ...BOOKS, name: "classics" }` in TypeScript. Each goes into its own map —
`VIEWS_BY_NAME`, `Views`, `VIEWS` — beside the tables and not among them.

All three adapters now declare the views as well as the tables, so all three
send a schema claim for a read through one.

## Why

The step-2 fix (`fingerprint::check_named`) made a view's claim verifiable, and
the F5c entry recorded who was actually sending one: the Python adapter, and
nobody else. The Go client has no catalog to build a declaration from and the
Node one returns no claim for an undeclared name, so **the view's read was the
unchecked read in two of the three clients** — which is the read most likely to
have been written against a stale idea of the base table's columns, because a
view is one more name between the caller and the table.

Generating it is what makes declaring it free. Before this, declaring a view in
the Node adapter would have meant one hand-written entry beside a generated
file, which is the drift `codegen.py` exists to remove.

The reason a view *can* be generated this cheaply is the no-projection rule:
`docs/views.md` refuses a projection in a view, so a view's ordinals are its
base table's, so a view's declaration is its base table's under a different
name. There is no per-view column list — only a name and a base — which is why
this emitter is twenty lines and not two hundred. That rule keeps paying: it
bought the plain conjunction in step 2, the base-table derivation in F5c's
adapters, and now the generator.

## Alternatives rejected

**Emit the columns again under the view's name.** The obvious shape, and it
recreates the thing generation is for: two column lists that can disagree,
generated from the same source today and edited apart tomorrow. The test
asserts the *derivation* rather than the text, and counts each column's
declaration to forbid a second one.

**One map holding tables and views.** `Declaring` takes one map and the
adapters merge, so a single generated map would have been less code at the call
sites. Refused because the two are not the same kind of thing: only a plain
query reads through a view, `/api/meta` reports them separately, and a
generator emitting a row type per entry would emit one for something with no id
and no write path. The merge is three lines in Go and a spread in TypeScript.

**Give `python_module` and friends a default `views=[]`.** Would have left
`scripts/test_codegen.py`'s 62 call sites untouched. Refused for the reason the
step-2 entry gives about fixtures: a default makes a future emitter silently
forget views, and the churn is one mechanical edit. The call sites now pass
`[]` explicitly and the new cases pass a real view.

**Emit the view block unconditionally.** An empty `Views` map and, in Go, an
unused `named` helper in every generated file. `gofmt` and the Go compiler are
both happy with that, so nothing would report it — which is exactly the shape
of `test_a_catalog_with_no_array_column_gets_no_array_helper`, and the reason
that case has a sibling here.

## Evidence

**The claim really is sent now, demonstrated rather than read.** Three
full-stack mutations, each a wrong declaration in a generated file, each caught
by the three-SDK conformance runner disagreeing:

- the Go client's view declaring `Tables["authors"]`;
- the Node client's view spreading `AUTHORS`;
- the Node client's view keeping `BOOKS`'s *name* — which is the sharpest of
  the three, because it proves the server verifies under the **view's** name:
  a claim hashed as `books` is refused for a read of `classics`, which is
  `fingerprint::check_named` doing the one thing it was added for.

Before this commit all three of those would have passed, because two of the
three clients sent no claim at all.

`scripts/mutate.py` could not score those, and this one is true: the conformance
runner printed `130 cases: the three SDKs agree on all of them`, which is not
libtest, not this repository's `test_*.py` format and not pytest, so the script
reported "no test results at all" and refused — observed, unlike the claim
about the frontend in the F5c entry, which has been withdrawn there. A second
scratch harness applied the same four protections and the working tree was
never left mutated.

**Fixed immediately afterwards, and not by a dialect.** The runner now prints
`FAIL  <what>` per finding and a closing `N passed, M failed`, which is the
house style `mutate.py`'s existing `python` dialect already reads —
the same fix `test_codegen.py`'s entry settled on, for the same reason. The
mutation above re-ran through `mutate.py` proper and is caught, naming all
three disagreeing cases and the must-differ pair.

`python3 scripts/test_codegen.py`: 33 passed, four new —

- the view is derived in all three languages, and each column is *declared*
  once, matched by each language's declaration spelling rather than by the bare
  name (the bare name also occurs in the generated row type, so counting it
  would make the case pass or fail for reasons unrelated to views);
- a catalog with no view emits no view block, and no `named` helper;
- a view over a table the generator does not emit is refused, with the
  never-fires half beside it;
- a catalog publishing no `views` key at all reads as none, which matters
  because `SLATE_SERVERD` points these scripts at prebuilt binaries by design
  and one built before the key exists is perfectly valid.

**A mutation found a test that read a string instead of running it.** The first
version asserted `"VIEWS_BY_NAME" in python`, and commenting the assignment out
survived it — the string is still in the file. The case now compiles and
executes the generated module, reads `VIEWS_BY_NAME["classics"]`, and asserts
its columns are the *same objects* as `BOOKS.columns`. Six generator mutations
in total, all caught after that fix: the Python view declaring no columns, the
Go view built fresh rather than renamed, the TypeScript view keeping the base
name, `VIEWS_BY_NAME` commented out, `VIEWS_BY_NAME` keyed by something other
than the name, and both "emitted unconditionally" cases.

`examples/explorer/run.sh --conformance`: 130 cases, the three agree.
`--e2e`: 24 passed. `codegen.py --check` against the demo config and against
the retention example's: all four generated files match. `sh scripts/check.sh`:
34 passed. `pytest examples/explorer/backends/python/adapter`: 30 passed.
`gofmt`, `go vet`, both TypeScript typecheckers: clean.

## What this does not do

**No client library gained a `Table.as_view(name)`.** Each generated file
builds the declaration inline. A public helper in three SDKs is a separate
change with its own docs and tests, and the generator removes the need for one
in the case that actually occurs — a client generated from a catalog.

**The web app's `VIEWS` is still hand-written.** `examples/explorer/web` does
not use the generated declarations at all — its `TABLES` is a hand-written copy
guarded by a test against `head.toml` — so its view map is hand-written under
the same guard. Bringing the web app onto generated declarations is a larger
change than this one and has nothing to do with views.

**Nothing generates a row *type* for a view.** Deliberate: the base table's
row type already decodes a view's rows, because the columns and the ordinals
are the same. A second identical type would be waste, and a caller who wants
one can use the table's.

**A view over a view is still unreachable**, so the generator has never emitted
one and the refusal it would need does not exist. Unchanged from step 2.

**The Go `named` helper is unexported and generated.** If a catalog ever
declares a view whose base table is absent, `declared_views` refuses before any
file is written — but nothing checks the *emitted* Go compiles, here or in
`test_codegen.py`; the Go and TypeScript cases read the text and the demo's own
build is what compiles it. That is the weakness `test_the_go_and_typescript
_decoders_check_an_array_element_by_element` already names for arrays, and it
is the same one.
