# Two demo panels, and a conformance case whose blocker had been removed weeks ago

- **Date:** 2026-09-29
- **Author:** Claude Code, task K4
- **Touches:** `examples/explorer/web/src/panels.tsx`,
  `examples/explorer/web/src/api.ts`, `examples/explorer/web/src/index.tsx`,
  `examples/explorer/web/e2e/explorer.mjs`,
  `examples/explorer/conformance/conformance.py`, `docs/caveat-status.json`
- **Kind:** feature

## What changed

Three caveats said the demo's UI showed nothing for features whose endpoints
existed, whose conformance cases passed and whose clients had the surface. Two
panels close all three:

- **Full-text search.** `title contains "…"`, answered by the text index or by
  a table scan, with the access path shown beside the rows.
- **Conditional writes.** `update … expected` and `delete … expected`, with the
  control being *what the other writer does* rather than whether to guard.

A fourth caveat said `contains` had no conformance case "because the demo's
catalog has no text index to point one at". **Both halves were false.**
`examples/explorer/head.toml` declares `by_title_text`, and the corpus already
carried eight search cases with `MUST_DIFFER` pairs on the access path. Written
before the index landed and never re-read — the same overtaking pattern
yesterday's census identified, at a distance of weeks rather than minutes.

A fifth named two corpus cases it wanted and did not have. One of them is now
there: `"solaris cyberiad"`, two real terms never in the same title, by index
and by scan.

## Why

A panel that only ever succeeds demonstrates nothing, and both of these were
one control away from being that. So the design of each is the control:

- **Search shows the plan, not just the rows.** A text index and a table scan
  return the same books for the same word — by construction, because `contains`
  with no index is the same predicate applied to every row. A panel showing
  results alone would look right against an adapter that ignored the path you
  chose. `access` is the only field that distinguishes them, which is also why
  the conformance corpus pairs the two requests under `MUST_DIFFER`.
- **Conditional writes are controlled by the other writer.** An unconditional
  update and a guarded one over an unchanged row do exactly the same thing. The
  stale case is the only one that shows anything, so the dropdown is "meanwhile
  somebody: does nothing / changes the price / deletes the row".

The delete's third case is the one worth the panel: against a row that is
*gone*, a plain delete reports `affected: 0` and a conditional one refuses.
"There was nothing to do" and "somebody got there first" are different answers
and only one form tells you which.

`"solaris cyberiad"` is the sharper conjunction test because it fails three
ways distinguishably. `Solaris` is book 17 and `The Cyberiad` is book 18, so the
correct answer is nothing: a client that dropped either term answers one row, a
client that turned the conjunction into a disjunction answers two. `"the games"`
— the case that was there — matches one row, which is also what dropping the
second term gives when the first is rare.

## Alternatives rejected

**A checkbox on the rows panel rather than a search tab.** `contains` is not a
comparison operator with a different symbol; it is a different access path with
a different plan, and the plan is the thing worth showing. Folding it into the
operator dropdown would have put the interesting half — index against scan —
nowhere.

**One panel for the conditional update and another for the delete.** They are
the same mechanism and the difference between them is the third outcome, which
is only legible side by side. Two panels would have made "the delete has a case
the update does not" a comparison a reader has to make by clicking away.

**Asserting the rows in the e2e search checks.** Both paths return the same
rows, so an assertion on them passes against the bug the panel exists to catch.
The checks assert that the two paths return the *same* number of rows and
*different* plans, which is the pair of claims that is falsifiable.

**Adding the conformance case for a multi-term search and leaving the
empty-answer one.** The caveat named both and `"solaris cyberiad"` is both at
once: the correct answer to a cross-row conjunction *is* nothing, so the case
covers the empty answer through the index without a second fixture. Adding a
separate no-match case would have been a second spelling of the same request.

**Adding an `includeDeleted` control while the file was open.** It is a fifth
caveat on a sixth surface and the endpoint takes the flag already
(`/api/query`), but the interesting half is the *refusal* for a persona without
`read_deleted`, which needs the panel to distinguish "no retired rows" from "you
may not see them". That is a panel's worth of design and it is not this one.

## Evidence

- `examples/explorer/run.sh --e2e`: **28 passed, 0 failed**, four of them new
  and none of them a render check. The two paths return the same rows and
  report different plans; a search for `zzzznotitle` matches 0 and still
  carries a plan, which is how an empty answer differs from a refusal; the
  conditional update applies at 12.50 unguarded and is refused with **11.00**
  stored once the other writer moves it; the conditional delete against a gone
  row is refused with `affected 0`.
- `examples/explorer/run.sh --conformance`: **133 cases, the three SDKs agree on
  all of them**, up from 131. The two new ones are the cross-row conjunction by
  index and by scan, paired under `MUST_DIFFER`.
- `cd examples/explorer/web && npm test`: 16 of 16, including the guard that
  every view the UI offers reads a table the catalog declares.
- `npx tsc --noEmit` clean for the web app.
- **The repository's own guard caught this before I ran the suite.**
  `scripts/check_demo_surface.py` holds `NOT_IN_THE_UI`, one line per adapter
  endpoint the UI deliberately does not call, and it refuses when one starts
  being called: *"NOT_IN_THE_UI says the UI does not call `/api/search`, and it
  does. Delete the entry — the decision it records has been made the other
  way."* Three entries deleted, including one whose reason was "deliberately no
  search box … which is how a demo becomes a menu". That was a real argument
  and it is now overruled by three caveats asking for the opposite; deleting
  the line is how the roster records that.
- Its own test then failed on `1 of 13 adapter endpoints`, which is the
  roster's length plus one and had been typed out. Derived from
  `len(NOT_IN_THE_UI)` now: it is a consequence rather than a decision, and a
  red suite reporting arithmetic is the kind of failure that teaches people to
  edit numbers until it passes.
- `python3 scripts/caveats.py`: 110 open, down from 115.

## The ceiling from the previous entry found a defect in the SQL front end

Not part of this task, and it arrived in the middle of it: CI run 452 failed
`a_subquery_drops_its_null_candidates` with

> an IN list carries **95337** values, more than the 10000 this node will check
> per row

`SELECT count(*) FROM trips WHERE passengers IN (SELECT passengers FROM trips)`.
The subquery's rows were rendered into the `IN` list **one value per row**, not
per distinct value. Over the taxi corpus that is 95,337 values where 10 would
do — measured, not estimated, by
`the_passengers_subquery_has_far_fewer_values_than_rows`, which is a test
rather than a comment because a number in a comment is the staleness this
repository keeps finding.

`resolve_subqueries` deduplicates now. `x IN (a, a, a)` and `x IN (a)` are the
same predicate, so nothing about any answer changes; what changes is that the
predicate stopped costing ninety-five thousand comparisons per scanned row.

**The query worked before and was wrong about its cost.** That is the useful
shape of this: the ceiling did not break a working feature, it made a hidden
cost fail loudly. Adding a limit and finding that something was already over it
is the limit doing its job on its first day.

## What this does not do

**No `includeDeleted` control.** `/api/query` takes the flag and no panel sets
it, so the demo still shows nothing for the one soft-delete surface that is
privileged. It stays open, with the reason above for why it is a panel rather
than a checkbox.

**The search panel refetches on every keystroke.** It is a read against a
six-row table on loopback, so the cost is invisible here and would not be
against a real corpus. No debounce, deliberately: adding one would make the
panel's behaviour depend on a timer, which is the thing e2e checks flake on.

**Nothing asserts the panels' prose.** The e2e checks the numbers the panels
report. That the explanation beside them is *true* is checked by nothing, which
is the standing residual for every panel in this file and not new here.

**The subquery fix is a dedup, not a join.** `IN (SELECT …)` still
materialises: a subquery whose distinct values genuinely exceed the ceiling is
still refused, and the refusal's advice — put them in a table and join to it —
is advice the front end could take itself and does not. Lowering a subquery to
a join is the real fix and is not this.

**The demo still groups a two-table join and nothing else.** Unchanged by this
work and recorded elsewhere as deliberate: widening the demo to exercise every
shape of every RPC would make it a test rather than a demonstration.
