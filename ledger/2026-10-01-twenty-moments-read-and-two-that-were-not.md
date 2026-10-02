# Twenty `moment` verdicts read, two of which were not moments

- **Date:** 2026-10-01
- **Author:** Claude Code (session: finish the backlog, the docs and the examples)
- **Touches:** `scripts/caveats.py`, `scripts/check_workspace.py`,
  `docs/caveat-status.json`, `ledger/draws/2026-10-01-1001.json`
- **Kind:** process

## What changed

**`moment` has a frame, and twenty of its 146 rows were read.** Two were
wrong. One of those named a mutation nobody had run; it was run, and all three
paths it is about are caught, so the row is `closed` rather than re-labelled.

**`caveats.frame()` takes a verdict**, and `draw()` records which one, so the
sampling machinery that has read 246 `deliberate` rows now points at `moment`
as well. `check_draws.py` needed no change: it validates a record from the
seed and the *recorded* frame size, never from today's frame.

**`check_workspace.py` refuses a member that does not say whether it
publishes.** Unrelated to the audit and in the same commit because it is the
same shape of finding: a default nobody chose.

## Why

Four live caveats across three entries said the same thing, the oldest since
2026-09-29:

> **`moment` and `deliberate` are not audited by anything.** […] Nothing reads
> a moment verdict again, by design — it described a moment — but nothing
> checks that a row marked `moment` really was one.

`deliberate` got its audit — a random draw, read, with the false rate
recorded — and `moment` did not, for a reason that does not survive stating:
`moment` means "nothing to re-read", and that was quietly taken to mean
"nothing to check". Those are different. There is nothing to re-read *if the
verdict is right*, and whether it is right is exactly the thing nobody looked
at. `moment` is the only verdict in the vocabulary that removes a row from
every worklist permanently, which makes it the cheapest place to hide a
standing claim, which makes it the one most worth sampling.

**The two frames are shaped differently, on purpose.** `deliberate` draws from
the *unchecked* rows, so passes pool and the union grows. `moment` draws from
**all 146**, stamped or not, because there is no "unread" subset: the question
is whether the row was a moment at all, and that is as open on a row somebody
stamped as on one nobody has. The cost is that a second audit may re-read a
row; what it buys is a second reader disagreeing with the first, which pooling
cannot give.

## What the twenty said

Eighteen were moments and read as such — a sweep's result, a count on a date,
a confession about one draft, what a particular commit's output ended with.
Three were borderline in a way worth naming (below). Two were not moments:

**`2026-09-15-having-on-a-join.md` — "No mutation testing yet."** The
sentence continues: *"the mutation worth running first is the obvious one —
delete the `having` call — and I would expect the two differentials to catch
it."* That is not a statement about a moment. It is a gap in the test suite,
with the experiment that would close it written out, and `moment` put it
somewhere nobody would ever look again. So the mutation was run.

**`2026-09-20-eight-of-nine-types-fuzzed.md` — "`ValueType::ALL` is new public
API."** The "I did not check" half is a moment; the substance is not. The
constant is still public and the question — should the derive and the three
client generators enumerate through it? — is still live. Re-marked
`deliberate`, with the answer argued in its `by`: each generator maps a type
to a different language's spelling, so a shared roster gives them one list and
three incompatible bodies, and the failure it would prevent is already caught
by the exhaustive match each one has.

**Two of twenty is 10%,** against the `deliberate` frame's two-in-fifteen and
two-in-thirty, so nothing here says `moment` is worse. One reader, one day,
twenty rows: the interval around 10% covers everything from "no worse" to
"twice as bad", and it is reported because it is the first number there has
ever been.

## The three borderline ones, which are a finding about the vocabulary

- *"It does not make the deployed run cheaper to be wrong about."*
- *"It does not prove CI is green."*
- *"The probe is not in the tree."*

Each states where a *change* stops, and that stays true of the change for
ever. `moment` is defensible — the sentence is about one commit — and so is
`deliberate`, because it is a scope decision. Nothing in `caveats.py`'s
definitions separates them, and both readings leave the row off every
worklist, so the choice has no consequence and was never argued. Left as
`moment` and recorded here rather than re-labelled on a coin toss.

## Alternatives rejected

**A guard, rather than a sample.** The caveat says "nothing *checks*", so a
`check_moment_verdicts.py` is the literal answer. Rejected after trying to
write its rule: the only mechanical signal is vocabulary — "yet", "so far",
"this session", "no longer", a run number — and that is the `REACHED_FOR`
roster shape this repository has already been burned by twice. Worse, here it
would be a guard that passes on every input it was built from, which is the
never-fires failure CLAUDE.md names. `deliberate` has a sample and not a guard
for exactly this reason, and the honest answer was to use the mechanism that
already exists rather than invent a weaker one.

**Re-labelling "No mutation testing yet" and moving on.** `open` would have
been correct and taken a minute. Rejected because the caveat named the
experiment, and a row whose own text says what to run is a row where
re-labelling is the expensive option — the work was one `mutate.py` invocation
and it turned a live claim into a closed one with a witness.

**Drawing from the unstamped `moment` rows only**, to match the `deliberate`
frame. Rejected above: there are none in any useful sense — every moment row
carries `reviewed` from the triage that marked it — so that frame is empty and
the draw would have been impossible. Making the frames the same shape would
have meant making the `moment` one useless.

**Forty rather than twenty.** The `deliberate` passes settled on thirty as a
sampling rate. Twenty here because the frame is 146 rather than a thousand, so
twenty is a seventh of it rather than a fiftieth, and because this is the
first audit of this frame — if the rate were terrible, twenty would show it.
It was not terrible, so the next pass can be larger and should draw a fresh
seed rather than extending this one.

## Evidence

**The draw**, `ledger/draws/2026-10-01-1001.json`: seed 1001, frame 146,
twenty rows, reproduced by `check_draws.py` from the seed and the size.

**The mutation the caveat named, run.** Three cases, all three HAVING
lowering paths in `crates/slate-wasm/src/lib.rs`:

| path | caught by |
|---|---|
| joined | `having_keeps_exactly_the_groups_that_pass`, `having_ands_its_terms_over_different_aggregates`, `having_is_refused_where_it_cannot_mean_anything` |
| chained | `having_works_on_a_chain_and_on_a_group_key` |
| single-table | `a_bracketed_having_reaches_the_kernel_and_is_not_silently_dropped`, `a_computed_column_can_be_a_having_key`, `a_group_key_can_be_filtered_alongside_an_aggregate`, `a_sum_over_an_integer_column_stays_an_integer` |

Records: `ledger/mutations/20261001T152145-crates-slate-wasm-src-lib-rs.json`
and `ledger/mutations/20261001T152453-crates-slate-wasm-src-lib-rs.json`. The
caveat predicted the joined case would be caught by the two differentials and
it was; it said nothing about the other two, and both are covered.

**The single-table case first scored SURVIVED, and that was my error.** The
first run's command was `--test taxi`, and the tests that cover the
single-table path live in `tests/having.rs`. One suite reported, none failed,
and the script said *survivor* — correctly, because from where it stands that
is what happened. **A suite that does not exercise the mutated code is not a
missing test**, and `mutate.py` cannot tell the two apart: it counts suites
that reported, not code that ran. That is a seventh way a run misleads, it
belongs to the reader rather than the tool, and the fix is the one the script
already asks for in its survivor message — rule out that the result means
something else before believing it.

**The workspace rule found nothing**, which is the honest result: all fourteen
members already say `publish = false`, each with the reasoning in
`docs/releasing.md`. The rule exists for the fifteenth.

**And a number this entry moved by existing, for the second time today.**
`scripts/test_read_deliberate.py` went red: the later-entry signal's second
labelled row grew from 9 candidates to 10, and the tenth is *this entry*,
which shares "default", "assertion" and "column" with a claim about a
workbench playground's default-column assertion because it quotes a verdict
about a different default column. The same thing happened this morning to
`ledger/2026-10-01-a-check-the-derive-could-not-declare.md`, 8 → 9.

Twice in one day makes it a rate rather than an anecdote, and the rate is the
finding: **this denominator grows with the ledger and the numerator does
not.** A signal whose candidate list grows while its answer stays absent is
one whose precision decays as the repository does. `RANKS` records it; the cut
is still not tuned, for the reason that file gives — the labelled set has two
rows and fitting a parameter to it would measure nothing.

**Not measured.** Nothing here is about speed.

## What this does not do

**Twenty of 146, once.** The other 126 are unread and the next audit should
draw a fresh seed rather than continue this one — the frame is the whole
population, so continuing a draw is just a bigger sample of the same shape
with no pooling benefit to protect.

**The borderline three are not resolved, and nothing resolves them.** A
sentence stating where a change stops fits `moment` and `deliberate` equally,
and the vocabulary does not say which. That is a gap in `caveats.py`'s
definitions, not in this audit, and closing it means deciding what the two
verdicts are *for* rather than what they describe.

**No guard, and the caveat asked for one.** The argument above is that a
useful one cannot be written, which is a claim and not a proof: it rests on
having failed to think of a mechanical signal that is not a vocabulary
roster. Somebody who thinks of one should write it.

**The false rate is one reader's.** The same ceiling
`docs/labelled-verdicts.json` records for the deliberate side, and sharper
here because the judgement is subtler: "was this a standing claim?" has no
tree to check against, only the sentence.
