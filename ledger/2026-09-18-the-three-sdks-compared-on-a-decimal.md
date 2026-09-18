# The conformance corpus can now catch three clients disagreeing about money

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working `docs/orm-comparison.md`'s W list
- **Touches:** `examples/explorer/` (`head.toml`, `CONTRACT.md`, all three
  adapters, `conformance/`, `web/`), `README.md`, `docs/orm-comparison.md`,
  `site/docs/{clients,features,roadmap}.html`
- **Kind:** feature

## What changed

`books` gains a `price` decimal column at scale 2, so every read case in the
conformance corpus now carries a decimal and the three adapters' encoders are
compared on it. A `POST /api/conditional-update` endpoint drives an optimistic
update that lands and one that is refused, and reports the price both as its
units and as a *rendered* string — which is the only place the three clients'
decimal renderers meet. Two corpus cases for the pair, one for a filter over a
decimal, and the documentation that had recorded both features as absent now
records them as present.

## Why

The previous two commits gave each client a decimal and a conditional update,
and each client's own suite tests them. That is exactly the hole the
conformance runner exists to fill: three suites against one server catch a
client that is wrong and cannot catch three that are wrong the same way — and
a decimal is the value type where "the same way" is most likely, because all
three clients are implementing the same paragraph of the same document about
where a decimal point goes.

## Alternatives rejected

**Putting the rendered string in the value tag** — `{"decimal": "1250",
"rendered": "12.50"}`. It would compare the renderers everywhere rather than at
one endpoint. Rejected because the tag is the *wire's* account of a value and
the wire has no scale; a tag that carried one would be the adapters inventing a
protocol field, and the first reader to take the corpus as a description of the
protocol would be misled. The rendering is an endpoint's answer, where it is
visibly the adapter's own work.

**A `/api/decimal` endpoint of its own**, rather than folding the rendering
into the conditional-update one. Two endpoints, two seeds, two id ranges, for
one extra string. The conditional update already needs a decimal column to be
worth testing — an optimistic update guards a running total, which is what a
decimal column holds — so they belong together.

**Two endpoints for the conditional update**, one that lands and one that is
refused. The `stale` flag is a flag because the *pair* is the case: an
unconditional update and a conditional one over an unchanged row do exactly the
same thing, so an adapter that built the expected rows and never sent them
passes the first and fails only the second. Splitting them would make it easy
to add the first and forget the second, which is precisely the mistake that
already happened twice in the clients' own suites.

**A new table rather than a column on `books`.** `books` already carries two
columns appended for exactly this reason — `released` for a calendar
expression, `embedding` for a distance — and both say so where they are
declared. A column joins every existing read case for free; a table needs a
case per read shape.

**Reporting a refused conditional update as an adapter error.** Then one of the
two cases is a refusal case and the other is not, and the corpus compares
different shapes. `refused` is a field, the same choice `failed` already makes
in the batch endpoint and for the same reason.

## Evidence

**86 conformance cases, all three SDKs agreeing on every one.** Before that,
two disagreed: a transaction probe in each adapter built a seven-column `books`
row and the eighth column made it a refusal, which Go and Node reported as the
server's `invalid-request` and Python as its own client-side `adapter` error.
That is the runner doing its job on the first run — three adapters, one of them
refusing in a different place.

**Three adapter mutations, all killed:**

| mutation | caught by |
| --- | --- |
| the Go adapter sends no expected rows | `a conditional update over a row that moved` |
| the Python adapter tags a decimal as an `i64` | `all types`, `a filter on a decimal column` |
| the Node adapter renders against scale 0 | both conditional-update cases |

The second is the one worth recording. `Units` is an `int` subclass in Python,
so without an explicit branch before the plain-`int` one a decimal tags as
`{"i64": …}` — a *wrong* tag rather than the `{"unknown": …}` a missing type
gets, and wrong is worse than unknown because unknown is visible. The branch
carries that reasoning; the mutation confirms two cases fire on it.

Also green: the browser e2e, 22 checks (the demo's column list grew by one);
`site/check/docs.py`; `site/check/quickstarts.py`, which runs all three
quickstarts against their own nodes; `scripts/check_workspace.py`; the
pre-commit hook's own 14 tests; `cargo fmt --all --check`.

## What this does not do

- **The corpus compares renderers at one scale.** Every adapter renders against
  2, because that is what `books.price` declares. A disagreement that only
  showed at scale 0 or at a negative value would not appear here — each
  client's own suite has the table of edge cases, and those three tables are
  written independently rather than shared.
- **The demo's UI does not show the conditional update.** The endpoint exists
  and the corpus drives it; no panel does. The e2e's 22 checks are unchanged in
  number for that reason.
- **Nothing here checks a client's declared scale against the server's**, which
  is the hole the previous commit recorded. All three adapters declare 2 and
  all three are right; a corpus of agreeing clients cannot catch a fact none of
  them can observe.
- **`price` is not in any aggregate case.** `SUM` over a decimal is exact and
  that is the claim worth comparing across three clients; it is not compared
  here, and it is the obvious next case.
