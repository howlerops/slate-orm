# The demo shows the last three features

- **Date:** 2026-09-18
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `examples/explorer/web/src/{api.ts,panels.tsx,index.tsx,styles.css}`, `examples/explorer/web/e2e/explorer.mjs`, `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

Three panels — **Predicate writes**, **Batches**, **Relationships** — and four
e2e checks. 22 pass. No server change: every endpoint these call already
existed and was already compared by the conformance corpus.

## Why

Predicate writes, batch and relationships past one level shipped, were tested
from three clients, and were invisible on the page whose entire job is showing
the surface. The adapters served `/api/predicate-write`, `/api/batch` and
`/api/path`; the corpus compared all three; a visitor could see none of them.

## What each panel shows, and why that rather than the feature

**A count proves nothing a delete-by-key does not.** So the predicate-write
panel makes `returning` a control rather than a detail. Turn it off and the
answer is "affected 2" and a note saying that is all that is left of those
rows. Turn it on and the rows are there. The argument for `returning` is made
by its absence, which is the only way to make it.

**A batch that never fails is a round-trip saving and nothing more.** So the
batch panel always runs *both* atomicities, on three writes of which the second
collides, and puts them side by side. `independent` succeeds, reports the
collision as one of its outcomes, and leaves three rows. `all-or-nothing` fails
the call, has no per-operation outcomes — which the panel says is not missing
information — and leaves one. A control that ran one at a time would leave the
visitor holding the other's answer in their head.

**Two readings of one request.** The relationships panel walks
`sales → books → editions` and toggles between the tree and the far rows. Book
12 has no editions, so the tree shows a level that is *present and empty* where
`through` shows nothing at all — which is exactly the distinction that made the
Python client hand back a shelf where a copy was asked for, recorded in N1.

## Alternatives rejected

**A count-only predicate-write panel.** What the feature technically is, and it
would teach nothing: the item says so and it is right.

**One atomicity at a time, with a switch.** Half the UI and none of the point.
The two numbers have to be visible together or the comparison happens in the
visitor's memory.

**A fourth panel for keyset paging over a join**, which N3 shipped this same
morning. Out of scope: the item names three features and paging is not one of
them. Recorded here rather than added quietly.

**Editable keys on the relationships panel.** Rejected because the three keys
are chosen — 10 has two editions, 11 has one, 12 has none — and a uniform set a
visitor typed would let a wrong regrouping look right.

## Evidence

22 e2e checks pass, four of them new. 83 conformance cases still agree. The
frontend's own unit tests pass.

**Two real bugs in the panels, both found by the e2e rather than by reading.**

*The two atomicities raced.* They were two concurrent queries against one
database, and the handler clears and re-seeds the same four rows — so whichever
arrived second saw the other's half-finished state and came back a refusal. The
panel rendered one column; the e2e timed out waiting for the other. They run in
sequence now, from one query, which is also the only arrangement in which the
number each reports means anything.

*A `<For>` over a freshly built tuple array.* Every item was a new reference on
each render, so the row was recreated rather than updated and one column showed
its heading and nothing else — intermittently, which is worse. Two columns are
written out rather than looped.

**The expected counts in the test were guessed, and wrong.** 2 and 0 against
the actual 3 and 1. The failure read as a broken panel for two rounds before I
asked the adapter. The counts are read off it now, and the test says so.

**A third fix that was not a bug but a bad selector.** `.split > div` and
`.badge` "first anywhere" both picked something other than the side they looked
like. Each side carries its own `data-test` name, and the e2e waits for a badge
in *each* rather than the first in either.

## What this does not do

**No new server surface**, which is what the item required and is worth
stating: the three panels are a client of endpoints that already had three
implementations and a corpus comparing them.

**The batch panel's numbers are the demo fixture's**, not a general claim. Three
rows left and one row left are what *these* three operations do to *these*
seeded rows; the panel says what it ran, and a reader who wants the general
shape gets it from the prose rather than the counts.

**Nothing here is measured.** The panels show what happens, not what it costs.
`examples/batchbench` is where the batch's cost is, and the demo does not link
to it.
