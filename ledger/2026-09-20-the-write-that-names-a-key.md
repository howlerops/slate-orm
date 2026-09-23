# A retired row can be written again, by whoever may see it

- **Date:** 2026-09-20
- **Author:** Claude, closing the gap the previous entry reported and declined to fix
- **Touches:** `crates/slate-kernel/src/{record.rs,security.rs,error.rs}`, `crates/slate-server/src/status.rs`, `crates/slate-kernel/tests/soft_delete.rs`, `docs/orm-comparison.md`
- **Kind:** fix

## What changed

`update`, `upsert`, `update_many` and `upsert_many` reach a row a soft delete
retired, when the caller holds `read_deleted`. Restoring one is sending it back
with null in the soft-delete column — which is what this repository's
documentation claimed for months and could not back.

Three supporting pieces. `SecurityCatalog::permits_row_with` and
`RecordTransaction::visible_row_with` take a `Deleted`, so a write path can ask
the question a read path has always been able to ask. `check_row` now answers
"you supplied the soft-delete column" separately from "a policy hid this row",
with its own error and its own status code. And `RecordTransaction::
retired_rows_reachable` is the one place the grant is consulted, so there is one
answer rather than four.

## Why

`ledger/2026-09-20-the-row-that-is-both-there-and-not.md` recorded the defect
and stopped:

> **A soft-deleted row cannot be written by anybody, at any privilege, so it
> cannot be restored.** It is simultaneously present — it blocks an insert at
> its key — and absent — an upsert and an update at that same key are both
> refused as missing.

and said the remaining question was a policy one I should not answer alone.
Answering it turned out to need no judgement call after all, because this file
had already answered it — for rows hidden by *row-level security*, which are the
same shape. On `upsert`:

> Read without the policy: a hidden row still occupies the key, so treating it
> as absent would turn an upsert into a silent overwrite of a row the caller may
> not touch.

That is the design: **a hidden row occupies its key and is not writable.** The
apparent contradiction — `AlreadyExists` from an insert, `RowNotFound` from an
update — is the deliberate answer to a harder question, because the alternative
hands a caller the power to overwrite a row they cannot read.

The reason that was a *design* for policy-hidden rows and a *defect* for retired
ones is the whole finding: for a policy-hidden row there is always some caller
whose policy admits it, so the row is writable by somebody. For a retired row
there was nobody. `read_deleted` is the missing privilege, it already exists,
and `purge_deleted` already demands it with the reasoning that carries across
whole — "erasing a retired row means reading it first".

The rule the four paths now share, which is what is worth remembering:

**A write that names a primary key means the row at that key. A write that
matches a predicate means the live ones.**

That explains every path without a special case. `insert` still refuses the key,
because succeeding would overwrite the row the retention window exists to keep.
`delete` is untouched: re-deleting a retired row is a no-op rather than a
contradiction. `update_where` and `delete_where` are untouched: a predicate did
not name the row, which `2026-09-20-four-say-absent-one-says-present.md` had
already measured as correct.

The separated error is the gotcha rather than the feature, and it is the first
thing anyone restoring by hand runs into: read the row with `include_deleted`,
which hands back the retirement timestamp, edit a field, write it back. That
used to come out as `RowCheckFailed` — "row-level security forbids writing this
row" — on tables with no row-level security at all. It sends the reader to the
grants. It now names the column and says what to send instead, as
`SOFT_DELETE_COLUMN_SUPPLIED` with `InvalidArgument` rather than
`PermissionDenied`, because no grant fixes it.

## Alternatives rejected

**Ungated: any caller who may update the table may restore.** This is what I
built first, and the live probe is what argued me out of it. A role holding
`read/insert/update/delete` and *not* `read_deleted` restored a row it could not
see, and an `update` replaces the whole row — so that caller could destroy
retained content without ever being able to read it, which is the power
`purge_deleted` is careful to gate. The case for it was that `insert` already
discloses existence at plain write privilege, so the grant buys no
confidentiality. True, and beside the point: the grant is not buying
confidentiality, it is buying *integrity* of the retained row.

**Make all four paths say "absent", so an insert succeeds over a retired row.**
Coherent, and wrong in the direction that loses data: the retired row is
silently overwritten by a caller who never knew it was there, and `purge_deleted`
stops being the only thing that removes a retired row.

**A new action — `restore`, or `write_deleted`.** Rejected. It would need a
grant nobody has yet, a wire field, and a line in every deployment's config to
turn on a capability `read_deleted` already implies. `purge_deleted` sets the
precedent by reusing `read_deleted` rather than minting `purge`.

**A `restore(table, key)` verb on the wire.** Considered and rejected for the
same reason the previous entry reverted the generated `restored()` helper: the
operation *is* an update, and a second spelling of an update is a second write
path to test against policies, checks, foreign keys, unique slots and partial
indexes. The one thing a verb would buy — not having to supply the whole row —
is `update`'s cost, not restore's.

**Leave the misleading `RowCheckFailed` message.** Tempting, since it is only a
message. Rejected because it is the message on the path this change exists to
open, and a wrong signpost at the entrance to a new feature is worse than none.

## Evidence

**Through a real `slate-serverd` and the Python client**, one soft-deleting
table, row 1 retired:

```
state: [(1, 'one', 'retired')]   ordinary read: []

insert          -> AlreadyExists: table `notes` already has a row with this primary key
insert(upsert)  -> affected=1    state: [(1, 'via upsert', 'live')]   read: [1]
update          -> affected=1    state: [(1, 'via update', 'live')]   read: [1]

# the row as `include_deleted` handed it back, written straight back:
update [1, 'via update', 1789921695]
  -> InvalidRequest: invalid_argument: column `deleted_at` of table `notes` is its
     soft-delete column and is written by `delete`, not by a caller; send null to
     restore the row, or leave the row alone to keep it retired

# role `plain`: read/insert/update/delete, no read_deleted
plain: see it    -> []
plain: update it -> NotFound: table `notes` has no row with this primary key
     state: [(1, 'via update', 'retired')]   read: []
```

The last two lines are the gate, and they are the ones that changed between the
first version of this fix and the one committed.

**Ten mutations, ten named failures**, `cargo test -p slate-kernel --test
soft_delete` (37 tests):

| mutation | test that failed |
| --- | --- |
| single-row `update` reads `Hidden` | `an_update_at_a_retired_rows_key_restores_it` (+2) |
| single-row `upsert` reads `Hidden` | `an_upsert_over_a_retired_row_restores_it_rather_than_reporting_it_missing` |
| `write_many` arm reads `Hidden` | `the_bulk_writes_restore_a_retired_row_too` |
| `permits_row_with` ignores its argument, always hidden | four tests |
| `permits_row_with` ignores its argument, always visible | `deleting_a_row_twice_is_not_an_error_and_does_not_restamp` |
| `check_row` loses the soft-delete column guard | `a_write_that_supplies_the_soft_delete_column_names_that_column` |
| the grant is ignored, always reachable | `without_read_deleted_a_retired_row_stays_out_of_reach` |
| the grant is ignored, never reachable | six tests |
| the grant asked for is `Read`, not `ReadDeleted` | `without_read_deleted_a_retired_row_stays_out_of_reach` |
| `grants()` inverted | six tests |

**One mutation survived the first round** and is why the bulk tests exist at
all: making `write_many`'s arm read `Hidden` again broke nothing, because
nothing exercised the bulk path. Two tests were written rather than the
survival hidden, and the mutation now fails `the_bulk_writes_restore_a_retired_row_too`.

**One mutation is equivalent and is not counted above.** `check_row`'s policy
check evaluates with `Deleted::Visible`; changing it to `Hidden` fails nothing,
because the guard three lines above has already established that the
soft-delete column is null, so the conjunct has nothing left to decide. The
comment at that line says so rather than leaving the next reader to re-derive it.

**The daemon's own guard caught the new error variant** before any of this was
pushed: `every_kernel_error_has_a_code_and_a_reason` failed with
`["SoftDeleteColumnSupplied"]` — "these `KernelError` variants have no status
code and fall to the wildcard, where a caller reads them as the server having
broken". That test is the reason a new variant cannot quietly become an
`Internal` on the wire.

**Suites**: `slate-kernel`, `slate-orm`, `slate-schema`, `slate-derive`,
`slate-server` and `slate-serverd` all pass. `cargo clippy --workspace
--all-targets` clean, `cargo fmt` applied to the two crates touched.

## What this does not do

**No client helper.** The `restored()` row-type helper that the previous entry
reverted is still reverted. The operation it wraps now works, so the reason for
reverting it is gone, but regenerating it across Python, Go and TypeScript is
its own change with its own tests and does not belong in the commit that makes
the operation possible. Until then, restoring from a client is: read with
`include_deleted`, replace the soft-delete column with null, `update`.

**The three clients have no test for this.** The probe above went through a real
server and the Python client, which is evidence the wire path works, not a
regression test that it keeps working. The conformance runner does not exercise
it in any of the three SDKs.

**A superuser restores without holding anything.** `authorize` short-circuits
for a superuser, so `retired_rows_reachable` answers `Visible`. That is
deliberate — a seeder or migration runner upserting at a retired key should
restore rather than fail — but it is a different rule from reads, where the
soft-delete conjunct is applied *before* the superuser bypass precisely so a
superuser does not see retired rows by accident. A named-key write is not an
accident; a scan can be. Nothing tests the superuser-restore case on its own
because every other test in the file uses `root()`.

**It does not revisit `delete_if_unchanged`.** A conditional delete at a retired
row's key still reports `NotFound`, which matches plain `delete` treating it as
already gone and matches that call's own documented meaning. That is consistent
under the naming-a-key rule only if you accept that "delete" and "already
deleted" are not a contradiction, which is the same argument plain `delete`
rests on. It is not measured against the alternative because there is no
alternative anyone has asked for.

**Restoring can still fail, for a reason nothing warns about.** Retiring a row
frees its slot in a partial unique index on `deleted_at IS NULL` — the documented
point of that shape — so if somebody took the slug, the email address or the
external id in between, the restore is refused with `UniqueViolation`.
`restoring_a_row_whose_unique_slot_was_reused_is_refused` pins the behaviour; no
API tells a caller in advance that a particular row is restorable.
