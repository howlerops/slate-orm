# The open frame read against the tree, and one asymmetry closed

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `docs/caveat-status.json`, `scripts/test_check_sh.py`
- **Kind:** chore

## What changed

Every `open` caveat — all 141 — was read against the tree in one pass, and
each now carries a `checked` date. Five changed verdict: three closed, two
narrowed. One of the remaining open ones was closed by fixing it.

| before | after |
|---|---|
| 141 open, most last read on 2026-09-28 or never | 135 open, all read 2026-09-30 |
| 122 narrowed | 124 |
| 497 closed | 501 |

`scripts/caveats.py --unread` reports **0** open or narrowed caveats not
re-read in 30 days. It reported 1 before the pass and had no way to report
the other 140, because a verdict with no `checked` date is invisible to a
staleness report rather than listed by it — the quieter of the two failures
and the one this pass is for.

## Why

`ledger/2026-09-30-the-whole-deliberate-frame-read-in-one-pass.md` emptied
the `deliberate` frame and left the `open` one untouched. Its own caveat
said so, and so did
`ledger/2026-09-29-three-verdicts-that-were-wrong-not-three-gaps.md`:

  > **Seventy-six open caveats last checked on 2026-09-28 were not re-read.**

An open caveat is a live assertion about the current tree. The cost of not
re-reading one is not that it sits in a list: it is that somebody builds a
feature that already exists, which that entry recorded as
*"the most expensive way to discover a stale verdict"*. Three of the five
verdicts it found wrong were exactly that.

## What "read against the tree" means here, exactly

Because the stamp is only worth what the read behind it was:

1. **Every one of the 141 claims was read**, in four batches, in full.
2. **The mechanical half was run over all of them** — the one
   `scripts/read_deliberate.py` does: resolve every backticked token in the
   claim against `git ls-files` and `git grep`. **Zero** tokens failed to
   resolve, so no open caveat is stale in the way the two false `deliberate`
   verdicts were, where the subject had been deleted.
3. **Claims whose truth could plausibly have changed were checked against
   the code** — the derive's check attributes, the decimal surface in the Go
   and TypeScript clients, `delete_if_unchanged` in the SQL front end, the
   catalog generator, the demo's grouping panels, and the frame counts
   behind the five meta-caveats.
4. **No measurement was re-run.** A caveat saying *"the ratio is one shape
   at one size"* is confirmed by there still being one shape, not by
   re-measuring it. This is the ceiling
   `ledger/2026-09-25-the-open-caveats-nobody-re-reads.md` prices at about
   six times in seven, and it applies here.

## Alternatives rejected

**Stamping without reading.** The fastest way to make `--unread` report
zero, and it would make the number a lie in the direction that matters most
— a staleness report that says everything is fresh is worse than one that
says nothing, because the first is believed.

**Sampling rather than reading all of them.** Five campaigns sampled the
`deliberate` frame at thirty a session to put an interval on how many
verdicts are false, and
`ledger/2026-09-30-the-whole-deliberate-frame-read-in-one-pass.md` recorded
why that was the wrong instrument for clearing a backlog: a rate chosen for
inference is not a rate chosen for throughput. 141 rows is a morning, and an
interval over a frame you can read entirely is a worse answer than reading
it.

**Closing more of them.** Tempting, and wrong. 135 of the 141 are correctly
open: narrow, specific, still-true statements about work not done —
*"no client surface"*, *"it measures no latency"*, *"one index set"*. A
verdict moved to `closed` without something in the tree to point at is the
failure `scripts/check_closed_caveats.py` exists to catch, and it would
catch it.

**Doing the work behind them instead.** Several are a day's work each
(`RETURNING` projections, a typed row across three clients, per-tenant
statistics). Re-reading is what makes choosing between them possible, and
this pass is the thing that has to happen before that choice, not instead
of it.

## Evidence

**The five that changed**, each verified before it moved:

| caveat | was | now | why |
|---|---|---|---|
| *747 remain unread* | open | closed | the frame is empty; `--unchecked` reports 26 and all 26 are verdicts written **after** that pass |
| *Seventy-six open caveats … were not re-read* | open | closed | this pass |
| *It did not look for stale reasons systematically* | open | closed | the whole-frame read is the systematic version, and found two false verdicts |
| *It says nothing about the 110 narrowed rows or the 1018 deliberate ones* | open | narrowed | the deliberate half is done; the narrowed rows are still unswept as a group |
| *`moment` and `deliberate` are not audited by anything* | open | narrowed | `deliberate` is now; nothing checks that a row marked `moment` really was one |

**The mechanical half, over all 141:** every backticked token resolves.
`0 tokens that no longer resolve`.

**One fixed rather than re-read.**
`ledger/2026-09-29-the-file-named-after-the-guard-carried-the-stale-count.md`
recorded an asymmetry: `scripts/test_check_sh.py` reads `check.sh`'s job
count with an anchored pattern and `CLAUDE.md`'s with a loop over every
`<word> jobs` in the file. The loop cannot tell a count from ordinary
English — it once reported `'the'` as a number it did not recognise — and a
guard that cries wolf is one people stop reading. `CLAUDE_MD_COUNTS` now
anchors both of CLAUDE.md's occurrences, separately, so a message names
which sentence is wrong.

**Two mutations, both caught**
(`ledger/mutations/20260930T211202-claude-md.json`): one job count drifted to
`twenty-three` while the other stayed right, and the second sentence
reworded from *"A push starts"* to *"A push kicks off"*. The first is the
drift this guard exists for; the second is the price of anchoring, failing
loudly as *"the sentence was reworded — update the pattern"* rather than
silently checking nothing.

**`sh scripts/check.sh`: 93 passed, all of them.**

## What this does not do

**It re-read, it did not re-derive.** Point 4 above. Every claim that rests
on a measurement — and a good third of the 135 do — was confirmed by the
measurement still being the only one, not by taking it again. A number that
has drifted since it was recorded would survive this pass.

**135 open caveats are still open.** The backlog is *read*, not worked. What
this changes is that choosing what to do next is now a decision over a list
somebody has looked at, rather than over a list nobody has.

**The 124 narrowed rows were not swept as a group.** They each carry a
residual and each carries a `checked` date recent enough that `--unread`
does not list them, which is not the same as somebody having read them
together and asked which residuals are still real. That is the caveat this
pass moved from open to narrowed rather than closed.

**`moment` is still audited by nothing.** A `moment` verdict says "this
described a moment and needs no re-reading", and nothing checks that the
claim really was about a moment rather than about the tree. Twenty-two of
them were triaged in one batch on 2026-09-28 and that batch's own entry
called them the weakest in it.

**One pass by one reader.** The same ceiling every reading-based closure
here carries, and the reason the mechanical half exists: it is the part that
does not depend on attention.
