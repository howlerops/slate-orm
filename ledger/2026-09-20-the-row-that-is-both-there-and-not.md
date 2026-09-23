# The row that is both there and not

## What changed

No code. A defect, reproduced, and a claim in an earlier entry withdrawn.

**A soft-deleted row cannot be written by anybody, at any privilege, so it
cannot be restored.** It is simultaneously present — it blocks an insert at its
key — and absent — an upsert and an update at that same key are both refused as
missing. The two answers contradict each other.

## Why this is here rather than fixed

I set out to close the last item of
`ledger/2026-09-19-a-row-that-is-gone-but-still-there.md`, which says:

> There is no `restore` and no reaper. **Un-deleting is an ordinary update**,
> and nothing ever removes a retired row…

I generated a `restored()` on the row types — the write-side twin of the
`retired` accessor, clearing whichever column the catalog names — wired it into
the retention harness, ran it, and the write was refused. **Un-deleting is not
an ordinary update. An ordinary update does not work, and neither does anything
else.** That sentence is withdrawn.

The generated helper is reverted rather than shipped. A helper for an operation
the database cannot perform is worse than no helper: a caller would reach for
it, write the row back, and get `NotFound` from a row they are holding.

What is left is a design question I should not answer alone, below.

## Evidence

**The contradiction, on one key, one table, one caller.** Against a running
head node over a soft-deleting table, with the row retired by an ordinary
delete:

```
  insert(upsert=False) -> AlreadyExists: table `notes` already has a row with this primary key
  insert(upsert=True)  -> NotFound:      table `notes` has no row with this primary key
  update               -> NotFound:      table `notes` has no row with this primary key
  final: [(1, 'x', True)]                 # still retired, unchanged
```

**It is not a privilege problem.** Repeated against the demo catalog as `app`,
which holds `everything` — `read_deleted` included, the grant that lets the
same caller *see* the row:

```
8900 retired; caller holds `everything`, read_deleted included
  insert(upsert=True) -> NotFound: table `shipments` has no row with this primary key
  update              -> NotFound: table `shipments` has no row with this primary key
  8900 now: [(8900, 'pending', True)]
```

The caller can read the retired row through `include_deleted` in the same
session, and cannot write it.

**Where the two answers part company.** `RecordTransaction::write_many` reads
the row at each key, and that read is *not* soft-delete filtered — which is why
`BulkMode::Insert` finds it and raises `DuplicatePrimaryKey`. The `Upsert` and
`Update` arms then call `permits_row(context, table, Action::Update, current)`,
and `permits_row` goes through `row_filter`, which defaults to
`Deleted::Hidden`. The row is found and then judged invisible, so the arms
raise `RowNotFound`.

`security.rs` is explicit that these are different things, in a comment
directly above the code that conflates them here:

> A deleted row is not hidden for a security reason, so the escape hatch for
> security is not the escape hatch for this.

The write path takes the security answer for the soft-delete question.

## Alternatives rejected — and the one I did not take

**Fix it by evaluating the write path's row filter with `Deleted::Visible`.**
Contained: one match arm plus a `permits_row_with`. The row policy still
applies, so a caller the policy refuses is still refused, and `insert` already
discloses that the key is occupied, so nothing new leaks.

I did not do it, and the reason is not the size. **Whether restoring should
require `ReadDeleted` is a policy decision, not a bug fix.** Two defensible
answers:

- *Any caller who may update the table may restore.* Simple, and it means a
  soft delete can be undone by the same people who could have avoided it.
- *Restoring requires `ReadDeleted`, as reading a retired row does.* Consistent
  with `include_deleted` being privileged, and it keeps "who can resurrect
  history" a separate grant.

Picking one changes the security surface of every soft-deleting deployment.
That belongs to whoever owns the model, with the RLS matrix run against it —
not to a session that found the bug forty minutes ago.

**Document a workaround instead.** There is none. Purge-and-reinsert loses the
row's identity in every index built on it and is not a restore.

**Ship `restored()` anyway, with a caveat.** Rejected above.

## What this does not do

- **It does not fix the defect.** It is reproduced, located and explained, and
  the row is still unrestorable.
- **It does not establish how long this has been true.** Soft delete shipped
  yesterday; the write path predates it. The interaction is what is new, and I
  did not bisect.
- **It does not check the other write paths.** `delete_if_unchanged`,
  `update_where` and `delete_where` all reach rows through filters of their
  own, and whether each treats a retired row consistently with the others is
  unexamined. `update_where` is the one I would look at first: a predicate
  write that silently skips retired rows is defensible, and a predicate write
  that *errors* on them would be a second instance of this.
- **No test was added.** A test asserting the current behaviour would pin the
  defect; a test asserting the fixed behaviour would fail. The reproduction
  above is the record until the decision is made.
