# The partial index that could have reopened the `RESTRICT` hole

- **Date:** 2026-09-20
- **Author:** Claude, working the caveats out of this morning's entries
- **Touches:** `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** fix

## What changed

Two tests and a guard. The tests close the two surfaces
`2026-09-20-reviewing-my-own-newest-code.md` listed as untested; the guard is
one of them turning out to be a cost question with an obvious answer.

## Why

### A hypothesis worth killing: the index the search cannot see

`ledger/2026-09-20-a-child-that-is-still-a-child.md` closed with the
`RESTRICT`/partial-index interaction as unmeasured, dismissing it as *"unlikely
to be anybody's index"*. That was the wrong reason to leave it, and re-reading
it turned into a real hypothesis:

`referencing_rows` is a **planned** read — its own doc comment says so, because
being planned is what makes a cascade a range rather than a scan. The index a
soft-deleting table most wants is partial on `deleted_at IS NULL`, which by
construction cannot hold a retired row. If the planner chose it for the
`RESTRICT` search, the arm would look for the retired child through a structure
that cannot contain it, find nothing, and let the parent go — **the exact defect
fixed this morning, reintroduced through a different door**, and invisible to the
test that fixed it, whose schema has no index on the referencing column at all.

It does not happen, and the reason is worth having written down rather than
re-derived: the search now carries `include_deleted`, so `deleted_at IS NULL` is
not in the filter, so a partial index predicated on it is not implied by the
query and cannot be chosen. The same conjoin-before-planning that stops a
covering scan resurrecting a row is what stops a partial index hiding one here.
That is a *consequence* of the morning's fix that nothing was asserting.

### A table with no soft delete should not pay, or change

Every named-key write now consults `read_deleted` before deciding whether it
reaches a hidden row, and the overwhelming majority of tables have no hidden row
to reach. On those the answer cannot matter — `row_filter_with` builds the same
filter either way — so a caller lacking the grant had the grant list scanned on
every write for a decision with one possible outcome. `retired_rows_reachable`
now answers immediately when the table does not soft-delete.

It also keeps this file honest with itself. `read.rs` already refuses to demand
`read_deleted` for a no-op, saying *"demanding a grant for a no-op teaches
callers to ask for privileges they do not need"* — and that reads oddly beside
code that asks anyway.

## Alternatives rejected

**Leave the partial-index question as the caveat it was.** What I did this
morning, on the grounds that nobody indexes a referencing column partially. That
reasoning is wrong twice: `live_by_folder` — an index on the foreign key,
partial on "not retired" — is the *natural* index for a soft-deleting child, not
an exotic one; and "nobody would" is not a reason to leave a path that would
silently reopen a fixed defect.

**Assert the planner's choice directly, through `explain`.** It would say more
— "this plan is a scan, not an index range" — and it would pin an implementation
detail rather than the property that matters. The property is that the parent
delete is refused; if the planner grows a way to use that index safely, this
test should keep passing, and one that asserted "scan" would not.

**Skip the guard: one grant scan is nothing.** True per call, and the reason
against it is the same one that made the hoist worth doing an hour earlier — the
scan is on the path of every named-key write to every table, and "nothing"
repeated per write across a fleet is how a cost becomes visible with nobody able
to say when it arrived.

**Guard inside `SecurityCatalog::grants` instead**, so any caller benefits.
Rejected: `grants` answers "does this context hold this action", and a table's
soft-delete column is nothing to do with that question. The knowledge that
`ReadDeleted` is meaningless without a stamp belongs to the caller that cares.

## Evidence

**The partial index is genuinely the dangerous one.** Read out of the backend
rather than inferred, because the whole hypothesis rests on the index not
holding the row:

```
partial index entries: live=1 retired=0
```

**And the test fails when the morning's fix is reverted:**

```
### restrict arm reads Hidden again
test a_restrict_edge_blocks_on_a_retired_child ... FAILED
test a_restrict_edge_blocks_on_a_retired_child_the_only_index_cannot_see ... FAILED
```

Two, not one — so the new test is not merely a second copy of the old one: it
exercises a schema where an index exists and excludes the row, which the old
one cannot.

**One mutation of the guard is caught and one is equivalent**, and the
equivalence is proved from the code rather than asserted. `row_filter_with`:

```rust
let not_deleted = match (deleted, table.soft_delete()) {
    (Deleted::Visible, _) | (_, None) => Expr::True,
    ...
```

`(_, None) => Expr::True` — on a table with no soft delete, `Hidden` and
`Visible` build the same filter, so making the guard return `Hidden` changes
nothing and fails nothing. Making it swallow the soft-deleting case too fails
`without_read_deleted_a_retired_row_stays_out_of_reach` and its bulk twin.

**So the guard is unobservable, and the test beside it is not about the guard.**
`a_table_that_does_not_soft_delete_is_untouched_by_the_restore_grant` pins the
*behaviour* — a caller with no `read_deleted` writes such a table exactly as
before — and it is load-bearing two layers down: breaking `row_filter_with`'s
`(_, None)` arm into `(Deleted::Hidden, None) => Expr::False` fails it by name.
It is a characterisation test, and the thing it characterises is a match arm in
another file that nothing else pointed at.

**Suites**: `slate-kernel` 42 tests in `soft_delete` and the whole crate green,
`cargo clippy --workspace --all-targets` clean, `scripts/check.sh` 20/20.

## What this does not do

**It does not test a partial index on the `CASCADE` arm.** The cascade reads
with `Deleted::Hidden`, so a partial index on "not retired" *is* implied by its
filter and may well be chosen — which is correct there, since the cascade wants
only live children. Correct by the same reasoning that makes the restrict case
safe, and asserted by neither.

**It does not measure the guard.** No benchmark; the argument is that the work
cannot change an answer, which is the same argument as the hoist and is not a
performance claim.

**The equivalent mutation stays equivalent.** I did not contrive a test that
discriminates `Hidden` from `Visible` on a table with no soft-delete column,
because there is no observable difference to test — writing one would mean
asserting on the guard's implementation rather than on any behaviour.
