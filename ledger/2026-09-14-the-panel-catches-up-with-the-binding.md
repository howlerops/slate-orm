# The panel catches up with what the binding can do

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `site/{index.html,playground.js,style.css}`, `site/check/playground.py`
- **Kind:** feature

## What changed

Three commits widened the binding — writes, conjunctions, joins and grouped
reads — and left the panel offering one filter and no writes. It offers them
now:

- a second condition, ANDed with the first;
- a write row with one field per column, `insert`, `delete by key` and `reset`;
- a collapsed join section that pairs `authors` with `books`, optionally groups
  by country or author, and shows the join plan with a badge per input.

## Why

A capability nobody can reach is not a capability. The binding's tests proved
the kernel answers correctly; they say nothing about whether a reader can ask.

The write row matters most. Inserting a book and watching it appear in a query
that is answered *from the index* is the difference between reading that this
is a record layer and seeing it be one — and it is the one thing the panel
could not do at all.

## Alternatives rejected

**A full query builder with arbitrary conjunctions.** Two conditions cover the
interesting case — one on the indexed column becoming a scan bound, one staying
residual — and an "add condition" button that grows a list is UI work that
teaches nothing the second condition has not already taught.

**Putting the join in the main panel.** It answers a different question with a
different plan shape and a different result table, and side by side the two
plans invite being read as one. A `<details>` keeps the page calm for a reader
who wants the simple thing.

**Letting the write row choose insert or update by checking existence.** A
button that sometimes inserts and sometimes replaces is a button whose effect
you cannot predict before pressing it. `insert` refuses a duplicate key, which
is a better lesson than silently overwriting.

**Showing the projected columns only, in the results table.** Rejected before
and still rejected: a projection changes what the plan *decodes*, not the shape
of the answer. The `decodes` badge is where it shows.

## Evidence

Twelve assertions in `site/check/playground.py`, in headless Chromium — three
new: an inserted row appears in the current query (a count that goes up by
one), a second condition narrows Le Guin's books to the three after 1970, and
the join panel produces a plan containing `Join` with at least one group.

Also still green: `site/check/quickstarts.py`, the hook's 14 cases, the
workspace-layout guard, and the binding's 23 Rust tests.

**On the deployed site**, driven with the published bytes: filter on
`author_id = 2`, project onto that column so the plan is `Index Only Scan using
by_author`, insert a book for author 2, and the same index-only query goes from
four rows to five. The index answered for a row that did not exist a moment
before, without reading it.

The first attempt at that check reported `insertedRowAppears: false` and the
panel was fine — the throwaway script had left the filter on `id` rather than
`author_id`, so a row with id 9300 correctly did not match `id = 2`. Worth
recording because it is the third time today a check has been wrong rather than
the code, and the tell each time was a failure that made no sense.

## What this does not do

The delete button takes the primary key from the first write field, because
both fixture tables have a single-column key. A composite key would need the
first *n* fields and nothing here knows *n* from the schema, though the schema
carries it.

No update button, though the binding has `update`. Insert and delete are enough
to show index maintenance in both directions; update's distinct lesson — the
entry *moves* rather than duplicating — is covered in the Rust tests and would
need a fourth button to reach.

The join's country filter is a free-text box against `authors.country`, so a
typo returns nothing and looks like a broken panel rather than an empty result.
A select built from the fixture's distinct countries would be better and needs
a `distinct` the binding does not expose.

The join section's row table has no headers, because grouped and ungrouped
results have different column counts and the code that would name them for both
is more than the section earns.
