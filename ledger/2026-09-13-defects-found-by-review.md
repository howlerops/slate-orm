# Twelve defects found by four parallel review streams, and where each came from

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5), backfilled
- **Touches:** `slate-kernel`, `slate-schema`, `slate-server`, `slate-serverd`, `slate-slatedb`
- **Kind:** fix

## What changed

Backfill, written after the fact: an index of the twelve defects found in one
session of adversarial review, so each is findable without reading four agent
reports. The fixes are in the commits named against each. Ten are fixed; two
are open with their reasoning in `docs/security-review.md`.

**Wrong answers** — none of which crashed:

| defect | how it showed |
|---|---|
| `Total::add` discarded every integer after the first float | `SUM` gave 1.5 forwards, 11.5 backwards |
| `narrowed_join` kept the join's `LIMIT` before grouping | 4/2/4/2 building right, 3/3/3/3 building left |
| `BuildTable` flagged a probe per *bucket*, not per row | right outer join with `having` dropped rows |
| index ids validated per table, global in the keyspace | a 3-row table's index scan returned 5 rows |
| `route` fell back to the writer without checking its sequence | `at_least = 2^40` returned every row |
| a failed write left a committable prefix | `update` failing mid-write left 0 index entries for 1 row |

**Cross-tenant** (`docs/security-review.md`): a `CASCADE` from a shared parent
destroyed other tenants' rows; `insert_many` was a free existence oracle over
gRPC; `RESTRICT` disclosed a referencing row. All three fixed. `EXPLAIN`
recovering values from superuser-gathered histograms, and trusted-header
first-copy-wins, remain open.

**Configuration** — shipped defaults that were never the intended ones: SlateDB's
caches compiled out (3 GETs per point read, for ever); `TCP_NODELAY` unset on
every server binding its own listener (154x on a streamed read); the in-process
S3 server likewise (which invalidated a published measurement); the daemon
polling replicas every 10 s against a 250 ms budget.

## Why

Written down because the question the ledger exists for — "where did this come
from and why" — was already hard to answer for this session. Twelve defects
arrived across four agent reports plus my own probing, and the commits that fix
them are interleaved with unrelated work.

## Alternatives rejected

**One entry per defect.** Better in principle and rejected as backfill: twelve
retrospective entries written in one sitting, by someone reconstructing from
reports rather than from the work, would be twelve thin entries. One honest
index beats twelve inventions. Defects found from here get their own entries,
written while the reasoning is live.

**Leaving it to the commit messages.** They are good and they are where the
detail lives — but finding them means already knowing which of thirty commits
to read.

## Evidence

Each fix landed with a failing test that exhibits the defect, and each was
mutation-tested: restoring the bug fails a named test. Two cases worth
recording, because they are the ones that nearly got away:

- The join bucket-flag bug was invisible to 24 hand-written join tests and
  needed the oracle to be taught to generate a cross-side condition. Reverting
  the fix now fails five tests across two files.
- The over-poisoning direction of the transaction fix broke *nothing* in the
  entire kernel suite, which is how the missing check-failure test was found.

## What this does not do

It is an index, not an account. The reasoning for each fix is in its commit and
in `docs/security-review.md`; this only makes them findable.

Two security findings stay open and neither is a patch: the `EXPLAIN` histogram
channel needs a design decision, because statistics must be global to be
correct, and the write-side existence report may be inherent — a unique key is
a shared resource, so refusing to say it is taken means accepting writes that
cannot land.
