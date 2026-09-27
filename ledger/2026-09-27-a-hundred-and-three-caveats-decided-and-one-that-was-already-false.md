# A hundred and three open caveats decided rather than carried. Two were not judgements at all: one had become false and nobody had looked, and one asked for two lines of YAML.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `docs/caveat-status.json`, `.github/workflows/ci.yml`, `scripts/check_closed_caveats.py`
- **Kind:** process

## What changed

Open caveats went from **186 to 83**. A hundred and one became `deliberate`,
each with the alternative named and the reason it was not taken. One became
`narrowed` because it had stopped being true. One became `closed` because it
asked for something that took two lines.

Nothing else. No guard, no test, no code.

## Why

The backlog was measured before this and it did not converge:

| over 15 commits | |
| --- | --- |
| total caveats | **+53** |
| open caveats | **−4** |

Every honest *What this does not do* records the limits of what was just
built, so closing one reliably opens two or three. That is the discipline
working, and it means the open count is not a queue of work — it is a mixture
of a queue with a much larger pile of decisions nobody had made. Asked which
target was wanted, the answer was zero open with `deliberate` acceptable: keep
the honesty, stop parking judgements as though they were tasks.

A caveat marked `open` says "this should be done and has not been". Most of
these said something else: this was considered, and here is the cost. The
tracker has a verdict for that and it was going unused.

## Alternatives rejected

**Leave them open and work through them.** At the rate measured above, zero is
about seven hundred commits away and the ledger passes three thousand caveats.
The number stops being a signal long before that.

**Mark them `deliberate` with a one-line reason.** Faster and worthless.
`scripts/caveats.py` requires a `by` and cannot require that it says anything;
the value of the field is that somebody had to write the alternative down.
Every reason here names what else would have worked and what it would cost,
which is the same bar `CLAUDE.md` sets for the section they came from.

**Widen the tracker so these are a different kind of record.** A fifth verdict
— "a gap we accept" — beside `deliberate`, which already means exactly that.
The vocabulary was adequate; the usage was not.

**Batch them by entry rather than by reading each.** Tempting at a hundred and
three, and it is how the 55% false-positive rate recorded in
`ledger/2026-09-27-five-gaps-decided-rather-than-deferred.md` happened. Each
caveat's paragraph was read from its entry before its verdict was written.

## Evidence

**Two of the hundred and three were not judgements.**

`2026-09-22-widening-the-tree-was-not-widening-the-claim.md` said "**The
general defect from #278 is still open.** No measurement in this repository
records which feature set produced it." That stopped being true when the build
stamp landed. `python3 scripts/check_table_provenance.py` answers `78
measurement tables, 73 predating the stamp`, and `docs/performance.md` carries
`build: slate-slatedb 0.0.1 | features aws, cache | … | slatedb 0.16.0, foyer
0.22.3, object_store 0.14.1, tokio 1.53.1` above its numbers. It is `narrowed`
now, with the 73 frozen tables as the residual. Nothing had noticed, because
nothing re-reads a caveat against the tree except a person deciding to.

`2026-09-25-a-reworded-caveat-is-a-different-claim.md` said "**Nothing
prevents the next orphan.** […] the tracker is not in CI. Adding it there is a
real thing to want and is not done." `scripts/caveats.py` was in
`scripts/check.sh` at line 130 and in `.github/workflows/ci.yml` nowhere — so
a session that did not run the script locally could reword a bullet, detach
its verdict, and push green. Two lines of YAML. `scripts/test_check_sh.py`
reports 108 CI steps, all accounted for.

**The shape of the hundred and one.** Grouped by the reason they were not
taken, counted from the verdicts written:

| why not taken | roughly |
| --- | --- |
| more measurement: another benchmark run, a bigger fixture, a real workload | 20 |
| a feature nobody has asked for (`DISTINCT ON`, an index cursor, a derive attribute) | 20 |
| prose against code, or a parser for a language a guard would have to embed | 15 |
| a test that asserts a line of code, an absence, or a fixture rather than a branch | 15 |
| the container: disk, no Docker, no browser that renders, minutes of CI per case | 12 |
| a design decision deliberately deferred until something wants it | 10 |
| not takeable from here at all — repository settings, a human's eyes | 5 |

The table is a summary written after the fact and the verdicts are the record;
it is here because the distribution is the finding. Two thirds of what looked
like a backlog was cost, not work.

**What this does not claim to have improved.** Nothing in the tree is better
except two lines of CI. The count moved because the count was measuring the
wrong thing.

## What this does not do

**Eighty-three remain open and they are the real ones.** Guards that do not
exist, tests for paths that are reasoned about rather than run, two
authorization paths covered only as superuser, an unbounded array validation,
a fan-out with no ceiling. Those were left open on purpose and are what the
number now means.

**A `deliberate` verdict is a judgement and nothing checks it.** The same
caveat `ledger/2026-09-24-the-ledger-records-762-caveats-and-tracked-none-of-them.md`
recorded about `closed`, and `scripts/check_caveat_citations.py` closed only
the path-resolving half of it. A hundred and one reasons written in one
session by one reader is a large surface for a wrong call, and the only
protection is that each names its alternative concretely enough to argue with.

**Nothing re-reads a `deliberate` verdict.** `--unread` covers `open` and
`narrowed` only, deliberately — a settled verdict carries `reviewed` rather
than `checked` precisely so it does not appear on a worklist. That is right
until a decision's premise changes, and the `narrowed` one above is exactly
that case going unnoticed for five days. There is no mechanism for it and this
does not add one.

**The rate is not fixed, only the backlog.** The next entry will add its own
caveats at the same pace. What changed is that they will be decided when they
are written rather than parked.
