# Two findings from reading today's changes as a hostile reader

- **Date:** 2026-09-20
- **Author:** Claude, reviewing the least-reviewed code in the repository, which was mine
- **Touches:** `crates/slate-kernel/src/{security.rs,record.rs}`, `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** refactor

## What changed

`SecurityCatalog::grants` is the primitive and `authorize` is built on it,
rather than the other way round. `RecordTransaction::write_many` asks the
grant question once per call instead of once per row. And a test, because
removing the per-row call turned out to break nothing.

## Why

The restore work landed three hours ago and is the newest code here, which by
this repository's own standard makes it the least reviewed. Reading it back as
someone looking for a reason to reject it found two things, and a third that
was already a test rather than a defect.

**`grants` allocated an error it threw away.** I wrote it as
`self.authorize(..).is_ok()`, which is the obvious spelling and builds a
`KernelError::AccessDenied { table: String, action }` — two allocations —
every time the answer is *no*. "No" is the common answer on the only path that
asks: a caller who does not hold `read_deleted`. Inverting the dependency
leaves one copy of the rule, which is what mattered about routing through
`authorize` in the first place, and stops the query path paying for a refusal
nobody reads.

**It was asked once per row.** `write_many`'s arm called
`retired_rows_reachable(context, table)` inside the loop over the batch. Both
arguments are fixed for the whole call, so the answer cannot change between
rows: a thousand-row upsert scanned the grant list a thousand times and — for
a caller without the grant — allocated and dropped a thousand errors. Hoisted.

I have **not measured either**, and neither justification is a performance
claim. The hoist moves work that cannot affect an answer out of a loop; the
inversion stops building a value that is immediately discarded. Both are right
whatever the numbers say, and the pre-existing `permits_row_with` builds a
filter expression per row, which is certainly the larger cost and which this
does not touch.

## Alternatives rejected

**Keep `grants` as `authorize(..).is_ok()` and hoist only.** The hoist alone
takes the per-row allocation down to one, which is small enough to shrug at.
Rejected because the shrug is what makes the next caller of `grants` pay it
again — and the inversion costs nothing, since `authorize`'s body becomes two
lines that read better than what they replaced.

**Duplicate the predicate in `grants` and leave `authorize` alone.** The same
performance, and two copies of "does this context hold this action on this
table" that a future change would have to remember to edit twice. This
repository has been bitten by a two-list version of exactly that — the demo's
query allowlist and its `meta` handler drifted the moment a table was added.

**Cache the answer on the transaction.** Correct today and a trap: it would be
keyed on the table and would silently outlive a context change if anything ever
threaded two identities through one transaction. The value is cheap to compute
once per call and the call is the natural scope.

## Evidence

**The inversion is load-bearing for the whole security layer**, which is the
point of routing `authorize` through it. Two mutations of `grants`, run over
the full `slate-kernel` suite rather than one file:

| mutation | named tests that failed |
| --- | --- |
| drops the superuser short-circuit | `aggregates_compute_what_they_say`, `a_sort_with_a_limit_returns_the_true_top`, and every other suite that reads as the superuser |
| ignores the action | `rbac_gates_each_action_separately`, `a_predicate_write_needs_the_grant`, `a_bulk_update_needs_only_permission_to_update`, `a_reader_may_group_but_may_not_explain_the_grouping` |

**The hoist exposed a missing test, and that is the part worth keeping.**
Setting the hoisted value to `Deleted::Visible` — the grant ignored, every
retired row reachable — broke **nothing**:

```
### the hoisted value ignores the grant (always reachable)
test result: ok. 39 passed; 0 failed
```

Every bulk test in the file runs as the superuser, for whom the answer is
`Visible` either way, so the bulk path's grant boundary was asserted by nothing.
`without_read_deleted_the_bulk_writes_stay_out_of_reach_too` is that assertion:
a caller holding read, insert, update and delete but not `read_deleted`, sending
a batch that mixes a row they may write with the retired one. With it, both
directions fail:

| mutation | test that failed |
| --- | --- |
| hoisted value always reachable | `without_read_deleted_the_bulk_writes_stay_out_of_reach_too` |
| hoisted value never reachable | `the_bulk_writes_restore_a_retired_row_too` |

The batch mixes two rows deliberately: a fix that only looked at single-row
batches would still pass a one-row case, and the trailing assertion is that
row 1 — which the caller *may* write — was not written either, because the
whole batch is refused rather than a prefix applied.

**Suites**: `slate-kernel`, `slate-server` and `slate-orm` pass.
`cargo clippy --workspace --all-targets` clean, `scripts/check.sh` 20/20.

## What this does not do

**No benchmark.** Neither change was measured, and the entry above says so
rather than implying a speedup. `slate-headbench` exists and could time a bulk
upsert before and after; I judged the invariance argument sufficient and the
measurement not worth the disk a release build costs here. If someone wants the
number, the shape to run is a wide `upsert_many` as a non-superuser, where the
old code allocated per row.

**The single-row paths still ask per call**, which is correct — `update` and
`upsert` write one row — but it means `retired_rows_reachable` is on the hot
path of every named-key write to *any* table, including tables that do not
soft-delete, where the answer cannot matter. Guarding on
`table.soft_delete().is_some()` would skip it, and I did not, because the guard
is a second place the "does this table soft-delete" question gets asked and the
scan is cheap. That is a judgement, not a measurement.

**The review found nothing wrong with the `RESTRICT` change**, which I re-read
with the same intent: the superuser context makes the new `ReadDeleted`
authorization inside `referencing_rows` a no-op, and the only cost is that a
partial index on `deleted_at IS NULL` can no longer serve that scan. Untested
and, on a `RESTRICT` edge, unlikely to be anybody's index.

**It is one pass by the person who wrote the code.** That is the weakest kind
of review and it is what was available.
