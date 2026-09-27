# Windows, arrays, full-text, views and soft delete were built, merged and tested. None of them appears in the README's status list, its body, or the page a visitor reads to learn what this does.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #300 (F7a)
- **Touches:** `README.md`, `site/docs/features.html`, `site/docs/limits.html`
- **Kind:** documenting what already shipped

## What changed

Five features got an entry in the README's **Status** checklist (66 → 71
items), a section in the README body, and a section in
`site/docs/features.html`. `site/docs/limits.html` gained a paragraph naming
where each of them stops.

| | README status | README body | features page | limits page |
|---|---|---|---|---|
| window functions | added | added | added | added |
| array columns | added | added | added | added |
| full-text / `CONTAINS` | added | added | added | added |
| views | added | added | added | added |
| soft delete | added | added | added | added |

## Why

They were absent from all four. `features.html` — the page under "The
database → What it does", which is where a visitor goes to find out what this
is — described the planner, joins, subqueries, computed columns, relationships,
paging, conditional writes, migrations, decimals and timestamps, and stopped.
A reader would have concluded there are no windows and no full-text search.

The roadmap page *does* mention all five, which is how this survived: it was
written when they were planned, so a grep for "window" or "view" finds hits and
the gap looks covered. The pages that describe what exists, rather than what is
intended, are the ones that had nothing.

`CLAUDE.md` calls stale documentation worse than none because it is read as
current. Documentation that omits a third of the query surface is the same
failure wearing a different face: the reader's conclusion is wrong either way,
and they have no way to tell.

## Alternatives rejected

**Add the status-list entries only.** Cheapest, and it would have left the
README describing every other feature in detail while five had a checkbox and
nothing else — an inconsistency a reader would read as "these are the
half-built ones". The body sections are what make the list honest.

**Write a page per feature on the site**, as `docs/arrays.md`,
`docs/views.md` and `docs/full-text.md` already are in the repository.
Rejected for now: the site's structure is one page per *area*, not per feature,
and five new sidebar entries would bury the five that were there first. The
design notes stay where they are and the features page summarises them.

**Leave the limits page alone**, on the grounds that the features page already
names each limit in context. Rejected because that page is titled *What it is
not* and is where someone checks before adopting; a limit that only appears
beside the feature it constrains is one you find after choosing.

## Evidence

`scripts/check.sh` at **51 passed, all of them**. `site/check/docs.py` (every
relative link resolves), `site/check/quickstarts.py` (the docs' code runs),
`check_cited_docs.py` (141 citations openable), `check_retired_claims.py` (3
retired claims, none restated), `check_cost_prose.py` (25 prose claims about
the cost constants, all current).

Every fact was read out of the source or the design notes before being
written down — the window function list from `crates/slate-sql/src/lower.rs`,
the array ordering and its refusals from `docs/arrays.md`, the inverted
index's cardinality from `docs/full-text.md`, the view composition from
`docs/views.md`.

**A claim I nearly made and did not.** The limits page says the SQL front end
stops at `UNION`, `EXISTS` and anything correlated. Tasks #158 and #179 are
titled as though they built those, so the page looked stale. It is not:
`sql.rs` refuses all three by name, and those tasks were about *deciding and
refusing well*. Checking took one grep; the correction would have been wrong
and would have gone onto a public page.

## What this does not do

**No prose is mutation-tested**, because there is nothing to mutate. The
guards check that links resolve, that citations open, that cost figures match
the constants and that retired claims stay retired — none of them can check
whether a sentence describing a feature is *true*. These five were written
from the source, which is a stronger claim than usual here, and still weaker
than a test.

**It does not audit the rest of the surface the same way.** Five were found by
grepping the site for the features this session knew about. Whatever shipped
before and was never written up is still missing, and nothing enumerates the
difference between "what the kernel does" and "what the docs say it does".
That check does not exist and would be a real piece of work: the honest
statement is that this closes five known gaps, not that the docs are now
complete.

**The client-facing docs were not touched.** `clients.html` and the three
SDK READMEs describe the client surface, and whether each of these five is
reachable from Python, Go and TypeScript is stated in the entries that built
them rather than in one place a client author would look.
