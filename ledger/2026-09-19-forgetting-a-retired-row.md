# Forgetting a retired row

## What changed

`RecordStore`'s transaction gained `purge_deleted(context, table, before,
at_most)`: it erases outright every row whose soft-delete column is stamped
earlier than `before`, maintaining indexes, and returns how many. A table
without `soft_delete` is refused rather than answered zero.

## Why

The soft-delete entry recorded the gap in its own "what this does not do":
*"there is no reaper, so a soft-deleting table grows without bound."* Stamping
a column instead of removing a row means the row is still there, and nothing
else in the file was ever going to remove it — every delete funnels through
`remove_row`, which is exactly what turns a delete into a stamp.

`before` is an instant, not a duration and not a policy. How long retired rows
are kept is a deployment's decision — a regulator's retention period, a
product's undo window — and a kernel holding an opinion about it would then
have to make the opinion configurable.

## Alternatives rejected

**Purge on a timer inside the kernel.** Needs a background task, a schedule,
and a policy — three things this layer has none of, and a destructive one
running on its own is the last thing to add without an operator asking.

**Bypass row-level security.** Tempting: a purge is housekeeping and the sweeper
is usually an administrator. Rejected because it would make this the one
operation in the file that can destroy data the caller cannot see. It
authorizes `Action::Delete` and scans under the caller's policy, so a caller
reaches exactly the rows it could have deleted.

**Cascade, or re-check `RESTRICT`.** Both were already done at *retirement*:
the cascade ran through `remove_row` and retired the children too. Walking the
graph again would at best repeat itself, and at worst erase a child that is not
yet old enough to purge. Each row is erased on its own timestamp.

**Erase as the cursor walks.** The scan reads the keyspace the deletes write.
Rows are collected first and the cursor dropped before anything is erased.

**Answer zero for a table that does not soft-delete.** A table with no
`soft_delete` column has no retired rows *by construction*, so the call is
aimed at the wrong table — and a caller running it nightly would never find
that out from a cheerful zero. `KernelError::NotSoftDeleting`, mapped to
`FailedPrecondition`: the request is well-formed and the table is the wrong
shape, which is precisely the line gRPC draws against `InvalidArgument`.

## Evidence

Seven mutations. Six caught, one survives and is explained below:

| mutation | caught by |
| --- | --- |
| the bound is ignored, every retired row purged | `…only_rows_retired_before_the_bound` |
| the bound is inclusive rather than strict | `the_bound_is_strict_so_a_row_retired_exactly_then_survives` |
| `include_deleted` left off, so a purge finds nothing | `…only_rows_retired_before_the_bound` |
| a table with no soft delete is a silent no-op | `…does_not_soft_delete_is_refused` |
| the ceiling never fires | `…past_its_ceiling_is_refused…` |
| index entries are left behind | `a_purged_row_takes_its_index_entries_with_it` |
| the `IS NOT NULL` conjunct is dropped | **nothing — see below** |

Two of these needed a second attempt, and both are worth recording.

**The boundary.** `<` mutated to `<=` passed everything. The fixture retired
rows at 1000, 5000 and 9000 and the bounds under test were 6000 and `i64::MAX`,
so no row ever sat *on* the boundary. There is now a test that purges with the
bound equal to a row's exact stamp and requires that row to survive.

**The orphaned index entry.** The first version read through the index with a
filter and asserted nothing came back — and an entry pointing at a deleted row
returns nothing either, because the executor fetches the row, finds it missing
and skips. The test could not see the bug it was written for. It now reads
**index-only**, with a projection of just the indexed column, so the scan is
answered from keys without a fetch and a surviving entry is returned.

### The surviving mutation, which is redundant code

Dropping `Expr::is_not_null(column)` from the predicate changes no behaviour: a
null `deleted_at` compares unknown against any bound and the row is withheld
regardless. The conjunct is genuinely redundant, the mutation is genuinely
undetectable, and the honest options were to delete it or to say so.

It stays, and the code comment now states outright that it is redundant and
that a mutation removing it survives. This is the one operation here that
destroys data nobody can get back, and "live rows are excluded" should be
legible in the predicate rather than deduced from SQL's null semantics by the
next reader. Writing a test for it would be a test that three-valued logic
works, which is a test of `Expr` and not of this.

### A refusal found by writing the fixture

The first fixture seeded ages by *inserting* rows with `deleted_at` already
stamped. Every insert was refused with `RowCheckFailed`: the write path checks
that the writer could read back what it wrote, and an already-retired row fails
its own soft-delete filter. **There is no way to create a row that is born
deleted**, which is correct and was not previously written down anywhere. The
fixture now moves a `FixedClock` between deletes.

`cargo clippy --workspace --all-targets`: clean. `cargo test` over the three
changed crates: 85 suites, no failures — which includes
`every_kernel_error_has_a_code_and_a_reason`, the guard that caught this change
adding a `KernelError` variant with no status code, exactly as designed.

## What this does not do

~~**Nothing calls it.**~~ Partly closed twice over: the gRPC method arrived with
`2026-09-19-a-purge-something-can-call.md`, and the scheduled job with
`2026-09-20-the-sweep-nobody-scheduled.md`. The original text follows.

**Nothing calls it.** There is no CLI flag, no gRPC method, no scheduled job —
a deployment that wants a nightly purge has to reach the kernel directly, which
means the Rust library and not the head node. That is the next step and it is
not here; the wire surface would also have to answer who may run it, which is
the same unresolved question `include_deleted` has.

No metric or log line counts purged rows, so a sweep's effect is visible only
in its return value.

The ceiling is a count, not a byte budget or a deadline, and it refuses rather
than purging a prefix. A caller with ten million retired rows must loop with a
bound it advances itself; nothing here helps with that.

It is untested against SlateDB and against a replica — only `MemoryStore`. The
erase path is the same one hard deletes have always used, so the risk is low
and it is still untested at that layer.
