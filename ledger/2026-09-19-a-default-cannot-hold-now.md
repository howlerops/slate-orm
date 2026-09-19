# A default cannot hold "now"

## What changed

One paragraph in `docs/orm-comparison.md`, under **What neither plan does**. It
claimed automatic timestamps and soft-delete conventions were "sugar over
things that already work (a default, a partial index)". Timestamps shipped
yesterday and are not sugar over a default. The claim is withdrawn in place,
with the reason, and the half of it about partial indexes is marked untested
rather than quietly dropped.

## Why

The paragraph was written when neither feature existed, and it was read as
current: it sat under a heading about what is *not* built, three screens below
a gap-table row that says timestamps **are**. A reader who got that far would
have found the document arguing against a feature it had already announced.

The more interesting half is that the reasoning was wrong, not just the tense.
A `DEFAULT` in this schema layer is a stored `Value`. The value automatic
timestamps need is whatever the clock says at the moment of the write, which no
stored value can be. Making a default hold it means an expression evaluated per
write, in a layer that evaluates nothing — a second expression language in the
catalog, for two cases. That is why `created_at` went into the store's write
choke point instead, and it means the word "sugar" was doing work the design
could not support.

Leaving a disproven argument in place is worse than leaving a stale date. A
date is obviously old; an argument reads as reasoning somebody did.

## Alternatives rejected

**Delete the paragraph.** Cheapest, and it loses the part that turned out
right: sugar belongs after the write path settles, and the write path did move
twice during the first six. That ordering call was correct and is worth
keeping — a document that only records its wrong predictions teaches the wrong
lesson about how often they were wrong.

**Edit it to say "timestamps are built" and stop.** This is what staleness
fixes usually are, and it would have left the false reason standing in a
document whose stated purpose is that "the argument is the thing to attack if
you disagree". The cost of withdrawing properly is four sentences.

**Fold it into the soft-delete change**, which will rewrite this same
paragraph and the same subject. Tempting, and rejected because it leaves a
false statement on `main` for as long as that takes, for the saving of one
ledger entry. The correction is independently true.

**Rewrite the gap-table row too.** Not needed — it was already updated when
timestamps landed, and says "Built" with the reasoning. The defect was that the
update stopped there and did not follow the subject to its second mention.

## Evidence

`grep -n "sugar\|deliberately left out" docs/orm-comparison.md` now returns the
withdrawal rather than the claim, and no other mention of automatic timestamps
in the file asserts they are absent.

`python3 site/check/docs.py` passes — the doc is not on the site, but the check
also walks relative links and this paragraph carries none.

No code changed, so no mutation testing applies. Stated rather than skipped
quietly: this entry's evidence is a reading of one file, which is the weakest
kind in this repository, and it is all the change admits of.

## What this does not do

It does not build soft delete, so the partial-index half of the original claim
is still untested — the paragraph now says so instead of implying both halves
were checked. It does not revisit the other prediction in the same section,
that codegen and generated migrations are the two rows most worth a plan next;
half of codegen has since been built and that sentence has not been re-read
against it.
