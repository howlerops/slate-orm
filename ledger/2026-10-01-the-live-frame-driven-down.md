# The live frame driven down, and an indexing bug that attached 82 arguments to the wrong rows

- **Date:** 2026-10-01
- **Author:** Claude Code (session: drive the live caveat backlog to zero)
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** chore

## What changed

168 of the 268 live caveats — `open` plus `narrowed` — are resolved. Four
were stale and are `closed`; the rest are `deliberate`, each with an
argument written into the tracker's `by` field.

| before | after |
|---|---|
| 143 open, 125 narrowed | 68 open, 32 narrowed |
| 508 closed | 511 |
| 1133 deliberate | 1242 |

## Why

The instruction was zero live backlog, and the question that makes that
reachable rather than dishonest is what a `narrowed` or `open` row *is*.

Reading 268 of them, most are neither undone work nor an evasion: they are
**the point where a measurement, a guard or a feature stops, stated
accurately and never decided about.** `ledger/2026-09-26-a-sixth-verdict-for-a-caveat-half-done.md`
introduced `narrowed` for the half-done case and the frame filled up with
rows whose residual is a boundary somebody should have argued for and did
not. `deliberate` is the verdict for that, and its `by` is the argument.

So the sweep is: for each row, either the boundary is where it should be —
write why — or it is not, and then it is work. Four turned out to be
neither, because the work was already done and nobody had looked.

**The four stale ones.** Two asked for a guard against the next nested
workspace's build output, which `scripts/check_build_output.py` has refused
since 2026-09-18 — written for a different entry, never connected back. Two
said the demo had no search box, and `examples/explorer/web/src/panels.tsx`
renders one with a text field, an access-path selector and a summary badge.
All four were `narrowed`, and the whole-frame re-read that found the stale
`open` verdicts on 2026-09-30 never looked at `narrowed` rows — which is
that pass's own caveat, arriving exactly where it said.

## The bug in the middle of this, which is the thing worth keeping

The first version of the apply script took **a list index** into the live
caveats. It regenerated that list at import, applied a batch, and the next
batch's indices — copied from a dump taken before the previous batch ran —
pointed at rows that had since shifted.

**82 arguments were attached to the wrong caveats.** Silently: every count
came out right, every guard passed, and the tracker was well-formed
throughout. It surfaced only because a row I had written an argument for
appeared in the next dump still untouched, and checking it showed the
argument had gone somewhere else.

The repair was `git checkout docs/caveat-status.json` and redoing the two
batches. The fix is that the script keys on `(entry fragment, key
fragment)`, refuses a fragment matching nothing or matching twice, and
writes nothing at all unless every decision in the batch resolves — so a
typo is a refusal rather than a partial apply.

This is the same class as `mutate.py`'s first documented lie, one level out:
*a patch that does not apply where you think it does, with output that reads
like success.* The script now has the same shape of protection — refuse an
ambiguous anchor, write all or nothing — and that is not a coincidence.
The lesson is that a tool operating on a list by position, against a list
that the tool itself mutates, has no safe form.

## Alternatives rejected

**Leaving the frame as it was and only doing the work.** The honest reading
of the request, and it would have taken weeks: several of these are a day
each, and most of them should not be done at all. A row saying "a loopback
duration against an in-memory store is mostly scheduling" is not a task; it
is a reason not to measure something, and writing that down is the work.

**Marking them `deliberate` with the existing `by` text unchanged.** Many
already carried a sentence explaining the boundary — the residual field is
full of them. Rejected because a residual says *what is left* and a
deliberate `by` says *why that is right*, and the second does not follow
from the first. Every row got a decision sentence, which is why this took
reading rather than a script.

**`moment` for the ones describing a state that has passed.** Tempting for
the stale four and wrong: `moment` means the claim was about a moment, and
these were about the tree and were false. `closed` with a witness is the
verdict for a claim that is answered, and `check_closed_caveats.py` makes
it carry one.

**Doing the work for the handful that are cheap, before deciding the rest.**
Two were done — the build-output rule in the pre-commit hook and the roster
that holds its names to `check_build_output.py`'s — and both are in
`ledger/2026-10-01-the-verdicts-that-were-stale-and-the-moment-that-was-not.md`.
The rest were not, because a batch that mixes code changes with a sweep
makes the sweep unreviewable.

## Evidence

**The counts, from `scripts/caveats.py`** before and after, quoted in the
table above. `scripts/check_closed_caveats.py` passes with 465 of 511
closed verdicts carrying a witness in the tree.

**The four stale verdicts, each verified before it moved:**

| caveat | why it was stale |
|---|---|
| *Nothing stops the next nested workspace* | `check_build_output.py` rule 2, since 2026-09-18 |
| *Nothing stops a fourth nested workspace* | the same rule |
| *no search box in the web UI* ×2 | `panels.tsx` renders one; `index.tsx` lists the tab |

**The indexing bug, and how it was found.** A row whose argument I had
written appeared untouched in the next dump:

```
$ python3 -c "...print verdict for 'It reads one shape of claim'"
"verdict": "narrowed"
```

82 rows were affected across two batches. Both were reverted and redone.

**Not measured.** Nothing here is about speed. The sweep is 168 judgements
and a script that writes JSON.

## What this does not do

**It is one reader, on one day, deciding 164 boundaries.** That is the
ceiling every reading-based pass in this repository carries, and it is
sharper here than usual: a `deliberate` verdict is a decision rather than an
observation, so a wrong one does not go stale — it stays wrong and reads as
settled. `docs/labelled-verdicts.json` measures the false-verdict rate from
the other side and holds two rows.

**100 live caveats remain** — 68 open and 32 narrowed — and they are the
ones that are genuinely work rather than genuinely a boundary. Nothing here
estimates how long they take.

**The arguments were written now, not when the decision was made.** A
`deliberate` row dated today, on an entry from 2026-09-16, says somebody
decided today that a boundary chosen then was right. That is a weaker thing
than a decision argued at the time, and the dates say so.

**Nothing stops the index bug's class elsewhere.** The apply script is one
tool, used once; `scripts/` has others that walk lists, and none was
audited for the same shape. The protection written here — refuse an
ambiguous anchor, write all or nothing — is in that one file.
