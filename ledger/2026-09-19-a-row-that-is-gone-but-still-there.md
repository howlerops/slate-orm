# A row that is gone but still there

## What changed

A table can declare `soft_delete = "deleted_at"`. `delete` stamps that column
with the current time and leaves the row in the keyspace; every read conjoins
`deleted_at IS NULL`, so it stops being visible. `Query::include_deleted` asks
for them back. Kernel, schema and the daemon's TOML; nothing on the wire.

## Why

The gap table said partial indexes "support `WHERE deleted_at IS NULL` well;
no convention on top", which was true and is the smaller half of the problem.
The convention is not the filter — anyone can write that filter. It is that
*every* read has to carry it, and a caller who forgets on one path gets a
resurrected row rather than an error.

Which is why the interesting decision here is not soft delete at all. It is
that the filter goes into `SecurityCatalog::row_filter`, the function
row-level security already uses. That path was proven to cover every access
path once, in the task that closed the RLS matrix, and it carries a property
soft delete needs and would not have thought to ask for: the filter is
conjoined *before* planning, so an index that does not carry `deleted_at`
cannot be chosen for an index-only scan and answer from keys alone. A covering
scan over a retired row's still-present index entry is the one failure mode
that would have been silent, fast, and wrong.

The rest follows from placement. `delete`, `delete_where` and the cascade walk
all funnel through `remove_row`, so putting the decision there makes a cascade
into a soft-deleting child retire the child; and because a retired row is
invisible to `permits_row`, deleting one twice is a no-op rather than a second
stamp that moves the time of death.

## Alternatives rejected

**A policy: `using = "deleted_at is null"`.** It would work today with no code
at all, and it is wrong in two ways that matter. A policy only applies when
RLS is enabled for the table, so a table without it would filter nothing; and
`row_filter` returns early for a superuser, so the seeder, the migration runner
and `--seed` would all see deleted rows. A deleted row is not hidden for a
security reason, so it should not inherit security's exemptions. The conjunct
goes in before the bypass.

**A second filter mechanism, applied in the executor.** The obvious
alternative, and it means proving the matrix again — joins, chains, aggregates,
point gets, covering scans, keyset pagination. The RLS path had a whole task
spent closing it. Reusing it costs one parameter.

**Reuse `managed`, as `managed = "deleted_at"`.** Tempting, since the machinery
for "a timestamp the store writes" was built yesterday. It inverts the rule
that matters: a managed column must be non-nullable, and a soft-delete column
must be nullable, because null is what "not deleted" means. Overloading the
field would have made the nullability rule conditional on the variant, which is
the kind of exception that gets missed. It is also not the same *kind* of
thing: managed is a column-stamping rule, soft delete changes what `delete`
means and what reads see.

**A boolean `deleted` rather than a timestamp.** Smaller and answers less. The
question after "is it deleted" is always "when", and a boolean beside a
timestamp is two fields that can disagree.

**Putting `include_deleted` on the wire.** Deliberately not done. "Show me the
deleted ones" is a privileged read and the protocol has no way to express who
may make it; shipping the flag first would put that decision in the caller's
hands and be hard to take back. Remote callers get the safe half — they never
see retired rows — and restoring or reaping runs against the kernel.

## Evidence

Twelve tests in `crates/slate-kernel/tests/soft_delete.rs`. Seven mutations
were run; five were caught first time and **two survived**, both of which were
real:

| mutation | outcome |
| --- | --- |
| superuser bypass skips the filter | caught |
| delete removes instead of stamping | caught |
| an aggregate drops `include_deleted` | caught |
| a scan ignores `include_deleted` | caught |
| a non-nullable soft-delete column is accepted | caught |
| **the filter is never conjoined for a non-superuser** | **survived** |
| **a soft-delete column in the key is accepted** | **survived** |

The first survivor was a missing test, and an embarrassing one: every test in
the file used `SecurityContext::superuser()`, which returns from `row_filter`
early — so the conjunction onto the tenant-and-policy filter, the path almost
every real read takes, had no coverage at all.
`an_ordinary_caller_does_not_see_a_deleted_row` now covers it and fails when
the mutation is reapplied.

The second survivor was **dead code**, and the check is gone rather than
tested. A soft-delete column must be nullable; a primary key column may not be
nullable; `NullablePrimaryKey` already refuses the combination. No input could
reach `SoftDeleteInKey`, so it and its error variant were deleted and the test
now asserts the case stays refused by whatever rule does it. A refusal with a
carefully argued comment that can never fire is worse than no refusal, because
it reads as a rule somebody verified.

The two tests worth naming: `an_index_only_scan_does_not_answer_from_keys_and_
resurrect_a_row`, which is the silent failure mode, and
`a_partial_index_on_not_deleted_frees_its_entry_when_a_row_is_retired`, which
is the gap table's own claim about partial indexes, run for the first time.

## What this does not do

It does not cross the wire. No client can ask for deleted rows and no client
can restore one; `scripts/codegen.py` does not know the column is special,
because `--print-schema` does not publish `soft_delete`.

There is no `restore` and no reaper. Un-deleting is an ordinary update, and
nothing ever removes a retired row, so a soft-deleting table grows without
bound. A reaper is a `delete_where` on a table temporarily declared without
`soft_delete`, which works and is not a feature.

~~`#[derive(Record)]` has no attribute for it, so the Rust library declares it
through the builder only.~~ **Closed** — see
`2026-09-20-the-attribute-the-builder-already-had.md`.

Nothing measures the cost. Every read of a soft-deleting table carries one
extra `IsNull` conjunct, and on a table with no partial index the retired rows
stay in every index and are filtered after the fetch. Both are obviously
non-zero and neither was benchmarked.
