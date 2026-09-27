# Five open caveats that were waiting for a decision rather than for code. Each is decided here, with the alternative and what it costs — and six more I had lined up turned out to be decided already.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

No code. Five caveats move from `open` to `deliberate`, each because a decision
was made and written down here — the alternative named, and what it would cost.

The standard is the tracker's own: `scripts/caveats.py` says `deliberate` is for
a caveat whose entry names *what else would have worked and why it was not
taken*. A caveat that is a question nobody has answered is `open`; one answered
"no, and here is why" is not work, and counting it as work makes the backlog lie
about how much is left.

## Why

Reading the 185 open caveats in order, a class stands out: not missing tests or
unfinished features, but **questions with a right answer that nobody had
committed to**. Each has an obvious implementation and a real reason not to
build it, and leaving it `open` puts it on a list a future session reads as a
to-do.

## The five

**1. Japanese and Chinese tokenize into one enormous term.**
`ledger/2026-09-21-one-entry-per-term.md`. The split is `char::is_alphanumeric`
and CJK does not put spaces between words. *Alternative:* bigram tokenization,
the standard dictionary-free fallback, or a real segmenter. *Cost:* both make
the tokenizer part of the **index's identity** — a query has to tokenize the
way the index did, so the choice has to be stored with the index and migrated
when it changes, and `docs/full-text.md` has not made that decision. Bigrams
also roughly double the term count for CJK text, which is a storage decision
for a feature nothing outside the kernel uses yet. *Refused* until full text
crosses the kernel boundary, which that entry's first caveat names as the next
piece.

**2. No screenshot or visual regression.**
`ledger/2026-09-20-the-undo-a-visitor-can-see.md`. *Alternative:* a committed
screenshot baseline per page, compared in the e2e. *Cost:* a binary in the
tree, regenerated on every styling change, in a repository whose
`site/build-wasm.sh` argues at length against committed binaries — and the
failure it catches, a layout that renders the badges unreadably, is one a
person sees the first time they open the page and a text assertion never can.
*Refused.*

**3. Plausible is a low bar** and **4. It does not know what a column means**,
both `ledger/2026-09-21-generate-the-rows-from-the-table.md`, one decision.
*Alternative:* a column-name registry — `email`, `country`, `phone` — mapping
names to generators. *Cost:* it guesses, and the guesses are wrong in the cases
that matter: `country_code` is not `country`, `user_id` is not `id`, and `name`
in the demo's `zones` table is a borough. A row that *looks* real and is not is
worse for a fixture than one that is obviously synthetic, because a reader stops
checking it. The generator's contract is "a row that satisfies the schema";
widening it to "a row that looks real" needs a different contract, not a bigger
vocabulary. *Refused.*

**5. It does not examine side channels.**
`ledger/2026-09-20-where-else-does-this-live.md`. Plan choice and response
timing both leak the same global statistics `estimated_rows` does, to a caller
with a valid identity and no `Explain` grant. *Alternative:* constant-time
planning, or a plan that does not vary with statistics. *Cost:* the planner is
what this system is *for* — its whole value is choosing differently when the
statistics differ, so the mitigation removes the feature. A caller who can
measure the difference already holds a valid identity for that tenant, and what
they learn is the shape of their own data. *Refused as outside the threat
model*, which is the decision recommendation 3 of that review left open and
nobody had made.

## Alternatives rejected

**Leave them open and build them one day.** What the backlog was doing, and the
cost is a list that cannot be finished and that a reader cannot tell apart from
one that can.

**Mark them `moment`.** Cheaper and wrong: `moment` is for a statement about
one run that was never a standing claim. All five are still true and will stay
true — they are decisions, not observations.

**Write five separate entries.** One decision per entry is the usual shape here
and it is right when the decision has evidence behind it. These share one
argument — an implementation exists and is not worth its cost — and five
entries would bury it five times.

## Evidence

**Six of eleven candidates were already decided, which is the measurement worth
keeping.** I picked eleven caveats that looked like decisions and checked each
against the tracker before writing. Six were already `deliberate`: the e2e's
`MUST_DIFFER` pair, the per-SDK panel loop, the identity switch, the client
factory, the `--seed` integration, and the derive-based factory. Only five were
`open`.

That is a 55% false-positive rate on *my* reading of which caveats look
undecided, and it says something useful in both directions: the earlier sweeps
did better than the backlog's size suggests, and "this looks like a decision" is
not a verdict — checking is. Had I written the entry before checking, it would
have claimed eleven decisions and made six of them twice.

**The tracker after the change:** `python3 scripts/caveats.py` reports **181**
open where it reported 185, nothing `untriaged`, and
`scripts/check_caveat_citations.py` confirms every verdict's citation resolves.

Five closed and four fewer: this entry's own *What this does not do* adds three
caveats, two of them decided here and one — the threat-model premise below —
left open, because it is a thing to test rather than a thing to decide. That
arithmetic is the honest shape of every entry in this ledger and is worth
stating once: an entry that closes five gaps and opens one has moved the
backlog by four, not five.

**No measurement, and that is the point rather than an evasion.** A decision has
no measurement; what it has is an alternative and a price, and each of the five
above names both.

## What this does not do

**It decides; it does not implement.** Every one of the five could be built, and
each says what building it would look like. A later session that disagrees
should reverse the verdict and say why, which is cheaper than discovering the
argument again.

**It does not touch the caveats that are genuinely unfinished work.** A CJK
tokenizer is a decision; a `DISTINCT ON` is not. Sorting the second kind is not
something a reading can do, and the remaining open caveats are mostly the second
kind: missing tests, unmeasured claims, and features with no decision behind
them.

**The threat-model decision in 5 is the one worth arguing with.** It is the only
one of the five that is a security judgement rather than a cost judgement, and
it rests on a premise nobody has tested: that a caller who can time a query
already holds a valid identity for that tenant. If that premise is ever wrong —
an unauthenticated path that plans — the decision goes with it. The nearest
evidence is finding 9 in the same review, which found the leadership RPC
answering unauthenticated; that one was fixed, and it is the reason this premise
deserves a test rather than a reading.
