# The fifth table list

## What changed

One line: `shipments` added to the demo UI's `TABLES` in
`examples/explorer/web/src/api.ts`.

## Why

CI went red on three consecutive commits, in exactly one job, on exactly one
test: `the UI's column names match head.toml, in order`. Adding the
`shipments` table to `head.toml` left the browser app's hand-maintained copy
behind.

The guard worked. It parses `head.toml` and diffs it against the constant, so
the failure named the missing table and the four columns it wanted. Nothing
about the diagnosis was hard.

## The part worth recording

This is the **fifth** place the demo names its tables, and the third time in
one afternoon that adding a table broke one of them.

Two commits ago the same table exposed that each adapter listed its tables
twice — an allowlist for `/api/query` and a literal in `/api/meta` — and Go's
and Python's had already drifted. That change made each adapter derive one from
the other, which felt like the end of the problem. It was not: the browser app
keeps its own copy for labelling columns, which no adapter consults and which
no amount of tidying inside the adapters would have reached.

So the count is now: three adapter lists (each derived from one constant), the
generated declarations (derived from the catalog), and this one. Only the last
is hand-maintained with no derivation at all — and it is the only one with a
test that diffs it against `head.toml`, which is why it is also the only one
that failed *loudly* rather than serving a quietly wrong answer.

That is the right trade for a UI constant that exists to label columns in a
browser: deriving it would mean shipping a TOML parser to the client or
generating a fifth file. A guard that fails in 250ms with the exact diff is
cheaper and, on today's evidence, sufficient.

## What I did wrong

I did not run `npm test` in `examples/explorer/web` before pushing. I ran the
conformance suite three times — which exercises the adapters and never loads
the browser app — plus clippy, the Rust suites, `ruff`, `ty`, `gofmt`, `go
vet`, and `tsc` in two places. The one suite I skipped is the one that covers
the one file I broke.

The lesson is not "run everything": the demo e2e and the deployed harness are
minutes each and skipping them is usually right. It is that **a change to
`head.toml` is a change to the demo's schema**, and the demo's own suites are
the ones that read it. `CLAUDE.md` lists them; I chose the wrong subset.

## Alternatives rejected

**Derive `TABLES` from the catalog at runtime.** The UI already fetches
`/api/meta`, so it could ask for columns too, and the constant would go away.
Rejected here because it is a real change to the app's startup path — it would
need a loading state for the schema tree — and this commit's job is to make CI
green, not to redesign the demo. Worth doing; not worth doing while red.

**Generate it with `scripts/codegen.py`, like the three client declarations.**
That is the consistent answer and it is more machinery: a fourth output file,
a fourth `--check` path in CI, for a constant whose only job is labelling. The
existing test already gives the guarantee generation would.

**Nothing — delete the test.** It is the only thing that caught this.

## Evidence

`npm test` in `examples/explorer/web`: 12 tests, 12 passing, where it was 11
and 1 before. `npm run typecheck`: clean.

The failing assertion, verbatim from CI, for the record:

```
not ok 12 - the UI's column names match head.toml, in order
  error: Expected values to be strictly deep-equal:
  + actual - expected
  -   shipments: [ 'id', 'book_id', 'status', 'deleted_at' ]
```

All seventeen other jobs were green on all three runs, so the three commits
were otherwise sound — this was one stale constant, not three broken changes.

## What this does not do

The constant is still hand-maintained, and the next table to be added will
break it again the same way. The test will say so again, which is the whole
argument for leaving it.

Nothing checks that the *order* of tables matches, only the set and each
table's column order — though `deepEqual` over an object compares neither, so
the name of the test slightly overstates what it holds.
