# CONTAINS in the SQL front end, and a withdrawn argument for not having it

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, finishing F6b
- **Touches:** `crates/slate-wasm/src/{sql.rs,lib.rs,fixture.rs}`,
  `crates/slate-wasm/tests/{fulltext.rs,sql.rs,playground.rs}`,
  `docs/full-text.md`, `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

`WHERE title CONTAINS 'the heaven'` parses, lowers to `Expr::contains`, and
runs in the workbench. The fixture's `books` gains an inverted index on
`title`. Seven hand-written cases, and `contains` joins the SQL round-trip
property's operator list so every generated statement can carry one.

## Why

The kernel, the wire, three clients and a TOML schema could all express a
full-text search and the editor on the front page could not.

**And the argument for deferring it was wrong.** F6a's ledger entry said a
`CONTAINS` the planner answers with a table scan — which it does here, at 4,848
books, with no hint syntax to overrule it — would be "a scan with a nicer
spelling". `a_contains_is_not_a_like` withdraws that:

- `LIKE '%Cosmic%'` finds *Cosmicomics*; `CONTAINS 'Cosmic'` does not, because
  a term is a whole word.
- `CONTAINS 'heaven the'` finds *The Lathe of Heaven*; `LIKE '%Heaven%the%'`
  does not, because a pattern is ordered and terms are not.

Neither is expressible as the other. What an access path costs and what a
predicate means are different questions, and the deferral conflated them.

## Alternatives rejected

**`CONTAINS(title, 'x')`, the function-call spelling SQL Server uses.** It
would need its own parse path and would put a column inside an argument list,
which nothing else in this grammar does. Infix drops into `comparison_tail`
beside `LIKE` and `~` — a column, an operator, a literal — and costs four
lines. Postgres spells it `to_tsvector(title) @@ to_tsquery('x')`; neither is
standard, so there was no spelling to be faithful to.

**Adding a hint syntax so the workbench could reach the index.** That is a
bigger feature than this one and would be the tail wagging the dog: the
keyword's value is the predicate, not the plan. The fixture's index is there
so the schema tree shows one and the executor maintains one, and the fact that
the planner reads the table is stated in the fixture rather than hidden.

**Leaving the fixture without a text index.** Then nothing in the workbench
would ever write a per-term entry, and the one path that exercises the write
side in a browser would not exercise it.

## Evidence

Four mutations, all caught:

- `CONTAINS` lowering to `Expr::like` of the same text → four cases;
- lowering with an empty search → four cases;
- the binding splitting the search and sending one term → `two_terms_are_a_conjunction`;
- the parser's `CONTAINS` arm never taken → four cases.

`cargo test -p slate-wasm`: every suite green, 226 tests. `sh scripts/check.sh`:
34 passed. `site/check/docs.py` and `quickstarts.py` green.

**Two of my own tests were wrong before they were right, both caught by
running them.** The first draft took its corpus from the *demo's* seed rather
than the workbench's — `The Player of Games` is not in this fixture — so three
cases asserted rows that do not exist. The second draft then wrote its `LIKE`
patterns in lowercase, and `LIKE` here is case-sensitive: `LIKE '%cosmic%'`
finds nothing whether or not a substring is a term, so the negative assertion
passed for the wrong reason. Both fixed by computing the expected sets from
the fixture rather than from memory, and the case-sensitivity trap is written
down in the test.

`the_schema_comes_from_the_catalog` asserted `books["indexed"] == [1]` and now
asserts `[1, 2]`. That is the new truth — the UI marks indexed columns and
`title` is one — not a loosened assertion.

## What this does not do

- **No search box in the demo's web UI.** The last surface, and the only part
  of F6b left.
- **The workbench takes a table scan for every `CONTAINS`**, because there is
  no hint syntax. Stated in the fixture beside the index.
- **No `NOT CONTAINS`.** `NOT (title CONTAINS 'x')` is not expressible either;
  the grammar's negation does not reach a comparison. Same for every other
  operator here, so this adds no new gap.
- **The SQL operator list has the weakness the wire's had.** `filter_strategy`
  in `tests/sql.rs` names its operators by hand, and nothing holds that list to
  the parser. The renderer panics on an operator it has no spelling for, which
  pins the renderer to the strategy and not the strategy to `comparison_tail` —
  so an operator added to the parser and to neither is invisible. Unlike
  `Expr`, there is no enum to pin it to; closing it properly means giving the
  parser a vocabulary constant, which is a change to production code for a
  test's benefit and was not made here.
