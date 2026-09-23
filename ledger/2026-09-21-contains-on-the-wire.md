# `contains` crosses the wire, and two mutation-runner holes found while proving it

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, working F6a
- **Touches:** `records.proto` (both copies), `slate-server/src/convert.rs`,
  `slate-serverd` (`config.rs`, `schema.rs`), `slate-kernel/src/expr.rs`, the
  Python, Go and TypeScript clients, `clients/python/testserver`,
  `scripts/mutate.py`, `scripts/test_mutate.py`, `docs/full-text.md`
- **Kind:** feature

## What changed

Full-text search stops being kernel-only. `Expr.contains` joins the `Expr`
oneof in `records.proto`, carrying the **search text and not a term list**;
`convert.rs` converts both ways; `text = true` declares an inverted index in
`slate-serverd`'s TOML schema; and `ColumnRef.contains` / `slate.Contains` /
`contains` search one from Python, Go and TypeScript, each with a live test
against a real node.

Two things came along because the tests needed them. Go and TypeScript had
**no access hint at all** — `Query.Hint` with `UsingIndex`/`UsingTableScan`,
and `query.hint` with `usingIndex`/`usingTableScan`, close a three-client
asymmetry that Python has had since it was written. And `Expr::NODE_NAMES` /
`Expr::node_name` now live in `slate-kernel`, pinned to the enum by a
wildcard-free match, so the wire round trip's generator-coverage guard reads
the enum instead of a hand-written list.

## Why

The kernel could search an inverted index and nothing outside it could ask.
That is the same boundary F3a and F2a stopped at, and the same fix: the wire,
the three clients, and the configuration file a server is actually started
from.

The wire carries the caller's text rather than terms because the alternative is
four tokenizers — Rust, Python, Go, TypeScript — and a client that split
differently would find *fewer rows than the table holds*, with no error
anywhere and only a comparison against a table scan to say so. Sending text
makes that unrepresentable: the server tokenizes with the same function its
write path tokenized the column with.

The access hint is not decoration. A text index is not chosen by cost at test
scale — `docs/full-text.md` measures the crossover at roughly one row in 24,000
— so an unhinted `contains` over four rows is a table scan, and a Go or
TypeScript test written that way would have passed with the index deleted. The
hint is what lets those tests reach the index at all.

## Alternatives rejected

**A `repeated string terms` field instead of `string text`.** Cheaper on the
server: no tokenizing per request. Costs the property above, which is the only
one that fails silently. A client is also the *worst* place to put a tokenizer,
because it is the place least likely to be updated when the server's changes.

**A new table for the full-text fixtures rather than an index on `posts`.**
The repository's own rule, stated in `testserver/src/main.rs` for `prices`,
`posts` and `libraries`, is a new table when a feature needs a new *column* —
because a column changes the fingerprint and therefore all three clients'
declarations of that table. An index does not: the fingerprint deliberately
covers what decides how bytes are read, and indexes are not that. So `posts`
gains `by_title_text` at no cost to any declaration. The Go and TypeScript
suites declare their own `articles` table in TOML instead, which is what makes
them a test of `text = true` and not only of the client.

**Leaving Go and TypeScript hint-less and testing `contains` unhinted there.**
This was the tempting one: the task asked for a predicate, not a hint, and
Python already covers the index. Rejected because an unhinted test is a test of
a table scan wearing the word `contains`, and this repository has been bitten
by exactly that twice in this feature already — the kernel's own full-text
oracle and its sort test were both vacuous when first written, for the same
reason. Two small additions buy three non-vacuous test files.

**Adding `CONTAINS` to the SQL front end in the same change.** Deferred, not
refused. The wasm binding has no corpus a text index would be chosen over, so
the keyword would ship as a scan with a nicer spelling, and the demo has no
search box to put it in. Both are named in `docs/full-text.md` as what is left.

## Evidence

Every surface mutation-tested with `scripts/mutate.py`, all caught:

- `convert.rs`: sending no search text, joining the terms with no separator,
  and resolving the column to ordinal 0 — each caught by
  `a_predicate_survives_the_round_trip` and two others.
- `slate-serverd/schema.rs`: dropping `if index.text` — caught by all three new
  `schema::tests::a_text_index_*`.
- Python: empty text, the client splitting the search itself, the wrong column
  — caught by four, one and four cases of `test_fulltext.py`.
- Go: empty text, truncating the search, dropping the hint, and
  `UsingTableScan` asking for an index — caught by four, one, one and one.
- TypeScript: the same four — caught by four, one, one and one.

Suites: Python 325 passed (was 318 before F6a's client half; 7 new). Go
`go test ./...` green. TypeScript 188 passed (was 182). `slate-kernel`,
`slate-server`, `slate-serverd` and `slate-schema` green.
`sh scripts/check.sh`: 34 passed, all of them.

**Two findings in `scripts/mutate.py` itself, both met while using it here, and
both the "nothing ran, scored as clean" failure its own docstring calls mode 2:**

1. The `pytest` dialect matched `FAILED` and not `ERROR`. A stale
   `SLATE_TESTSERVER` made every test error in *setup*; pytest summarised that
   as `7 errors in 0.11s`, which the report marker matched, so **two genuine
   mutations were reported as survivors.** Fixed, with
   `a pytest ERROR counts as a catch, not as a clean run`.
2. The `go` dialect's report marker demanded a package name after the verdict,
   specifically to reject a bare `FAIL` from a build error. But `go test` on a
   package that does not compile prints
   `FAIL\tgithub.com/x/y [build failed]` — a per-package line with a package
   name, matching exactly. A Go mutation using `strings.Split` in a file that
   does not import `strings` **scored as a survivor.** Fixed, with
   `a go build failure with a package name is not a suite that reported`.

`scripts/test_mutate.py`: 17 passed (was 15).

**And a third, in the wire round trip.** `the_expression_generator_reaches_every_variant`
compared `any_expr()`'s output to a **hand-written list of eleven names** and
`collect_variants` had a `panic!` wildcard — the same shape the file's own prose
calls out two tests further down, where a generator that never produced a vector
let the vector path pass by never being tried. `Expr::Contains` had been on the
wire for a whole commit's worth of work with the round trip never converting
one, which is why the empty-search-text mutation survived the first run. The
guard now reads `Expr::NODE_NAMES`, minus `in_sorted`, which is a planner form
nothing sends.

## What this does not do

- **No `CONTAINS` in the SQL front end and no search box in the demo.** Named
  above and in `docs/full-text.md`.
- **No conformance case.** F2b added window cases to the three-SDK runner;
  `contains` has none, because the demo's catalog has no text index to point one
  at. That is the same follow-up as the demo search box.
- **The planner still prefers a table scan** at every corpus size measured. That
  is `docs/full-text.md`'s finding, unchanged by this work, and the reason every
  test here hints.
- **The Go and TypeScript hints are untested beyond this file.** They are used
  by the two full-text oracles and nothing else; in particular nothing checks
  that a hint naming an index the table does not have produces a warning, which
  Python's `test_explain.py` does check.
- **`slate-serverd`'s fixture `python-client.toml` gained no text index.** It
  mirrors the first six testserver tables and `posts` is not among them, so
  `tests/fixture.rs` does not exercise `text = true`; the three `schema.rs` unit
  tests and the Go and TypeScript TOML do.
