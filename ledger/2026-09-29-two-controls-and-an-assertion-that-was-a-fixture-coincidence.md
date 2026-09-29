# Two controls, and an assertion that turned out to be a fixture coincidence

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `examples/explorer/web/src/{panels.tsx,api.ts}`,
  `examples/explorer/web/e2e/explorer.mjs`, `scripts/check_closed_caveats.py`
- **Kind:** feature

## What changed

Two controls in the demo, for two things the adapters already served and the
browser never showed.

**`include_deleted`**, in the soft-delete panel: a select that re-reads
`shipments` with the flag. The point is not the row count — nothing in the
fixture stays retired — it is that switching the identity to `reader` turns the
same request into `permission-denied` rather than an empty answer. That is the
distinction a soft delete has to make and an ordinary filter cannot.

**A `measure` control** in the grouped panel: count, or `SUM(books.price)`
drawn and sorted as money, rendered against the column's scale. The bar is
drawn in *units* and only the label is rendered, because dividing first would
put a float on the axis for a value the database keeps exact.

Two e2e checks, 28 to 30. And three things came out of writing them, all of
which are the entry.

## Why

Three caveats, one sentence apart:

> **The demo UI has no control for it** — the identity switcher shows the
> policy but not the flag.
> — `ledger/2026-09-19-asking-for-the-rows-that-are-gone.md`

> **No `includeDeleted` control.** `/api/query` takes the flag and no panel
> sets it. The interesting half is the refusal for a persona without
> `read_deleted` — distinguishing "no retired rows" from "you may not see
> them".
> — `ledger/2026-09-29-the-panels-the-endpoints-were-already-serving.md`

> **The demo's UI does not show it.** `GroupRow.total` is in the frontend's
> types and the chart still draws the count.
> — `ledger/2026-09-29-the-one-aggregate-that-returns-money.md`

The third was written four hours before this and answered by it, which is the
same shape this session has now hit four times: a caveat recorded and closed by
the next task nobody connected to it.

## Alternatives rejected

**Seed a permanently retired shipment, so the flag changes the count.** It
would make the `app` half of the control visible instead of a no-op. Rejected
because the count is checked by the demo's README guard, by conformance cases
that read `shipments`, and by the restore panel's own arithmetic — four things
to move so that one number differs. The refusal is the half the caveat called
interesting, and it needs no retired row at all.

**Draw the money bars in major units.** `4144` becomes `41.44` and the axis
reads like money. It puts a float on the chart for a value the kernel keeps as
an exact count of cents, which is the one thing a decimal column exists to
avoid — so the bar is the units and the label is the rendering.

**Import the TypeScript client's `unitsToString` into the browser.** The page
already has a renderer's worth of logic; a fourth implementation is a fourth
thing to get wrong. Rejected because the browser is not a slate client: the
import pulls a gRPC stack into a page that speaks HTTP. It is nine lines, and
`/api/render-decimals` — added an hour earlier — holds the three that matter to
each other.

## Evidence

- `examples/explorer/run.sh --e2e`: **30 passed, 0 failed**, up from 28.
- **Two mutations, both caught**
  (`ledger/mutations/20260929T030111-examples-explorer-web-src-panels-tsx.json`):
  the retired-rows control never setting the flag → *the retired-rows flag is a
  privilege, not a filter*; the measure control not changing the sort → *the
  chart can be drawn in money instead of rows*.
- Getting to those two took **four rounds**, and the first three are the
  finding:
  1. Both mutations survived. The refusal assertion read the whole panel's
     text, and the retire-and-undo above it is refused for a reader too — so
     `permission-denied` was there either way. Scoped to the read's own answer,
     plus a control asserting the *unflagged* read is served.
  2. The sort assertion compared the leading group of the two orders and
     failed **on an unmutated tree**. Three causes, in order: the panel's own
     controls survive a tab switch, so the grouping was whatever the previous
     check left; switching the measure back re-renders the labels from a signal
     while the rows are still the previous answer, with no spinner to wait on;
     and — the real one — the write panels above leave extra books on one
     author, so by the time this runs the count order and the money order agree
     at the head and differ further down. The leader was a coincidence of the
     seed, read off a running stack once and treated as a fact.
  3. It checks the property now: the bars descend in money. That holds whatever
     the writes above have done to the data, and a sort that followed the count
     breaks it.
- **And `mutate.py` could not score the e2e at all.** It printed
  `  FAIL  <what>` indented by two; the `python` dialect reads `^FAIL\s+`. The
  summary line matched, so a run scored as "1 suite reported, no failure
  named" — `UNREADABLE`, on mutations that were in fact caught. Column zero
  now, the house format every `scripts/test_*.py` prints. The earlier records
  (`ledger/mutations/20260929T025930-examples-explorer-web-src-panels-tsx.json`,
  outcome `problems` with both cases `unreadable`, and
  `ledger/mutations/20260929T025458-examples-explorer-web-src-panels-tsx.json`,
  whose *baseline* was unreadable for the same reason) are what that looked
  like.
- A page error is a **named failure** now too. It used to print `30 passed, 0
  failed` and exit 1, which is a suite that passed to anything reading the
  output and one that did not to the shell — `mutate.py`'s second lie, and the
  same fix `test_conformance.py` took an hour earlier for the same reason.
- `python3 scripts/check_demo_surface.py`: 15 of 26 endpoints in the UI.
- `examples/explorer/web && npm test`: 16 passed, 0 failed.
- `python3 scripts/caveats.py`: 1647 caveats, 113 open, 72 narrowed, 409
  closed, 920 deliberate, 0 untriaged.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**The `app` half of the retired-rows control changes nothing visible.** The
fixture keeps no retired row, so the count is the same with the flag and
without it, and only the `reader` refusal distinguishes them. A visitor who
never touches the identity switcher sees a control that appears to do nothing —
which is honest about this data and is why the panel's prose says so.

**The money measure is one column at one scale.** `books.price` is the demo's
only decimal, so the control shows a renderer against a hard-coded 2. The UI
holds that 2 the way the three adapters hold theirs, and nothing checks it
against the catalog — the generated `catalog.ts` carries column *names* only.

**The e2e's mutation-readability is fixed, not tested.** Nothing fails if
somebody re-indents those lines; the next mutation run would simply go back to
saying `UNREADABLE`, which at least interrupts rather than passing. A guard
holding every runner in this repository to the one output format is the version
that scales, and it is unbuilt — `scripts/test_check_sh.py` is the precedent
for where it would live.

**The order property does not pin the numbers.** "Descending in money" is true
of the right answer and of several wrong ones — a server that returned every
total as zero would satisfy it. The values themselves are compared across the
three SDKs by the conformance runner, which is where a number belongs; this
checks that the control reaches the sort.

**Nothing shows both measures at once.** A second series would answer "which
author is expensive per book", which is the question a reader actually has
after seeing these two charts, and it is a chart change that has to argue for
itself rather than a control that already had an endpoint behind it.
