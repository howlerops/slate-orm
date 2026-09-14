# The playground was fifth on the page with nothing linking to it, so the first thing anyone said about it was to ask whether it had deployed

- **Date:** 2026-09-14
- **Author:** Claude (agent session), at the request of the repository owner
- **Touches:** `site/index.html`, `site/playground.js`, `site/check/playground.py`, `site/README.md`
- **Kind:** fix

## What changed

The `#playground` section moved from fifth on the page to second, directly
after the quickstart. The header nav and the hero gained a link to it. The
`where` picker now opens on an indexed column rather than the first column,
which for both fixture tables is the primary key. The prose above the panel
was rewritten to match the control state it now describes, and three
assertions were added to the browser check: the panel opens on an indexed
column, at least two links point at it, and it precedes `#what` in document
order.

## Why

The panel was deployed, served, and working — a headless Chromium run against
the live bytes returned rows and a plan — and the first report about it was
"I don't see the wasm playground? Did it not deploy yet?". That report was
correct about the thing that matters. Reaching the panel required scrolling
past four sections, and once there it rendered as a single grey button
labelled "Load the database"; nothing in the nav, the hero, or the first
screen said it existed.

The default query made the second half of it worse. The picker opened on the
first column, `id`, with the value box pre-filled with `1`, so the first plan
any reader saw was `Point Get on books (rows=1)`. That is the one access path
whose existence surprises nobody, and it made the panel read as a lookup form.
It also contradicted the paragraph directly above it, which told the reader to
filter on the indexed column — a step the reader had to perform before the
panel demonstrated anything. Opening on `author_id` means the first plan on
screen is a table scan over 4,824 rows that three unticked checkboxes turn
index-only, which is the whole argument the page is making.

Nothing here changes the kernel, the binding, or what the panel computes.

## Alternatives rejected

**Leave the position, add links only.** Cheaper, and the nav link alone would
have answered this particular report. Rejected because the hero's own claim —
a planner, indexes, EXPLAIN — has a running demonstration on the same page,
and burying it under three sections of prose about the same claims is the
wrong order. A reader who scrolls past "What it does" has already decided
whether to believe it.

**Put it first, above the quickstart.** Tempting, and rejected: the quickstart
is what a returning reader comes for, and a 600 KB panel is not what they want
between the URL and the TOML. Second place costs the quickstart nothing and is
above the fold's first scroll.

**Auto-load the wasm when the section is first painted rather than on
scroll-into-view.** Would remove the button entirely and make the panel appear
as a working thing. Rejected: it undoes the lazy-loading work recorded in
`2026-09-14-lazy-wasm.md`, which exists so a reader who never scrolls here
pays nothing. The IntersectionObserver already starts the fetch 200 px early,
so a reader who scrolls to it sees the panel, not the button.

**Default the filter to `author_id` but leave the value empty.** An empty
value means no condition, so the first plan would be an unfiltered table scan
over everything — honest, and less interesting than the filtered scan that
sits one checkbox away from being index-only.

**Assert the placement in prose only** (a README line saying "keep it
second"). Rejected for the reason the rest of this repository does not do
that: a sentence in a README is not a check. The three new assertions were
each mutation-tested below.

## Evidence

The panel was working before this change, which is why the report was about
placement. Driving the live deployment in headless Chromium at 1280×900:

```
{"headingVisible": true, "loadButtonVisible": true, "panelHiddenAtStart": true,
 "afterScroll_panelVisible": true, "rowsRendered": 1,
 "plan": "Point Get on books  (rows=1 cost=3.00 limit=20 decodes=[0,1,2,3])",
 "problems": []}
```

`rowsRendered: 1` and that plan are the default-query problem stated as a
measurement: one row, the least interesting access path, on first paint.

`site/check/playground.py` goes from 12 assertions to 15, all passing after
the change. Each new one was mutation-tested — break it, confirm the *named*
check fails, restore, re-verify:

| mutation | check that failed |
| --- | --- |
| `el("column").value = …` replaced with a no-op | `the panel opens with the filter on an indexed column` |
| the hero's `#playground` button deleted | `the page links to the playground, more than once` |
| the section moved back below `#what` | `and the playground comes before the prose sections` |

Each mutation failed exactly one check and left the other fourteen passing,
so none of the three is being carried by an existing assertion. Restored
tree: 15/15.

The ordering check uses `compareDocumentPosition`, not a line number, so
editing the file around either section does not move it.

## What this does not do

It does not measure whether anyone now finds the panel; the evidence is that
it is linked and above the fold's second screen, not that a reader reached it.
There is one data point for the original problem and it will not be repeated.

The ordering assertion only pins `#playground` before `#what`. Inserting a new
section between the quickstart and the playground would pass. Pinning an exact
index would fail every time a section is added, which is worse.

The default-column assertion checks the *option label* ends in `·idx`, which
is the marker `describeTable` writes. A change that stopped marking indexed
columns would fail this check for the wrong reason — it would still be a real
failure, but the message would point at the default rather than the marker.

The panel still opens on `books`, which is hard-coded in `boot()`. If a future
fixture table has no index at all, the picker falls back to the first column
and the new check fails; that is intended, but the fallback has no test
because the fixture has no such table.
