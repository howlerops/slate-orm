# The demo shows a soft delete being taken back

- **Date:** 2026-09-20
- **Author:** Claude, closing the last surface the restore work left dark
- **Touches:** `examples/explorer/web/src/{api.ts,panels.tsx,index.tsx}`, `examples/explorer/web/e2e/explorer.mjs`
- **Kind:** feature

## What changed

A **soft delete** tab in the explorer: retire a shipment, watch an ordinary read
stop returning it, undo the delete, watch it come back. Four badges and a line
saying which columns survived. One e2e check, which fails if the restore
silently does not happen.

No adapter change. The panel calls `/api/restore`, the endpoint the conformance
runner already compares across the three SDKs.

## Why

`ledger/2026-09-20-three-sdks-agree-about-a-restore.md` closed with *"The demo
UI still has no restore. A visitor cannot see this happen; only the conformance
runner can."* Before that, I had argued the demo showing none of the retention
surface made it out of scope. Both things are true and the second is not a
reason: the demo exists so somebody can see what this database does, and what
it now does — that it did not this morning — is let you take a delete back.

**Reusing `/api/restore` rather than adding a UI endpoint is the design
decision here.** The panel and the conformance case then exercise one handler,
so they cannot drift: a change that breaks the panel breaks 101 cases across
three SDKs first, and a handler written to satisfy the UI cannot quietly stop
being the thing three clients agree about. The endpoint's answer was shaped for
a diff rather than a screen and it turned out to be exactly what a screen wants,
because the reason it reports three points is the reason a visitor needs three:
"it is live now" is also what a handler that inserted a fresh row at the key
would say.

That is why the panel ends on a line about `status` and `book_id` rather than a
tick. The badges cannot distinguish a restore from a replacement — a replacement
reports the same id and the same "not stamped" — and the columns carried through
are the only thing that can.

## Alternatives rejected

**A restore *button beside each row* in the rows panel.** The obvious product
shape and the wrong teaching shape: it would show the effect and hide the rule.
The rule — a retired row is invisible to an ordinary read, and a write that
names its key reaches it only with `read_deleted` — is what a visitor needs, and
a button that just works says none of it.

**Show the refusal too** — the row written back with its stamp, refused naming
the column. It is the mistake everybody makes first, so it is tempting. Left to
`/api/restore-unchanged` and the conformance runner: a panel whose main control
is a thing that fails teaches "this is fragile", and the refusal's value is that
three clients report it identically, which a screen cannot show.

**A new UI-shaped endpoint returning the row.** More natural to render, and it
would be a second handler doing the same thing with nothing comparing them — the
drift the codegen work in this repository exists to prevent, reintroduced one
layer up.

**Add `purge` to the demo as well, so the tab is the whole retention story.**
Rejected for now: a purge is table-wide and destructive, the handler that runs
one has to put the corpus back afterwards, and a visitor clicking it twice
should not be racing another visitor's undo. The tab says what the window is
*for*; what closes it is in `examples/retention`.

## Evidence

**23 e2e checks pass in a real browser against the real stack**, the new one
among them:

```
  ok    a retired row goes invisible, and comes back the same row
23 passed, 0 failed
```

**And it fails when the restore does not happen.** The Go adapter's update
removed — not the panel broken, the *system* broken — the whole way through a
browser:

```
  FAIL  a retired row goes invisible, and comes back the same row
22 passed, 1 failed
```

That is the mutation worth running here: a panel test that only proves the panel
rendered would have passed, because the badges still render and three of the
four still read correctly. The one that changes is "after the undo it sees",
which is the claim the tab exists to make.

**Other suites**: the frontend's own 12 tests, `npx tsc --noEmit`, and
`scripts/check.sh` 20/20.

## What this does not do

**No `MUST_DIFFER`-style pair in the e2e.** The check asserts four badges from
one run, so a panel that ignored `hidden_while_retired` and hard-coded "0" would
pass. Asserting that the same panel says something *different* before the undo
would need the panel to render an intermediate state, which it does not — the
handler does the whole cycle in one request. The protection is that the numbers
come from the adapter and the adapter is compared across three SDKs.

**The panel is only exercised against whichever SDK is selected**, which the e2e
leaves at the default. The rows panel has a per-SDK loop; this does not, because
the three-way comparison already happens in the conformance runner and repeating
it in a browser would be slower and say less.

**The `identity` switch does nothing useful on this tab.** `reader` and
`stranger` will fail it, correctly and unhelpfully — the panel does not explain
that restoring needs `read_deleted`, beyond a sentence in the prose. A version
that demonstrated the grant boundary by switching identity would be better and
is a different panel.

**No screenshot or visual regression.** The check reads text out of the badges;
a layout that renders them unreadably would pass.
