# The live refusal breaks two rules now, because reading a list is different code from reading one

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `examples/explorer/head.toml`, the three demo adapters
- **Kind:** fix

## What changed

A second `CHECK` on `shipments` — `id_is_seeded`, reserving ids of 9000 and
above for rows the demo writes in order to have them refused — and the purge
handler's three fixture ids moved from 9401-9403 to 8401-8403 so that they
satisfy it. `/api/bad-status` writes id 9499 with status `"teleported"`, so one
row now breaks both checks and the conformance case compares a list of two.

## Why

The gap the previous entry recorded: "The live case exercises one failing
check, not three. `shipments` declares only `status_known`, so
`/api/bad-status` reaches the single-failure shape."

That matters more than it sounds, because the two are not the same code. Each
client reads `violations`, gets a count, and loops; with one failure the loop
runs once and the count could be anything positive and still look right. The
*list* — indexed keys, declaration order, a count that has to match — was
reached only by each client's unit fixture, which is a recording. Now a live
server produces it.

## Alternatives rejected

**A cross-column check, matching the fixture's `discount_under_price`.** The
most valuable shape, because it is the one with no `column` and no `message`,
and the one where a client that assumed every failure has a column falls over.
`shipments` has four columns and no pair with a rule worth writing between
them; the candidates I tried — a delivered row cannot be retired, a retired row
must have shipped — are either satisfied by the bad row or violated by the
seeded ones. Inventing a rule the schema does not want in order to break it
would be a demo constraint that exists to be a test, which the V3 entry already
argued against. The cross-column shape stays on the fixture, and this entry
says so rather than implying the live case now covers everything.

**A check on `book_id`.** The natural second column, and it collides with the
foreign key: any value that breaks a plausible bound also breaks
`shipment_book`, and which refusal arrives first is a question about kernel
ordering rather than about the client. Not worth finding out here.

**Leave the purge ids at 9401-9403 and set the bound higher.** `id < 9450` and
9499 still breaks it. It is also a number with no meaning, and the next person
to add a handler picks an id in the gap and gets a puzzle. `9000` is a round
boundary with a sentence attached, and moving three fixture ids cost three
lines.

**Two checks that the *same column* breaks.** `status in (…)` and
`length(status) < 20`, say. One bad value, two failures, no id range and no
moved fixtures. It also makes both failures name `status`, so a client that
mapped failures onto fields by overwriting rather than appending would look
correct. Two different columns is what separates those.

## Evidence

98 conformance cases, the three SDKs agreeing on all of them. The refusal's
`violations` now carries two entries where it carried one, observed rather than
assumed by printing the agreed answer from the runner:

    {"error": {"kind": "invalid-request",
               "message": "row violates 2 checks on table `shipments`: …",
               "reason": "CHECK_VIOLATION",
               "violations": [
                 {"check": "status_known", "column": "status",
                  "message": "Status must be pending, shipped or delivered."},
                 {"check": "id_is_seeded", "column": "id",
                  "message": "Shipment ids above 9000 are reserved …"}]}}

Two failures, two *different* columns, and `status_known` first — which is the
order `head.toml` declares them in and not the order the row breaks them in.
Worth having looked at: the first draft of this entry had the pair the other
way round, from reasoning about which value is "more wrong" rather than from
reading the output.

The demo's own suites: 15 in the node adapter, 11 in the Go schema package, 12
in the browser app (which reads `head.toml` and would have failed on a check it
did not know about, and did not, because checks are not in the list it
compares). `sh scripts/check.sh` is 19 passed, which includes the codegen
`--check` that regenerates all three declarations and diffs them.

No mutation this time, and the reason is worth stating rather than omitting:
the change *is* a fixture, and the thing it exercises — the counting loop in
three decoders — already has mutation coverage from its own suites, where
dropping the count, returning a prefix, and walking the map were all caught.
What this adds is that a real server produces the shape those tests recorded.

## What this does not do

**The cross-column shape is still fixture-only.** Said above and repeated here
because it is the one failure mode a live case has never produced: a check with
no column and no message, which every client must render beside the form rather
than beside a field.

**Three failures is still fixture-only too.** Two is not three, and the
difference between them is nothing — the loop that handles two handles twenty.
Said for completeness, not as a gap worth closing.

**`id_is_seeded` is a rule that exists for the demo's convenience.** It is a
real constraint with a real message and the server really enforces it, but no
production schema would reserve an id range for its test fixtures. A reader
taking `head.toml` as an example of schema design should take this one as an
example of a *check*, not of a good rule.
