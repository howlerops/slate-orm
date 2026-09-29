# Two rows for one gap, and the day's own caveats given verdicts

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

Two things to one file.

**Fifteen caveats were untriaged** — the ones this session's three earlier
entries raised in their own "What this does not do". Each now has a verdict:
five `deliberate` where the entry argues against doing the thing, five `open`
where it is ordinary undone work, three `moment`, one `narrowed` with a
residual, and one more `deliberate`. Zero untriaged is the invariant
`scripts/check.sh` enforces and it was red.

**Two `open` rows turned out to be the same gap as a `deliberate` one**, and
both say so in their own text:

- `2026-09-16-a-durability-check-that-cried-data-loss.md` :: *"The batch cap is
  still enforced only server-side … noted in the previous entry and still
  true."* The previous entry is
  `2026-09-16-batch-in-three-clients-and-a-token-that-only-survives-batched.md`,
  whose row is `deliberate` with the argument: the limit is configurable, so a
  client-side copy is a second number that has to agree with a server the
  client cannot read.
- `2026-09-20-the-sweep-nobody-scheduled.md` :: *"No offline tool."* —
  word for word the caveat in `2026-09-19-a-purge-something-can-call.md`, whose
  row is `deliberate` because an offline purge needs a binary that opens the
  store outside the writer's lease.

Both move to `deliberate`, crediting the entry that reasoned it.

## Why

An open list is read only while every row on it is work somebody could pick up.
A row that duplicates a decided one is not work — it is the same decision
recorded twice with one copy mislabelled, and a reader who picks it up
rediscovers an argument that is already written down two files away.

The triage is the other half of the same thing from the other end. A caveat
with no verdict is invisible to every count, so a session that writes three
honest entries and stops has quietly enlarged the backlog without the number
moving. The invariant exists because zero *open* is unreachable by
construction — the hook requires a "What this does not do" section and every
bullet in it becomes a caveat — while zero *untriaged* is reachable and means
"somebody has looked at all of it".

**And the arithmetic is worth stating plainly rather than buried.** The count
went 126 → 124 on the two duplicates and then 124 → 129 on the fifteen. Today's
work closed two and opened five. That is what an honest entry costs, and a
session that wanted the number to fall would have written thinner ones.

## Alternatives rejected

**File the fifteen as `open` and move on.** Fastest, and wrong for ten of them:
a caveat whose entry argues in its own Alternatives that the thing should not be
done is `deliberate` by definition, and putting it on the open list invites
somebody to do the rejected thing. The verdict is supposed to carry the
reading, not the reader's haste.

**Leave the two duplicates open on the grounds that a gap is a gap.** Defensible
— nothing about the batch cap or the offline purge is *fixed*. Rejected because
the verdict records the decision rather than the state, and the decision was
taken and written. If either is reopened, it is reopened in one place.

**Fold the duplicate rows together — one row, one entry.** The tracker is keyed
on `entry::claim`, so a caveat exists wherever it was written; there is no
mechanism for one row spanning two entries, and inventing one to save two rows
would make every existing key ambiguous. Cross-referencing in the `by` is what
the schema supports.

**Sweep for every duplicate mechanically.** A near-duplicate search over the
open rows was run and is mostly noise: "Nothing measures the cost." appears
against two unrelated subjects, "It builds nothing" against three. Text
similarity finds phrasing, not subject, and the two acted on here were
confirmed by reading both entries. A third candidate was rejected on exactly
that basis — see below.

## Evidence

`python3 scripts/caveats.py`: **1834 caveats, 129 open, 103 narrowed, 458
closed, 1002 deliberate, 0 untriaged**, from 15 untriaged before.

`sh scripts/check.sh`: exit 0, **87 passed, all of them**.

The near-duplicate sweep proposed eight pairs over the open rows at a
similarity above 0.86. Six were phrasing coincidences across unrelated
subjects. Two were real and are the ones moved.

**The third was examined and deliberately left open**, which is the case worth
recording. `2026-09-18-a-decimal-and-a-conditional-update-on-the-wire.md` ::
*"A conditional update is one round trip per row"* scores 0.86 against
`2026-09-18-deleting-a-row-somebody-else-just-edited.md` :: *"A conditional
delete is one round trip per key"*, which is `deliberate`. Reading the reason
shows it does not transfer: it is that two keys in one batch can reach the same
doomed row by different **foreign-key paths**, so a batched delete would have
to union the closures first. A conditional update has no closure to union. Two
sentences a word apart, one settled and one not, and only reading both says
which.

## What this does not do

**It does not sweep the other 124 open rows for duplicates.** The search was
run once over the open set against the settled set and its output read; it was
not run over open-against-open, where two rows could duplicate each other with
neither settled.

**Nothing stops the next duplicate.** The pair found here is two entries a day
apart, the second explicitly pointing at the first, and the tracker recorded
both as open anyway because it keys on `entry::claim` and has no notion of one
gap. A guard would need to know two claims are the same gap, which is the
prose-against-prose problem several entries already record as unautomatable.

**The fifteen verdicts are one reader's, same-day.** They were written by
whoever wrote the entries, hours earlier, which is the least independent
reading there is — and it is the same weakness
`ledger/2026-09-29-the-deliberate-sample-carried-to-168.md` measures in the
`deliberate` bucket at large. These fifteen are now part of that bucket and
inherit its rate.
