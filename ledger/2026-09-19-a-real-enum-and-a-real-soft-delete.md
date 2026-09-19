# A real enum, and a real soft delete

## What changed

The demo schema gained a `shipments` table: a `status` column constrained by a
`CHECK` to three values, a `deleted_at` soft delete, and a foreign key into
`books`. The seeder writes four rows and retires one.

And a generator bug it immediately found: a narrowed Python type inside the
decoder's quoted cast produced a module that would not parse.

## Why

Three entries from the last few hours each ended by admitting the same gap in
different words. The enum narrowing was "proved only against synthetic
catalogs". `include_deleted` had "nothing in the demo or the conformance corpus
using it". `purge_deleted` was untested outside `MemoryStore`. All three were
short of the same thing — **a real table, in a real catalog, that a real server
serves**.

One table supplies it for all three, which is why this is one change and not
three. A new table also disturbs nothing: nothing in the demo asserts how many
tables there are, and the existing four keep their ids.

## The bug, which is the point of the entry

The demo would not start:

```
status=cast("Literal["pending", "shipped", "delivered"]", …)
                     ^^^^^^^^^  SyntaxError
```

The Python decoder casts to a **quoted** type — `cast("int | None", …)` —
deliberately, so it does not build a union object per row. A `Literal["…"]`
closes that string early, and the whole generated module becomes a syntax
error.

**Every check passed on it.** `ruff` never parses a file it is not handed, and
the generated adapter schema is not in its path. The codegen tests asserted on
the *annotation* line — which is real code, correctly double-quoted — and never
on the cast. And the one test that compiles generated Python used a catalog
with no enumerated column. Three checks, each of which could have caught it,
each looking slightly past it.

What found it was starting the demo, which is the only thing in this repository
that had ever imported the result.

The fix is one character: single quotes inside the cast string, double quotes
in the annotation. Two spellings of one type, each correct where it sits, with
a comment saying why they differ.

## Alternatives rejected

**Add the enum to an existing table instead.** `books.title` and `authors.name`
are not enumerations and pretending otherwise would be a demo that teaches the
wrong shape. Adding a *column* to `books` would also have changed its
fingerprint, its seed rows and every adapter's hand-written queries, for a
column none of them use.

**A weak check again, like `year_is_positive`.** That one exists to prove the
plumbing and is deliberately unfalsifiable by the demo's own data. A second of
those would have proved the plumbing twice and the narrowing not at all.

**Stop casting to a string in the Python decoder.** It would have removed the
quoting problem at the source. Rejected because the comment above it already
records why it is a string — `cast(str | None, …)` builds a union per row — and
the alternative import is unused on most catalogs, which is the drift failure
this session has now hit three times.

**Seed a row already retired.** Not possible, and worth knowing: the write path
checks the writer could read back what it wrote, and a row born deleted fails
its own soft-delete filter. The seeder inserts live and then deletes, which is
also the only path that stamps the server's clock.

**Give `reader` the `read_deleted` grant.** It would make the demo show more.
It would also delete the only demonstration that the grant is separate from
`read`, which is the entire reason the action exists.

## Evidence

The narrowing, in the real generated files rather than a test fixture:

| language | what is generated |
| --- | --- |
| Python | `status: Literal["pending", "shipped", "delivered"]` |
| TypeScript | `status: "pending" \| "shipped" \| "delivered"` |
| Go | `var ShipmentsStatusValues = []string{…}`, field stays `string` |

Two mutations of the quoting, both caught: the cast reverting to double quotes
(the original bug) and the annotation switching to single quotes (which the
existing narrowing test pins, because ruff wants double quotes in real code).

`./run.sh --conformance`: **92 cases, the three SDKs agree on all of them**,
with the new table seeded and one row retired. That is the check that matters
here — the adapters share one database and a bad declaration refuses every case
naming that table.

`ruff check`, `ty check` against a clean virtualenv, `gofmt`, `go vet`,
`go test ./schema/`, `tsc --noEmit`, and the docs-site check: all clean.

## What this does not do

Nothing **reads** the new table yet. No conformance case queries `shipments`,
no adapter has a handler for it, and the demo UI shows it only in the schema
tree. It exists so the next two changes have something to act on, and on its own
it proves the generator and the catalog rather than any query.

The retired row is invisible to every current caller, because no client can ask
for it. That is the next entry's job; until then the only way to see row 603 is
the kernel.

The Go decoder's allowed-values slice is generated and unused — nothing
validates against it, because the server already does.

No test asserts the demo's *generated* schema module imports cleanly; the new
codegen test compiles a synthetic catalog with the same shape. The demo
starting is what covers the real file, and that runs only in the conformance
job.
