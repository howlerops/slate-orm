# The predicate shape an earlier entry named as unrun

- **Date:** 2026-09-20
- **Author:** Claude, working down the "what this does not do" sections of this week's entries
- **Touches:** `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** process

## What changed

Two tests. No production code.

`ledger/2026-09-20-four-say-absent-one-says-present.md` measured `update_where`
and `delete_where` against a predicate matching *every* row, found them correct,
and closed by naming the shape it had not run: **a predicate matching only the
retired row.** That shape is run now, with its control.

## Why

The measurement that entry made cannot distinguish the two explanations for its
own result. A predicate matching everything touched two of three rows — which is
what "the retired row was skipped" looks like, and equally what "the live rows
were all there was to find" looks like. Both give the same count, so the count
said less than the entry claimed for it.

Selecting the retired row *alone* separates them. It reports zero affected, and
zero rather than an error is the right answer and not an obvious one: a caller
who asked "update everything matching this" and is told nothing matched has been
told the truth, because to them the row is not there.

This is the same boundary the restore fix drew from the other side. **A write
that names a primary key means the row at that key; a write that matches a
predicate means the live ones.** Both halves now have a test that fails if the
line moves.

## Alternatives rejected

**Leave it: the shape was already reasoned about and the reasoning was right.**
It was. That is why this is two tests and no code — but a gap this repository
wrote down and left open is still open, and a reasoned expectation is not a
measurement. The whole reason the entry named the shape was so that somebody
would run it.

**Probe it live against a server, as the original entry did.** A running node is
how that entry found the asymmetry, and it was the right tool then because
nobody knew what the answer would be. It is the wrong tool now: this needs to
keep being true, and a probe run once proves nothing about next month.

**Only the retired-row case, without the live-row control.** Rejected, and the
control earns its place — it would catch the version of this test that passes
because the predicate matches nothing at all: a wrong ordinal, a wrong value
type, a comparison that never fires. "Zero rows were affected" is the cheapest
possible false pass.

## Evidence

Both new tests pass. `cargo test -p slate-kernel --test soft_delete`: 39 tests,
all green.

**The mutation**, on `matching_rows` — the one read both predicate writes go
through — set to `include_deleted = true`:

```
test a_predicate_write_still_skips_a_retired_row ... FAILED
test a_predicate_that_selects_only_the_retired_row_touches_nothing ... FAILED
```

Two failures rather than one, so the new test is not the only thing holding the
line. That is worth saying plainly rather than claiming the test was strictly
necessary: the existing match-everything test *does* catch this mutation. What
the new one adds is not extra coverage of that mutation — it is that the claim
"a predicate skips the retired row" is now asserted by a case where skipping and
not-skipping give different *answers* rather than the same count for two
different reasons.

## What this does not do

**It does not probe the same shape through the wire.** `update_where` and
`delete_where` both have RPCs and all three clients can call them; this is a
kernel test. The predicate reaches the same `matching_rows` from either side, so
the behaviour is the same — that is an inference from the code path, not a
measurement.

**It does not cover a predicate over a *column* rather than a key.** Selecting
the retired row by primary key is the sharpest version because it is
unambiguous, but a real application filters on a status or a timestamp, and a
predicate over a column a partial index covers takes a different plan. The
covering-scan case is tested elsewhere in this file for reads
(`an_index_only_scan_does_not_answer_from_keys_and_resurrect_a_row`) and not for
predicate writes.

**`at_most` and `returning` are untouched.** A predicate write that selects only
retired rows under a ceiling, or with `returning()` asked for, is not run here.
Both go through the same read, and both would report the empty set, but neither
is measured.
