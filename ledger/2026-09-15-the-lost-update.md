# Optimistic concurrency: the lost update nothing was reporting

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/src/error.rs`, `crates/slate-orm/src/ext.rs`, `crates/slate-orm/tests/concurrency.rs` (new)
- **Kind:** feature

## What changed

`RecordTransaction::update_if_unchanged(context, table, row, expected)` writes
only if the stored row still equals `expected`, and refuses with
`KernelError::RowChanged` otherwise. At the ORM layer,
`Records::replace_record(context, previous, next)` is the same thing in records.

## Why

The store already detects write-write conflicts, so two transactions writing the
same row *at the same time* is handled. This is the other case, and it is the
one applications actually hit:

1. Request A reads a row.
2. Request B reads the same row, increments a counter, commits.
3. Request A renames the row, writing the copy it read in step 1.

No two transactions overlap, so there is nothing for the store to detect, and
B's increment is silently gone. There is a test that does exactly this and
asserts the increment has vanished — `a_plain_update_loses_the_other_writer_s_edit`
— because a test that only showed the fix would not establish there was ever
anything to fix.

The cost of adding the check is **zero round trips**: `update` already reads the
existing row to enforce the row policy, so the comparison is on bytes that are
in hand.

## Alternatives rejected

**A version column**, bumped on write and compared on update — the usual answer,
and what Hibernate and ActiveRecord do. Rejected because it only detects changes
made by writers who *remembered to bump it*. That makes it a convention enforced
at every call site rather than a property of the data, and the one writer who
forgets is exactly the one whose edit gets lost. Comparing the whole row detects
every change, needs no schema support, and works on tables that already exist.
The cost is comparing a row rather than an integer, on a row that has already
been read.

**Making `update_record` do this automatically** when a version column is
present. Rejected for now with the version column, and separately because it
would change what an existing call does based on a schema detail: the same line
of code would start failing after somebody added a column.

**`&mut` on the record so the version could be bumped in place.** Follows from
the version column and dies with it. It also makes the common call site take a
mutable borrow to express something that is not a mutation of the caller's
value.

**Retrying on `RowChanged`.** Deliberately not offered, and the error's own
documentation says why: a transaction conflict is two writers overlapping in
time, and retrying is right. This is a writer whose *decision* was computed from
a row that is gone. Retrying the same write re-applies the same stale edit. The
caller has to re-read and re-decide, and there is a test showing that loop
working.

**Letting a mismatched `previous`/`next` fall through to the read.** It is
almost redundant — the stored row at `next`'s key will differ from `previous`
anyway, so the same error comes out. Kept, because of the one case where it does
not: when `next` names a row that does not exist, falling through reports
`RowNotFound` and sends the caller to investigate a missing row, when what is
actually wrong is that they handed in a mismatched pair. A mutation removing the
check survived until that case was tested.

## Evidence

Five tests in `crates/slate-orm/tests/concurrency.rs`, each showing both halves:
the plain update losing the edit, and `replace_record` refusing.

Covered: the lost update itself; the refusal, with the store left untouched and
the re-read-and-retry loop succeeding afterwards; an unchanged row replaced
normally; a row deleted underneath, which reports `RowNotFound` rather than
`RowChanged` (a conditional update that resurrected it would be a way to undo
somebody else's delete without noticing); and the mismatched pair, in both the
existing-target and missing-target forms.

### The mutation pass

Five mutations, one survivor:

| mutation | result |
| --- | --- |
| the row comparison is removed | caught |
| a missing row reports `RowChanged` | caught |
| the write applies `expected` rather than `row` | caught (2 tests) |
| `replace_record` becomes a plain update | caught (2 tests) |
| the primary-key comparison is removed | **survived** → new assertion |

The survivor is the redundancy described above: both paths produce
`RowChanged` when the target row exists, so nothing distinguished them until the
missing-target case was added. That case is now asserted with the reason in the
test.

`cargo clippy` is clean and the `slate-orm` and `slate-kernel` suites are green.

## What this does not do

**Nothing on the wire.** The gRPC protocol has no conditional update, so Python,
Go and TypeScript cannot do this. That is the same gap the pagination cursor has,
and the two want the same protocol change: a request that carries the row the
caller believes it is replacing.

**No `delete_if_unchanged`.** Deleting a row somebody else just edited is the
same class of mistake, and the same argument applies. It is not here only because
the update path already read the row and the delete path's shape was not
checked; that is a reason to do it next, not a reason it is fine.

**It does not help two writers who both use it.** Both refuse, both re-read, and
whichever re-reads second wins — which is correct, and is livelock-free only
because each retry sees a newer row. Under heavy contention on one row this is
worse than a lock, and there is no measurement here of where that point is.

**A row that changes and changes back is not detected.** Comparing values rather
than versions means A→B→A reads as unchanged. For a lost *update* that is the
right answer — nothing was lost — but it is not the same guarantee a version
column gives, and anyone who needs "has this row been written at all" needs the
other mechanism.
