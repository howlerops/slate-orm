# The cost-prose guard's two exemptions, both of which let a live claim through: a marker nobody had to deserve, and a strikethrough that skipped whole paragraphs.

- **Date:** 2026-09-26
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_cost_prose.py`, `scripts/test_check_cost_prose.py`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

`check_cost_prose.py` now reports a `<!-- not a cost-model claim -->` marker
sitting over a passage that states no cost-model figure. And `live()` carries a
struck-span flag from chunk to chunk, so an unbalanced `~~` is a boundary
rather than a verdict: the live side of it is read and the struck side is not,
whichever side that is, and a paragraph wholly inside an open span is skipped
because it is inside one rather than because it happens to contain a marker.

## Why

Two caveats, recorded four days apart, that are the same shape: an exemption
that could not be wrong.

**The marker.** `ledger/2026-09-22-the-page-a-reader-actually-reads.md` named
this in the entry that introduced the marker — *"A paragraph rewritten from
history into a live claim keeps its marker and goes unchecked. The roster-style
answer — report markers that no longer sit above a matching claim — would catch
it; this does not do it."* The failure is slow and silent: a passage is written
as history, marked, and rewritten sessions later into a statement about today.
Nothing in the diff says the sentence changed kind, and the figure the marker
now covers is the one figure on the page nobody checks. This repository's whole
argument about `EXPECTED_REFUSALS` is that an exemption nobody has to justify is
an exemption that spreads.

**The strikethrough.** `ledger/2026-09-22-widening-the-tree-was-not-widening-the-claim.md`:
*"A chunk with an unbalanced `~~` is skipped whole. Joining runs makes an
unbalanced span rarer, not impossible, and a skipped chunk is a claim nobody
checks."* The old rule was conservative in the expensive direction — a
paragraph whose **last clause** opens a struck span had its first three
sentences skipped along with it.

## Alternatives rejected

**For the marker: require the marker to name what it excuses.** `<!-- not a
cost-model claim: POINT_READ_COST -->`, checked against the patterns that
matched. Sharper, and it makes every existing marker wrong at once — four in
the tree — for a precision nothing needed: a marker covers one paragraph, and a
paragraph carrying two different stale figures is not a case that has come up.
Rejected as a migration with no failure behind it.

**Warn rather than fail on a stale marker.** A stale marker is not itself a
wrong claim, so a softer verdict is arguable. Rejected because this runner has
no warning level and inventing one here would be the first: a check that prints
something and exits 0 is a check that gets read once. The `check.sh` run is
already the place where a green run means something.

**For the strikethrough: drop from the unbalanced marker to the end of the
chunk, always.** One line, no state, and wrong half the time — it assumes the
lone `~~` is an opener. In the chunk that *closes* a span it is the closer, and
that rule would throw away the live text after it while keeping the struck text
before it, which is the guard reading exactly the wrong half. The flag is four
lines and knows which.

**Refuse an unbalanced `~~` outright, as a formatting error.** Would make the
state unnecessary, and it forbids a legitimate thing: a struck passage spanning
two paragraphs is ordinary Markdown, and `docs/performance.md`'s withdrawal
passages are written that way. A guard that demands the prose be reshaped to
suit it is a guard people work around.

## Evidence

**Mutations.** Two runs, ten cases:

- `ledger/mutations/20260926T233138-scripts-check-cost-prose-py.json`, nine
  cases: eight caught, one survivor.
- `ledger/mutations/20260926T233222-scripts-check-cost-prose-py.json`, the
  survivor re-run after the missing test was written: caught.

| mutation | outcome |
| --- | --- |
| the own-line marker is never judged | caught |
| the inline marker is never judged | caught |
| a marker at the end of a file is forgotten | caught |
| a marker displaced by a second marker is forgotten | caught |
| `claimed()` returns `True` for everything | caught, 2 named cases |
| `PATTERNS` loses one of its five | caught, by the case holding it to `check()` |
| an unbalanced `~~` skips the whole chunk again | caught, 2 named cases |
| the open-span flag is never carried forward | caught |
| a chunk wholly inside a struck span is read as live | **survived** |

The survivor was a missing test, not redundant code: every fixture's struck
span opened and closed within two paragraphs, so the arm that handles a
paragraph with no `~~` of its own and a span still open was never reached. A
three-paragraph fixture was written and the mutation is caught.

**The existing fixture that had to change, and why it is worth recording.** The
case *"but not the one after that"* excused a paragraph reading `History.` —
with no figure in it. Under the new rule that marker is undeserved, and the case
failed. The fixture was not wrong before; it was *about* one rule and silently
also an instance of another. It now excuses a paragraph with a figure in it, so
it is about marker reach and nothing else.

**Suites.** `scripts/test_check_cost_prose.py` 60 passed 0 failed (was 51).
`scripts/check_cost_prose.py` against the tree: 25 prose claims, all current,
and none of the four live markers is undeserved. `sh scripts/check.sh`: 65 of 65.

## What this does not do

**No marker in the tree was found stale.** Four exist and all four sit over a
passage with a figure in it, so this change caught nothing today; it is a guard
against a future edit, which is what the caveat asked for. Saying that plainly
is better than implying a defect was found — the run above is the evidence that
there was none.

**The rule asks whether a figure is there, not whether it is history.** A marker
over a paragraph that states a *current* figure correctly is still accepted, and
a reader who marks a live claim to silence the guard succeeds. That is the same
trust the `~~` exemption takes, and narrowing it means deciding from prose
whether a sentence is about now — which is the judgement the marker exists to
delegate to a person. The alternative worth having is the one rejected above:
making the marker name what it excuses.

**A struck span is still tracked per file and per chunk list, not per
character.** Nested or overlapping `~~` spans, and a `~~` inside a code span or
a link, are read as ordinary markers. Markdown does not nest strikethrough and
fenced blocks are already skipped, so the remaining case is a literal `~~`
inside inline backticks, which no page here writes; a real parser is the fix if
one ever does, and this guard is three hundred lines of regular expressions by
design.
