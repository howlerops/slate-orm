# The demo's UI has no search box, and that is the design rather than a gap

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, closing F6b
- **Touches:** `docs/full-text.md`, `docs/orm-comparison.md`
- **Kind:** docs

## What changed

Two sentences that listed "no search box in the demo's web UI" as the last
outstanding surface for full-text. They now say it is deliberate, and why.

## Why

F6b's remaining item was a search box. Before building one I counted what the
UI actually calls, and the count is the finding: **11 of the adapters' 24
endpoints.** The thirteen it does not call include `/api/window`,
`/api/chain`, `/api/nearest`, `/api/related`, `/api/page` and `/api/purge` —
every one of them a surface some earlier task added to the adapters and the
conformance corpus and stopped there.

So the demo already answers the question "should a new surface get a panel?"
and the answer has been no six times. The README's *What the UI shows* names
what it is for instead: the SDK switcher (same query, three clients, one
answer) and the identity switcher (`app`, `reader`, `stranger`, differing only
in what the database grants). Surface coverage is the conformance runner's
job, which is why that runner has 126 cases and the UI has seven panels.

Adding a search box and not a window panel would have made the UI less
coherent, not more — and would have set the precedent that the *most recent*
feature gets a panel, which is how a demo becomes a menu.

One thing this did confirm rather than change: the README already documents
that `reader` cannot `EXPLAIN`, for exactly the reason `/api/search` had to
return a `null` access path. The endpoint's behaviour matches what the demo
already told its readers.

## Alternatives rejected

**Build the search box.** It is perhaps thirty lines, and the objection is not
cost. It would be the seventh surface with a panel and the seventh of
nineteen, chosen because I happened to be working on it today rather than
because searching is more worth showing than a window function or a
nearest-neighbour query.

**Add panels for all six missing surfaces.** A real option and a real project,
and it is a redesign of the demo rather than the tail of a full-text task. If
anyone takes it on, the argument for it is that the UI's curated list has
drifted from the adapters' since it was written — which the 11-of-24 count is
the evidence for, and is now recorded.

**Leave the docs saying a search box is owed.** The worst option: it reads as
a to-do, so the next person either builds it without the count or re-derives
the count to decide not to. Stale documentation is worse than none, and a
promise nobody intends to keep is the stalest kind.

## Evidence

The endpoint lists, taken from the source rather than from memory:

- UI (`web/src/api.ts`): aggregate, batch, explain, explain-aggregate, join,
  meta, path, predicate-write, query, restore, transaction — 11.
- Adapters (`backends/go/main.go`): those plus bad-batch, bad-status, chain,
  conditional-delete, conditional-update, nearest, page, purge, related,
  restore-unchanged, search, typed, window — 24.

`python3 site/check/docs.py`: the docs site holds together.

## What this does not do

Nothing makes the UI's coverage visible. There is no check that the README's
*What the UI shows* still describes `web/src/`, and no check comparing the
UI's endpoint list to the adapters' — so the drift this entry measured
happened silently over six features and was found by counting on purpose. A
guard is writable (both lists are greppable, as above) and was not written,
because a guard that fails whenever the adapters gain an endpoint would fail
on every surface task and be switched off.
