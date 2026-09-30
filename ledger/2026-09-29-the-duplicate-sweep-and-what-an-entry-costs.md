# The duplicate sweep in the direction nobody had run it, and the arithmetic that says the open list grows

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

Four verdicts, no code:

- `2026-09-29-two-rows-for-one-gap.md` :: *"It does not sweep the other 124
  open rows for duplicates."* → **`closed`**, exempt as a reading. The sweep is
  run below, open-against-open, which is the direction it named as missing.
- `2026-09-20-the-attribute-the-builder-already-had.md` :: *"Nothing measures
  the cost."* → **`closed`**, witness `soft-delete-read-cost`. It is the third
  row for the gap
  `ledger/2026-09-29-what-a-retired-row-costs-a-read.md` measured this
  afternoon, and the sweep is what found it.
- `2026-09-29-the-deliberate-verdicts-priced.md` :: *"840 remain unread."* and
  `2026-09-29-the-deliberate-sample-carried-to-216.md` :: *"792 remain
  unread"* → **`moment`**. One live count, not five.

**133 open**, from 137.

## Why

The goal this session is working under is *nothing open*, and before grinding
at it further it is worth knowing whether the list can be emptied by the
method being used on it. It cannot, and the number is not close.

**46 entries dated today carry 73 `open` rows between them: 1.6 open caveats
per entry.** An entry that closes one caveat and honestly records what it did
not do is **net +0.6 on the backlog**. That is not a criticism of the
convention — `ledger/2026-09-29-two-rows-for-one-gap.md` already said "work
closed two and opened five … that is what an honest entry costs" — it is the
arithmetic that says which *kind* of work reduces the list. Only two do: a
change whose own caveats are all settled, and a re-reading that closes rows
without writing an entry per closure. This is the second.

## Alternatives rejected

**Build the duplicate sweep as a guard.** Measured and rejected, which is the
useful half of this entry. **24 key texts appear in more than one entry, across
65 rows** — and they are almost all short generic sentences ("No measurement.",
11 times; "Nothing measures it.", 4) that are different gaps in different
entries. Narrowing to the disagreeing cases, where the same sentence is `closed`
in one entry and `open` in another, gives **three clusters**, and reading all
three shows every one is a distinct gap sharing a generic phrase. A guard with
24 findings of which 0 are real is a guard nobody reads, which
`ledger/2026-09-26-the-half-that-asks-which-files.md` argues at length about
`self.table(`. The tracker's key is a 60-character prefix, so this is a property
of the key rather than something a better matcher fixes.

**Leave the two superseded "remain unread" counts `open`.** They are true
statements that were true when written, which is the definition of `moment`,
and the tracker already has four progress markers retired this way in
`ledger/2026-09-29-...` and task J1. Five rows for one number is the shape this
sweep exists to find.

**Merge the one real duplicate pair into a single row.** `A conditional update
is one round trip per row.` appears in
`2026-09-18-a-decimal-and-a-conditional-update-on-the-wire.md` and
`2026-09-18-decimals-and-conditional-updates-in-three-clients.md`, both `open`,
both with the same reason. The precedent in
`ledger/2026-09-29-two-rows-for-one-gap.md` moves a duplicate to *match* the
verdict of the row that reasoned it, and here they already match — so there is
nothing to move. Two entries each made the claim and both are true. The tracker
counts claims, not gaps, and this is the one place where those numbers differ.

**Close more aggressively to make the number fall.** The four here are the four
the sweep and the afternoon's measurement actually answered. Reclassifying a
live gap to make a count look better is the failure
`ledger/2026-09-29-three-verdicts-that-were-wrong-not-three-gaps.md` is about,
and it was committed three times before anybody checked.

## Evidence

**The sweep, open-against-open.** All 137 open rows, each pair, keys normalised
(lowercased, punctuation dropped, whitespace collapsed) and compared with
`difflib.SequenceMatcher` at a 0.72 threshold. Ten pairs above it:

| similarity | what it is |
| --- | --- |
| 1.00 | `A conditional update is one round trip per row.` — **a real duplicate**, two entries a day apart, neither settled |
| 0.88, 0.88, 0.82 | the three `N remain unread` progress markers, 840 / 792 / 747 |
| 0.82 | `Nothing measures the cost.` against `Nothing measures a sweep's cost.` — different gaps |
| 0.82, 0.81, 0.76 | three more `Nothing measures … cost` pairs — a restored freshness floor, a tower layer, a purge; all different |
| 0.77 | `No client surface.` against `No SQL surface.` — different |
| 0.75 | `It does not measure a round trip.` against `It does not measure a partial index` — different |

One real duplicate in 9,316 pairs. **The caveat predicted exactly this case** —
"two rows could duplicate each other with neither settled" — and it exists, once.

**The exact-key population, for the guard question.** 24 duplicated key texts
over 65 rows; 3 clusters where a `closed` row and an `open` or `narrowed` row
share a key. All three read as coincidence:

| key | the three entries |
| --- | --- |
| `It builds nothing` | a view guard (closed), a hash-input store (moment), a guard not built (open) |
| `Nothing measures the cost.` | a join page (deliberate), a view read path (closed), soft delete (open → closed today, on its own measurement, not on this) |
| `Nothing stops this recurring.` | two ruffs (closed), a gap list (deliberate), three wrong verdicts (open) |

**The per-entry arithmetic**, counted from the tracker rather than estimated:
46 entries dated 2026-09-29, 73 `open` rows among them, **1.6 per entry**. 64 of
the 137 open rows predate today.

No mutation: nothing executable changed. The one edit outside `ledger/` besides
the tracker is a witness row and an exemption in
`scripts/check_closed_caveats.py`, and that guard's rules are mutation-tested
already — `ledger/mutations/20260929T201851-github-workflows-ci-yml.json` is the
most recent against them. `python3 scripts/check_closed_caveats.py`: 467 closed,
431 witnessed, 36 exempt, all eight rules ok.

## What this does not do

**It does not make the open list reachable.** 133 rows, of which 64 predate
today and most name work that needs building: a scale on the wire, a typed row
in three clients, a backfill estimate, a partial index. The arithmetic above
says a session closing them one entry at a time adds 1.6 for every 1 it
removes, so the list falls only if the closures are batched the way these four
were, or if the work closes gaps whose own caveats are all settled.

**The sweep is a reading and leaves nothing behind.** Run again tomorrow it
would have to be re-typed. It is exempt as a reading for that reason, and the
measurement above is the argument that a guard in its place would be noise —
which is a reason not to build one, not a reason the gap is covered.

**It says nothing about the 110 `narrowed` rows or the 1018 `deliberate`
ones.** The sweep was over `open` only, because that is the set the caveat
named. A duplicate spanning `open` and `narrowed` would not have been found.

**The 1.6 figure is one day.** It is this session's entries, which are unusually
dense in guards and measurements — both shapes that name their own limits
freely. A day of feature work might be cheaper or dearer and nobody has counted
one.
