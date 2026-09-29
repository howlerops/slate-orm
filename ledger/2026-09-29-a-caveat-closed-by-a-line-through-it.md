# A caveat closed by a line through it

## What changed

`scripts/caveats.py` grew `struck()` and one rule in `report()`: a caveat whose
claim also stands **struck through** in the same "What this does not do"
section may not be recorded `open`, `untriaged` or `moment`. The strike says a
later entry answered it, so the verdict has to be `closed` or `narrowed`, with
a `by` naming that entry.

It found one, immediately:
`2026-09-19-the-count-a-refusal-did-not-write.md`'s plain-delete-arm caveat,
recorded `open` with *"still unpinned"* — nine days after
`2026-09-20-the-child-that-made-the-arm-reachable.md` pinned it with three
tests. Now `closed`.

The same entry's `Result`-typed-signature caveat moved from `open` to
`deliberate`, its premise re-read against `session.rs` rather than assumed.

## Why

This ledger records "answered later" by striking the caveat and writing the
closure into the strike. The convention deliberately leaves the **original
bullet standing underneath** — the count entry says so in as many words,
because the estimate it got wrong is the useful part of it.

So the section holds the claim twice: once struck, once not. `ANNOTATION`
drops the struck copy, correctly — a withdrawal is not a caveat. What it
leaves is the standing copy, which is *history written in the present tense*,
and it is the only thing the tracker can see. Anyone triaging from the tracker
reads "still unpinned" and records `open`, which is exactly what happened.

Three entries here have such a pair. Two were triaged correctly, by somebody
who read the entry rather than the extract. One was not. One in three is not a
rate worth reasoning from, but the failure is silent and permanent: nothing
about an `open` verdict on a closed caveat ever looks wrong, and the standing
text will keep saying the same thing for as long as the entry exists.

The rule does **not** read the strike as a verdict. It refuses a live one. A
struck caveat closed by a later entry still has to name that entry in `by`,
which is the whole point of the field, and inferring `closed` from a line
through some Markdown would have thrown that away to save a sentence.

`moment` is refused alongside `open` and `untriaged` because it means *a
passing observation, true when written* — and a claim somebody later went and
fixed is not that.

## Alternatives rejected

**Make `ANNOTATION` keep the struck copy and drop the standing one.** The
tidier fix: one caveat per claim, and the one the tracker sees is the current
one. Rejected because it inverts which text is authoritative in a way the
ledger's own convention did not intend — the standing bullet is the record of
what was believed, and the strike is an annotation on it. It would also change
every key derived from a struck bullet at once, orphaning verdicts across three
entries to fix one wrong verdict.

**Strip the strike and rewrite the bullet when a caveat is closed.** The
simplest rule, and the one the repository has explicitly refused elsewhere:
`docs/correctness.md` and `docs/performance.md` both keep withdrawn claims with
the reasoning intact, and the count entry's own text says the wrong estimate is
why the original stands. Fixing the tracker is cheaper than changing how the
ledger writes history.

**Have the strike *set* the verdict to `closed`.** It would have fixed this
instance with no triage at all. It also invents a verdict with no `by`, and a
`closed` row that names nothing is already refused three lines above in the
same function — for the good reason that "somebody says it is closed" and
"here is what closed it" are different claims.

**Widen `KEY` past 60 characters so the two copies differ.** They would not:
the struck and standing bullets here are the *same sentence*, so no prefix
length separates them. It would also orphan verdicts repository-wide. Noted
because it is the first thing that comes to mind and it does not work.

**Leave it, and fix the one wrong verdict by hand.** Two of three pairs were
already right, so the rule buys one correction today. It is worth having
anyway: the cost of being wrong is a caveat that reads as open forever, the
rule is eight lines, and the next pair is written by whoever next closes a
caveat from another entry — which is a thing this session does several times a
day.

## Evidence

Running it against the repository before any verdict was edited:

```
2026-09-19-the-count-a-refusal-did-not-write.md: `The plain (non-conditional)
delete arm's "failed having appl` is open, and the same claim also stands
struck through in that section.
```

One problem, and it is the true one. The other two pairs —
`2026-09-19-decoders-over-the-servers-own-rows.md` and
`2026-09-19-forgetting-a-retired-row.md` — were already `closed` and are not
flagged, which is the rule not firing on the cases it should not.

Three new cases in `scripts/test_caveats.py` (51 passing): the pair recorded
`open` fails, the same tree recorded `closed` passes, and an `open` caveat in
an entry with nothing struck is not flagged. The third exists because a rule
that fired on every open caveat would satisfy the first two.

Six mutations,
`ledger/mutations/20260929T061903-scripts-caveats-py.json`, all caught:

| mutation | caught by |
| --- | --- |
| the rule accepts `open` | `a claim that also stands struck through cannot be open` |
| the rule fires on every caveat | four existing cases |
| `struck()` keeps the leading `~~` | the pair case |
| `struck()` ignores the bold lead | the pair case |
| `struck()` collects unstruck units | four existing cases |
| the rule also refuses `closed` | `the same pair recorded closed is fine` |

**The `Result`-typed caveat's premise, re-read rather than assumed.**
`session.rs` has seven `tally.applied` call sites over four outcome shapes:
`Result<()>` from `insert_many`/`upsert_many` with the applied count derived
beside it; the same accumulated in three different loops, two of which keep
their own counter; `Result<u64>` from `purge_deleted`; and `Result<Vec<Row>>`
from `delete_where` and `update_where`. Three arms would have to build a
`Result` purely to destructure it. That is what the entry said in September and
it is still true, so the verdict is a decision rather than a gap. The entry
names the trigger to revisit it — an eighth arm — and there are still seven.

Open caveats 111 → 109; closed 426 → 427; `sh scripts/check.sh` green.

## What this does not do

**It does not check that the `by` names the entry the strike names.** The rule
demands a settled verdict and a `by`; it does not read the strike's own text to
see which entry it credits. A `closed` row citing the wrong entry passes.

**It matches on the key, so a struck bullet reworded past 60 characters stops
pairing.** Whoever strikes a caveat usually copies it, which is why all three
pairs here match exactly — but a strike that paraphrases is invisible to this,
and looks identical to an entry with no strike at all.

**Nothing stops the pair being created wrongly in the first place.** The
convention is "strike the closure, keep the original", and an entry that only
strikes, or only rewrites, is not checked either way. That is a rule about how
to write an entry and the `pre-commit` hook is where it would live.

**Two of the three pairs were already right, so the rate is one in three on a
sample of three.** That is not a measurement of how often this happens; it is
the whole population today. The rule is worth having on the cost of being
wrong, not on an observed frequency, and saying otherwise would be inventing a
number.
