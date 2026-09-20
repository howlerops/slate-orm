# A RESTRICT foreign key let its parent go once the child was retired

- **Date:** 2026-09-20
- **Author:** Claude, working through the probes the previous entry named as unrun
- **Touches:** `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/tests/soft_delete.rs`, `docs/orm-comparison.md`
- **Kind:** fix

## What changed

`deletion_closure` walks referencing rows twice — once to gather what a
`CASCADE` will delete, once to find what a `RESTRICT` must refuse for — and both
walks went through one `referencing_rows`, which read with the ordinary
soft-delete filter on. That is right for the cascade arm and wrong for the
restrict arm. `referencing_rows` now takes which it wants; the cascade arm
passes `Deleted::Hidden` and the restrict arm passes `Deleted::Visible`.

Four tests, in `soft_delete.rs` rather than `constraints.rs`, because the claim
is about what a retired row *is* to a constraint, not about foreign keys.

## Why

Retire the only child on a `RESTRICT` edge and the parent delete went through.
The child stayed in storage holding a key to a parent that no longer existed —
readable with `include_deleted`, and, if the restore this repository still owes
is ever built, revivable into a row that violates a constraint its own schema
declares. `RESTRICT` was passing while the thing it guards was true.

The file already argues the case against itself. On `delete`:

> The search for referencing rows deliberately **ignores row-level security**,
> and it has to: a child hidden from the deleter is still a child, and skipping
> it would either leave a dangling reference or make `RESTRICT` pass while the
> thing it guards is true. Integrity is not relative to who is asking.

A soft delete hides a row by the same mechanism — a conjunct installed in
`row_filter` — and the reasoning carries across unchanged. It was not carried
across, because one function served two callers that wanted opposite answers
and only one of them had been thought about.

The cascade arm's answer is the one that was right, and it is worth stating why
rather than leaving it as "unchanged": a retired child's own cascade already
ran, at retirement, through `remove_row`. Reaching it again would re-stamp
`deleted_at` with the later time, pushing its purge deadline out by the gap
between the two deletes. A retention window would silently restart. That is a
worse failure than the one being fixed, which is why the parameter exists
instead of one constant for both.

## Alternatives rejected

**Leave it: a soft delete means the application considers the row gone, so it
should not block.** The defensible reading, and the one the code was
accidentally implementing. Rejected because the row is not gone — it is in the
keyspace, it holds the parent's key, `include_deleted` returns it, and
`purge_deleted` is the only thing that removes it. A constraint that decides by
reading has to say which of "hidden" and "absent" it means, and for referential
integrity the answer is not "whatever the current reader can see". The same
argument the file already makes about row-level security.

**Make it configurable — a per-edge or per-table knob.** Rejected: a knob on a
correctness fix produces two behaviours, one of which nobody runs. The repository
has been bitten by exactly that shape (a check that never fires is a check nobody
has debugged).

**Re-check `RESTRICT` at purge instead**, so the retired child blocks nothing
now and the *purge* is what refuses. Rejected for two reasons. It moves the
refusal to a background sweep, where nobody is holding the request that caused
it, and `purge_deleted`'s doc comment already reasons its way to not
re-checking `RESTRICT` — correctly, for the case it was reasoning about. It
would also not help: by then the parent is already gone.

**Cascade into retired children too, so the graph is always fully walked.**
This is the mutation, and it is caught: it resets the retention clock. Measured
below.

## Evidence

**The defect, over a running `slate-serverd`**, with `strict_kids` soft-deleting
on a `RESTRICT` edge to `parents`, caller holding `everything`:

```
delete parent with a LIVE child (control) -> InvalidRequest: cannot delete from
    `parents`: rows in `strict_kids` still reference it through `sk_parent`
children now: [(10, 1, 'retired')]
delete parent with only a RETIRED child   -> affected=1
children after: [(10, 1, 'retired')]
parents after:  []
```

The control is the point: the same edge, the same delete, the same caller. Only
the child's `deleted_at` differs, and the constraint changes its mind.

**The same probe found two adjacent behaviours that are correct**, and they are
recorded so nobody re-opens them:

- A child inserted *under* a retired soft-deleting parent is refused
  (`foreign key hk_parent: table sparents has no such row`). Hiding the retired
  row makes the constraint stricter there, which is the safe direction — the
  opposite of what it did on the restrict edge.
- `purge_deleted` honours a row policy in both directions. As a principal who
  owns one retired row it erases one; as a principal who owns none it reports
  `affected=0` and leaves both owners' rows intact. The negative case is what
  makes the positive one mean anything.

**Six mutations, six named failures**, `cargo test -p slate-kernel --test
soft_delete`:

| mutation | test that failed |
| --- | --- |
| restrict arm reads `Hidden` (the original defect) | `a_restrict_edge_blocks_on_a_retired_child` |
| cascade arm reads `Visible` | `a_cascade_edge_leaves_an_already_retired_child_and_its_timestamp_alone` |
| parameter ignored, always hidden | `a_restrict_edge_blocks_on_a_retired_child` |
| parameter ignored, always visible | `a_cascade_edge_leaves_an_already_retired_child_and_its_timestamp_alone` |
| restrict arm never fires | `a_restrict_edge_blocks_on_a_live_child` **and** `..._retired_child` |
| cascade arm never fires | `a_cascade_edge_still_retires_a_live_child` |

The clock-moving test is the one worth keeping: at a single fixed instant a
re-stamp is invisible, so an earlier draft that retired and cascaded at the same
second passed under the mutation. Retiring at 1,000 and cascading at 9,000 is
what makes the re-stamp show up as `Some(9_000)` where `Some(1_000)` was
expected.

**The whole Rust workspace**, run crate by crate because disk here cannot hold
every debug test binary at once: all twelve members pass, no failures.
`cargo clippy --workspace --all-targets` is clean, `cargo fmt -p slate-kernel`
applied.

## What this does not do

**It is a behaviour change, and a deployment can feel it.** A schema that
soft-deletes its children and hard-deletes its parents now finds the parent
undeletable until the children are purged, where before the delete went
through. That is the correct refusal and it is still a refusal that did not
happen yesterday. There is no migration path offered and none is needed — the
remedy is to purge, which is the operation that was always meant to be what
finally removes the row.

**It does not revisit whether `RESTRICT` should fire on the soft delete of a
parent at all.** Retiring a soft-deleting parent with a live child is refused
today, and nothing here changed that, although the parent row does not actually
go anywhere and so nothing dangles. That is a pre-existing question and
widening this change to answer it would have made the mutation table
meaningless.

**It does not close the restore gap.** A retired row still cannot be written by
anybody (`ledger/2026-09-20-the-row-that-is-both-there-and-not.md`), which is
the reason the "would violate its own foreign key the moment anybody restored
it" argument above is hypothetical rather than demonstrated. If restore is
built, this fix is what keeps the parent there for it to point at.

**Nothing above the kernel was changed or tested for this.** The daemon, the
three clients and the conformance runner reach `RESTRICT` through the same
`deletion_closure`, so the new answer is the one they get, but no test at those
layers asserts it. The probe that found the defect ran through a real
`slate-serverd` and the Python client, which is evidence the path is reached,
not a regression test that it stays reached.
