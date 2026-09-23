# The third decoder test

## What changed

`examples/explorer/backends/python/adapter/test_rows.py` — sixteen cases over
the demo's *generated* Python decoders, the file Go and TypeScript have each
had since their own decoders shipped compiled-but-never-run. It is wired into
`ci.yml`'s `scripts` job and into `scripts/check.sh`, which is now twenty
checks rather than nineteen.

`check.sh` grew a shared `ci_env` helper: two checks now need the virtualenv
that holds exactly what CI installs, where before only `ty` did.

## Why

`ledger/2026-09-19-every-generated-decoder-runs.md` named this and named it
precisely: *"The Python decoders have no equivalent coverage check.
`scripts/codegen.py` generates Python too and `scripts/test_codegen.py`
compiles and executes a module built from a **synthetic** catalog — good
coverage of the generator, no coverage of the demo's generated file."*

The distinction is the whole point and it is easy to wave away. A generator
that is right about a catalog it invented and wrong about the real one passes
`test_codegen.py` completely. What Go and TypeScript check, and Python did not,
is that *this* file — the one the demo imports and serves requests from — reads
the ordinals *this* catalog declares.

Two of three languages having a guard is worse than none having it, because the
repository's habit is to reason about "the clients" as a unit. Every other
three-language surface here is at parity by rule; this one was at two-thirds and
the entry that said so was a day old.

## Alternatives rejected

**Generate the test.** Immediately tempting, since the same generator emits all
three declarations, and immediately wrong for the reason the Go and TypeScript
files already state in their first sentence: a test the generator emitted agrees
with the generator by construction. It would be a check that cannot fail.

**Put it in the `demo` job beside its two siblings.** That job is where the Go
and TypeScript decoder tests live, and the comment there gives a real reason —
it is the job that has just diffed the generated files against the catalog. I
put the Python one in `scripts` instead, because it needs neither the built
server nor the browser that job exists to provide, only the committed file and
an installed client. That difference is what lets `check.sh` run it: `go test
./schema/` is in the guard's `ELSEWHERE` list as "needs the generated
declaration and a server", and this one needs no server, so it is the first of
the three a contributor can run locally in one command.

**Cover only the five decoders, and skip the checks and foreign keys.** Would
have been half the file. Rejected because the Go and TypeScript tests cover
both, and a Python test that covered less would leave the parity claim still
false in a smaller way — which is harder to notice than the current, obvious
absence.

**Write it with `unittest` to avoid a dependency.** `pytest` is already this
repository's Python convention, already in `clients/python[dev]`, and already
installed by the job the step went into. A second convention for one file costs
more than the import.

## Evidence

**Four mutations of the generated `schema.py`, each caught by a named test**,
restored and re-verified after each:

| mutation | test that failed |
| --- | --- |
| `books.title` reads the year's ordinal (different type) | 6 tests, including `test_each_decoder_reads_its_own_ordinals[Books]` |
| `authors.name` reads `country`'s ordinal (**same type**) | `test_each_decoder_reads_its_own_ordinals[Authors]` |
| a new generated decoder class with no case | `test_every_generated_decoder_is_exercised` |
| `shipments.deleted_at` declared non-nullable | `test_each_decoder_reads_its_own_ordinals[Shipments]` |

The second is the one worth having. A same-typed neighbour swap is invisible to
the decoder's own type check — the Go and TypeScript tests say so explicitly —
and it is caught here only because the `authors` case uses `"Ursula"` and
`"US"` rather than the same string twice. That is a property of the test data,
not of the decoder, and it is why the case carries a comment saying so.

**The `_field` null handling has two inputs, and now both run.** `None` and
`Null()` are treated alike by the generated code; the wire can produce either
and a decoder that accepted one and raised on the other would fail in
production on whichever the demo happens not to send. Two cases, not one.

**Checks:** `python3 scripts/test_check_sh.py` — 61 steps and 5 blocks, all
accounted for. `sh scripts/check.sh` — 20 of 20, the new one among them.
`ruff check .` and `ty check` at the root both clean.

**One self-inflicted finding.** The first draft imported `Callable` from
`typing` and the root `ruff` refused it (`UP035`). That is the two-ruffs-over-
disjoint-trees gotcha from `CLAUDE.md` arriving exactly as described — and it
was caught here rather than in CI only because `check.sh` now runs both, which
is the point of the previous entry that built it.

## What this does not do

- **The decoders still never see a row the server sent.** These build values by
  hand, like the Go and TypeScript cases. The live-row coverage from
  `2026-09-19-decoders-over-the-servers-own-rows.md` exists for `books` and
  `shipments` only, and adding Python here does not widen it.
- **No live-row case for `authors`, `sales` or `editions` in any language.**
  Tracked separately; this entry is about the language that had nothing, not
  about the tables that have fixtures only.
- **The three test files do not share a corpus.** Each is hand-written against
  the same catalog, so a case added to one does not appear in the other two, and
  nothing checks that the three agree about what a valid row looks like. That is
  a fourth guard and I did not build it; the generated declarations are diffed
  against the catalog, which is the thing that would actually drift.
- **No measurement.** A test file has nothing to measure.
