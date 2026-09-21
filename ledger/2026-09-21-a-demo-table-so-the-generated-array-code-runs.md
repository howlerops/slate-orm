# A table in the demo whose only job is to make the generated array code run

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `examples/explorer/head.toml`, the three generated `schema.*`, the three decoder suites, `scripts/codegen.py`, `docs/orm-comparison.md`
- **Kind:** fix

## What changed

`examples/explorer/head.toml` gains a `posts` table with two array columns of
different element types, and the three committed generated declarations are
regenerated from it. Each of the three decoder suites gains a `posts` case, so
the generated array decoder and encoder are executed in Python, Go and
TypeScript rather than only compiled. Two type errors in the generated Python,
which only existed once a real generated file contained an array, are fixed in
the generator.

## Why

The previous commit's own ledger recorded this as what it did not do: the
generated Go and TypeScript array code was compiled and read and never called,
because the suites that execute generated code are the demo's own and the
demo's schema had no array column. That is the shape this repository already
closed twice — CG1 and G5 — and a new feature had quietly reopened it.

The gap is not hypothetical. Running the generated Python is what found, in one
session: a `SyntaxError` from a quoting bug (earlier), a `NameError` from a
missing import (last commit), and now two type errors. Reading generated code
catches the first class of mistake and not the second.

## Alternatives rejected

**An array column on `books`.** The first plan, and worse. `books` is named in
the Node adapter's table allowlist, the frontend's column map, the conformance
cases and three decoder suites, so widening it moves a fingerprint that four
things restate. A new table is additive — the adapters already expose a subset
of the catalog, `editions` being in neither allowlist — so nothing existing
changes. Verified rather than assumed: 101 conformance cases and 23 e2e cases
pass unchanged.

**Leaving it at the caveat.** It was written down honestly in two places, which
is the minimum and is not the same as fixing it. A recorded gap in generated
code is a gap that ships.

**Granting `posts` to `reader` as well as `app`.** Rejected: nothing in the
demo reads it, and a table reachable by a role no demonstration uses reads as
an oversight rather than a decision. The comment in `head.toml` says so.

**One array column instead of two.** A single `array<str>` is satisfied by a
generator that hard-codes `str`, which is exactly the bug the element type
exists to prevent. Two element types is the smallest schema that cannot be
passed by accident.

## Evidence

Every suite this touches, run:

| suite | result |
| --- | --- |
| `examples/explorer/backends/go` (`go test ./schema/`) | pass, 3 new array tests |
| `examples/explorer/backends/node` (`npm test`) | 32 passed, 0 failed |
| `examples/explorer/backends/python/adapter` (pytest) | 30 passed |
| `./run.sh --conformance` | 101 cases, the three SDKs agree on all |
| `./run.sh --e2e` | 23 passed, 0 failed |
| `sh scripts/check.sh` | 32 passed |
| `scripts/test_codegen.py` | 28 passed |

**The repository's own guard caught this change before I finished it.** The
Node suite's "every generated decoder is exercised" test failed with
`decodePosts is generated and nothing runs it; add it to DECODERS` — a roster
check written after a hand-written list drifted five times, doing exactly its
job on the sixth. The Python suite has the same guard and the same thing
happened. That is a better outcome than my remembering.

**Two type errors in the generated Python, found by `ty`.** Neither could exist
before this commit, because neither the repository nor CI had a generated file
containing an array:

- `_elements` narrowed its value with `is None`, which leaves `object` minus
  `None` — not something a checker will iterate. Now an `isinstance(value,
  Array)`, which narrows and is honest: `_field` returns `None` exactly when
  the column was null.
- `Array(x for x in …)` passed a generator to a constructor declared
  `Sequence[PyValue]`. It works at run time, because `tuple.__new__` consumes
  any iterable, and `ty` refuses it. A list comprehension now.

Both are the class `CLAUDE.md` describes for `ty`: a check is only as good as
the files it is given, and a file that does not exist yet is given to nothing.

**This commit also repairs the previous one, which CI caught.** `dadfe70` went
red on `codegen.py --check`: "examples/explorer/backends/python/adapter/schema.py
is not what the catalog says", and the same for TypeScript. The cause is
straightforward and was mine — that commit changed what the generator *emits*
for every catalog, because the `_elements` and `elements` helpers go into the
preamble of every generated file whether or not the schema has an array, and I
regenerated nothing. Go was unaffected only because its array support is
entirely inside the per-column emitters and adds no preamble.

Worth recording for two reasons. The check is a good one and it worked: a
generated file that disagrees with its generator is exactly the drift it exists
to find, on the two languages where the change was invisible locally because I
was testing the generator rather than its output. And it is a reminder that
"the tests pass" and "the committed artefacts are current" are different
claims; `scripts/check.sh` cannot make the second, because it needs a built
server and says so.

**No new mutations were run against the generator.** The thirteen from the
previous commit already cover every array path in `codegen.py`, and a mutation
there cannot reach these suites, which read *committed* generated files rather
than regenerating. What holds those two together is CI's `codegen.py --check`,
which fails if the committed files and the catalog disagree. That is stated
rather than demonstrated here.

## What this does not do

**`posts` is not seeded and nothing reads it through the demo UI.** It exists
for the generated code, and the decoder suites build their own rows — which is
deliberate and is rule R6 in this repository's history: a decoder test that
used the server's rows would pass whenever the server and the client were
wrong the same way.

**No array column has a nullable variant in the demo.** The generated nullable
array path — a pointer in Go, `| null` in TypeScript, `| None` in Python — is
generated correctly as far as `tsc`, `go vet` and `ty` can tell, and is
executed nowhere. `scripts/test_codegen.py` covers it on a synthetic catalog
only.

**The deployed harness and the MinIO integration were not run**, because
neither was reachable here. Both read the same catalog, so a schema change is
the kind of thing that could affect them; nothing in this change is specific to
a backend, and that is a reason to expect it and not evidence.
