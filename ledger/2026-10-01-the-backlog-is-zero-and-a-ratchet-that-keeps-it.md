# The backlog is zero, and a ratchet so it stays that way

- **Date:** 2026-10-01
- **Author:** Claude Code (session: finish the backlog, the docs and the examples)
- **Touches:** `docs/caveat-status.json`, `scripts/check_live_frame.py`,
  `scripts/test_check_live_frame.py`, `scripts/mutate_guard.py`,
  `scripts/test_mutate_guard.py`, `scripts/mutations.json`, `scripts/check.sh`,
  `.github/workflows/ci.yml`
- **Kind:** process

## What changed

**No caveat in this repository is `open`.** The live frame — `open` plus
`narrowed` — is **0 and 2**, from 268 this morning and 762 when the tracker
was built. The two that remain are the same caveat written in two entries, and
both close on one event.

**`scripts/check_live_frame.py` holds it there.** Two rules: no verdict is
`open`, and every `narrowed` one is named in `STILL_NARROWED` with what is
left and what closes it. It runs in `check.sh` and in CI.

**`scripts/run_mutations.py` has been run end to end**, for the first time
since it was written: 31 suites, and the run found a defect in
`mutate_guard.py`.

## Why

| | morning | now |
|---|---|---|
| open | 143 | **0** |
| narrowed | 125 | **2** |
| closed | 508 | 518 |
| deliberate | 1133 | 1348 |

The instruction was zero live backlog and the honest question is what that
can mean. Reading the frame, almost none of it was undone work: it was **the
point where a measurement, a guard or a feature stops, stated accurately and
never decided about**. `deliberate` is this repository's terminal verdict for
exactly that, and its `by` is the argument. So the sweep is one decision per
row — either the boundary is where it should be and here is why, or it is
not and then it is work.

Six rows turned out to be work and were done rather than argued; they are in
the three entries beside this one. The rest are arguments, grouped by theme so
that the answer to "why is this not measured" is one argument cited eleven
times rather than eleven paraphrases: a loopback duration is mostly
scheduling, a guard that needs a parser stops being a guard somebody reads, a
roster of spellings is a guess with a known failure mode.

**And the count going to zero is worth nothing on its own.** It took three
sweeps across two days to get here; each new entry writes three or four
caveats, so the frame refills at the rate work gets done. A number reached by
effort and held by nothing is a number that is wrong again next week, quietly,
which is the whole argument for the ratchet.

## What the ratchet refuses, and what it deliberately does not

It refuses `open`. It does **not** refuse a gap — the other four verdicts are
all still available and all say something: `closed` with a witness,
`deliberate` with the argument, `moment` if the claim was about one run,
`narrowed` with a roster row. What it refuses is leaving a caveat
*undecided*, which is the state that costs nothing to create and accumulates.

`narrowed` is rostered rather than banned because it is the one live verdict
that is legitimately live — something is genuinely half done — and a roster is
what stops it becoming where `open` rows go to hide. One entry per caveat,
naming what closes it, in the `EXEMPT_BECAUSE` idiom this repository already
uses twice: a list you are forced to edit is a list that stays true.

## Alternatives rejected

**A ceiling on the live count rather than zero.** `live <= N`, lowered as the
frame shrinks. Rejected because the number is the wrong thing to hold: a
ratchet on a count lets one row close and a worse one open, and the ledger
would read as improving. The verdict is what matters, and `open` is the only
one that admits to having no reasoning behind it.

**Refusing a new caveat whose entry is older than today's newest.** The first
design, and it fails on the state this entry actually ends in: the two
remaining `narrowed` rows are from 2026-09-29, so the rule would refuse the
truth. A rule a correct tree cannot satisfy is a rule somebody deletes.

**Leaving the frame as it was and only doing the work.** The honest reading of
"zero backlog" and it would take weeks: several of these are a day each, and
most of them should not be done at all. A row saying "a loopback duration
against an in-memory store is mostly scheduling" is not a task; it is a reason
not to measure something, and writing that down *is* the work.

**Marking the remaining two `deliberate` to reach a clean zero.** Tempting,
and it would have been a lie of exactly the kind this tracker exists to catch:
the mutation workflow genuinely has not fired, and "nothing runs it on a
schedule" is not a boundary anybody decided on — it is a button nobody has
pressed. `narrowed` with a rostered reason is what that state is.

**Not adding the roster, and letting `narrowed` pass unexamined.** One fewer
list. Rejected because `narrowed` would then be the escape hatch — write
`narrowed`, write a residual, never look again — and that is the shape `open`
had before this sweep.

## Evidence

**The counts**, from `scripts/caveats.py`, in the table above. `0 open` is the
first time since `docs/caveat-status.json` was created on 2026-09-24.

**Five mutations of the new guard, all caught**
(`ledger/mutations/20261001T160213-scripts-check-live-frame-py.json` and
`ledger/mutations/20261001T160229-docs-caveat-status-json.json`):

| mutation | caught by |
|---|---|
| the no-open rule never fires | `one open verdict fails` |
| a narrowed caveat need not be rostered | `a narrowed verdict nobody rostered fails` |
| a roster row covering two caveats is accepted | `a roster prefix matching two narrowed caveats fails` |
| the empty-tracker refusal is dropped | `an empty tracker fails rather than passing on nothing` |
| **a settled verdict reverts to `open`** (the real tree) | `check_live_frame` refuses |

The last is the one that matters: the others prove the rules fire against
written fixtures, and that one proves the guard is pointed at this
repository's actual tracker.

**The roster match is a bijection, and the first version was not.** Checking
only "every narrowed caveat has a roster row" passes when one row's prefix
covers two caveats — the roster looks complete and one caveat is wearing the
other's reason. Found by writing the case and watching it pass; the guard now
checks from both ends and the case fails as it should.

**`run_mutations.py`'s first full run: 31 suites, and it found a defect.**
`check_caveat_citations` reported as *red before any mutation*, with the line

```
the suite is red before any mutation: ['check_caveat_citations: ok    docs/caveat-status.json parses  (2014 verdicts)']
```

Two things, and both are findings.

The guard really was red — this session's own sweep had introduced three
invented ledger citations and an absence claim with no search behind it, and
`check_caveat_citations.py` caught all four. **That is the fourth time in one
day I have cited a ledger entry that does not exist**, and the only reason it
is not in the tree is that a guard reads every citation.

And the *message* was misleading: `mutate_guard.py` quoted the guard's first
line, which for every guard here is an `ok` line, because they print their
passing rules before the failing one. A refusal reported with a sentence
beginning `ok` sends the reader looking for a parser bug. It now quotes the
refusing line, with a test (`a refusal quotes the line that refused`) and a
fixture guard shaped like a real one. The same
names-what-it-found-not-what-the-reader-wanted class
`ledger/2026-10-01-the-other-messages-that-name-what-they-found.md` spent a
session on, one level out, met the same day.

**After the fix: 31 suites clean, 0 with findings.**

**Not measured.** The guard reads one JSON file. `check.sh` is 95 checks now
and still finishes in the time it took at 93.

## What this does not do

**It cannot tell an argument from a sentence.** A `deliberate` verdict with
filler in its `by` passes, and 1348 rows carry one. What measures that from
the other side is `docs/labelled-verdicts.json`, which holds two labelled
false verdicts, and the sampling passes that feed it. The ratchet only refuses
the verdict that admits to having no argument at all.

**215 decisions were written by one reader in one day.** That is the ceiling
`ledger/2026-10-01-the-live-frame-driven-down.md` already recorded for its own
168, and it is sharper now because the total is 383 across the two sweeps and
because a `deliberate` verdict does not go stale — it stays wrong and reads as
settled. Several of the arguments here are one sentence where the entry they
decide deserves a paragraph.

**The two narrowed rows close on an event nobody here can cause.** The
mutations workflow is on `main` and its cron is live, so it will fire on
Monday; `workflow_dispatch` would fire it now and needs a push-access token
this session does not have — `POST
/actions/workflows/mutations.yml/dispatches` returns 403. Running the roster
by hand, as this entry did, is not the same thing and the roster rows say so.

**Zero is a property of the tracker, not of the repository.** It means every
recorded caveat has a verdict with reasoning behind it. It does not mean there
is nothing left to do — `docs/orm-comparison.md`'s gap table is the list of
things this does not have, and it is long on purpose.
