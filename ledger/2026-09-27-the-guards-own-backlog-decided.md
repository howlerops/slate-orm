# The tracker's own backlog was 29 rows about the tracker

## What changed

Twenty-nine `open` caveats decided: 25 `deliberate`, 2 `closed`, 1 `narrowed`,
1 `moment`. Open falls 85 → 56. Two `WITNESSED` rows and one `EXEMPT_BECAUSE`
reason added to `scripts/check_closed_caveats.py` for the two closures.

The two closures are the only ones that needed evidence rather than judgement:

- `2026-09-26-the-refusal-nothing-ran.md :: "Nothing has run the negative case
  end to end."` The entry said the built `slate-headbench` binaries and the
  runner's exit-2 handling "have not met", and named CI's `headbench` job as the
  first run that would join them. CI run 389's `head-node benchmarks, smoke` job
  is that run: `ok head_concurrency, 91s, refused --not-a-section`, the same
  for `head_report` and `stream_step`, `5 passed, 0 failed`.
- `2026-09-27-five-narrowed-caveats-were-invisible-to-the-staleness-report.md ::
  "Three residuals are now on the worklist and have not been re-read."` Read in
  commit `483154d`. All three still hold; one citation had gone stale, saying
  "all 21 guards under `scripts/`" where `check_guard_scope.py` now reports 22.

The remaining twenty-seven were judgements already argued in the entry that
raised them, sitting under `open` because nobody had written the verdict down.
A representative sample, with what each is now:

- "The witnesses are one person's choices, and 179 of them" — `deliberate`.
  The alternative is a second reviewer, which this repository does not have.
- "Sixteen reasons are sixteen sentences I wrote about my own closures" —
  `deliberate`, same shape.
- "Still nothing re-runs the committed specs" — `deliberate`, because it is the
  *same* caveat `2026-09-26-the-guard-that-only-knew-one-phrasing.md` carries as
  its own, and two open rows for one gap is exactly the double-counting the
  tracker exists to prevent.
- "Five caveats went back on the backlog and none of them got done" — `moment`.
  It is a statement about one sweep, not a standing gap; the five were decided
  in the sweeps since.
- "The fourth invented citation was found by a guard written in the same
  session" — `narrowed`. The residual is that the guard's reach is now `docs/`
  and `ledger/` prose as well as source, so a fifth would be found by something
  older than the session that wrote it; what remains is that a citation to a
  file that *exists but says something else* is still invisible.

## Why

`open` in this tracker means "somebody intends to do this". A row that is
really a judgement — the trade was made, argued in the entry, and will not be
revisited — occupies the worklist forever and makes the real remainder
unreadable. Fifty-six rows is a list a person can read in one sitting and
decide what to work on next; a hundred and eighty-six is a list nobody opens.

The two closures matter for a different reason. Both were written as "this has
not been verified yet", both *had* been verified since, and neither noticed —
the first by a CI job whose log nothing in this container reads, the second by
the previous commit's own work. A caveat that has silently come true is worse
than one that is still open, because the backlog is then wrong in the direction
that loses information: it hides finished work and invites doing it twice.

## Alternatives rejected

**Leave them `open` and work them down one at a time.** This is what the
backlog assumes. It costs: at the rate the previous fifteen commits managed
(+53 caveats written, −4 open), the list diverges. The reason is not laziness —
it is that most rows were never tasks. "The witnesses are one person's choices"
describes a condition of working here, not a defect with a fix, and it would
have stayed `open` through every future sweep, read each time, rejected each
time, at a cost per reading and no progress.

**A `wontfix` verdict distinct from `deliberate`.** Rejected because
`deliberate` already carries the obligation that makes it honest: the schema
requires it to name the alternative *and* why it was not taken. A `wontfix`
with no such requirement is a bin, and a bin absorbs the row that should have
been `open` — the same argument `check_closed_caveats.py` makes at length for
why `EXEMPT_BECAUSE` is one reason per caveat rather than one per kind.

**Batch by grep rather than by reading.** Twenty-nine rows share phrasings
("Nothing checks…", "…is one person's judgement") and a regex would have sorted
them in a minute. Rejected because the previous sweep found three caveats that
had become *false*, and all three read as ordinary standing gaps from their key
alone — the difference was only visible in the paragraph. This sweep found two
more of exactly that kind, which is the same rate, so the reading is the part
that pays.

**Close the two by asserting CI ran, without reading the log.** The `headbench`
job is green, and green is not the same claim. The caveat was specific: the
built binary is handed a name it does not have and exits 2. Only the job's log
says that happened; the job's conclusion says the suite passed, which it would
also say if the probe had been dropped. The `by` quotes the three lines.

## Evidence

- `python3 scripts/caveats.py` — `976 caveats: 56 open, 27 narrowed, 226
  closed, 592 deliberate, 0 untriaged`. Against `HEAD`: `85 open, 26 narrowed,
  224 closed, 567 deliberate`.
- `python3 scripts/check_closed_caveats.py` — 9 of 9, `209 of 226 closed
  caveats have a witness in the tree; the other 17 are exempt`. Before the two
  new rows it failed on exactly the two closures, naming both, which is the
  guard doing its job: it is not possible to close a caveat here without saying
  in the tree what closed it.
- The headbench closure's three `ok` lines were read out of run 389's
  `head-node benchmarks, smoke` job log via `mcp__github__get_job_logs`, not
  from the run's conclusion.
- Mutation, `scripts/mutate.py` over `scripts/test_check_closed_caveats.py`:
  pointing the `headbench-smoke` witness at a flag `ci.yml` does not carry is
  caught by *the real roster: every witness is still in the tree*; deleting the
  refusal closure's `WITNESSED` row is caught by *the real roster: every closed
  verdict names a witness or an exemption*. Both mutations are textual changes —
  one substitutes a different path fragment, one removes three lines.
- `sh scripts/check.sh` — 71 of 71.

## What this does not do

**Twenty-five `deliberate` verdicts are twenty-five judgements I made about
caveats I wrote.** The tracker records the reasoning and nothing tests it; a
row I should have left `open` now reads as settled and will not be offered
again. The alternative is a reviewer, and there is not one. What limits the
damage is that `deliberate` must name the alternative, so a wrong one is at
least legible as a wrong *argument* rather than as an absence — which is the
best a single-writer repository can do and is worth saying plainly.

**The `headbench-smoke` witness is coarser than the closure.** It is the string
`crates/slate-headbench/run.sh --smoke` in `.github/workflows/ci.yml`, which
would survive someone deleting `refuses()` from `run_examples.sh` — the
`bad-section-run` witness covers that half, but nothing ties the two together,
so a future edit that keeps both files and stops the probe running is invisible
to both. A tighter witness would name the probe's output line, and that line is
in a CI log, not in the tree.

**Fifty-six open rows remain, and they are the hard ones.** What is left after
three sweeps is the residue that really is work: the committed mutation specs
nothing re-runs, `ErrorInfo.metadata` unsurfaced, `purge_deleted`'s two-grant
path untested, the unbounded array element check, the relationship fan-out with
no ceiling. Those will not fall to triage, and reporting 56 as progress without
saying so would misread the number: the easy half is gone, and the rate from
here is the rate of doing the work.
