# Look before the restart

## What changed

`slate-serverd --plan` prints the schema migration a restart would apply, and
applies nothing. It exits 1 when the migration is blocked, so a deployment can
gate on it.

## Why

`migrate::plan` has been a separate function from `migrate::apply` since the
runner was written, and `MigrationPlan`'s own doc comment says why: *"Separate
from applying it so a deployment can look first."* Nothing shipped let a
deployment look. The separation was built, documented, and reachable only from
tests — a capability with no way to use it, which is the same shape as the
workflow in `CLAUDE.md` that existed, was marked active, and had run zero times.

What that costs today is specific, and smaller than it first looks. Row data is
never at risk: a layout change is *refused*, not applied, and the four steps a
plan can hold are register, build an index, drop a dead index's entries, and
note a version. But `migrate_on_start` defaults to true, so the first anyone
learns of a refusal is a node that will not boot — during the deploy, not
before it. And a build over a large table is the one step that takes minutes,
which is a deploy-window question nobody could answer in advance.

The running node announces `BuildIndex` and nothing else, on the reasoning that
a backfill is the one step slow enough to look like a hang. That is right for a
node that is already committed to applying the plan and wrong for a preview,
which has no excuse to be incomplete, so `--plan` names every step.

## Alternatives rejected

**Extend `--check`.** The obvious move, and it breaks the one property that
makes `--check` worth having: it opens no storage and binds nothing, so it is
safe anywhere, including a CI box with no credentials. A plan is a function of
what is *stored*. Folding them would mean `--check` sometimes needs a bucket
and sometimes does not, decided by a flag — so they are separate, and
`conflicts_with_all` refuses both at once rather than letting one silently win.

**Write migration files to disk, as Drizzle Kit and Alembic do.** That shape
belongs to a system where the database is the source of truth and the code has
to catch up. Here the catalog is the source of truth and the diff is computed
live against it, so a checked-in migration file would be a second, staler
account of something already derivable. The reviewable artifact those tools
actually provide is *the diff, before it runs* — which is this.

**Open the writer store and call `plan`.** Simplest code, and it would fence
the live node: opening a SlateDB writer takes the writer role from whoever
holds it. A flag whose purpose is safe inspection would have been the most
dangerous thing in the binary. It opens a `SlateReader` instead, which is the
same mechanism a read replica uses, and it never campaigns.

**Reuse `open_read_only`.** It refuses a node with no `[[replicas]]`
configured, which is correct for a follower that would otherwise serve nothing,
and irrelevant to whether schema state can be read. Reusing it would have made
`--plan` unavailable in the common single-node case.

**Exit non-zero on any pending migration**, not just a blocked one. Attractive
for a pipeline that wants "nothing should change", and wrong as a default: a
pending index build is the normal state of a deploy that adds an index, and a
preview that fails on the expected case gets wrapped in `|| true` and stops
being read.

**Let a missing manifest be an error.** What the first version did, and the
first thing tried against a real config: a keyspace that does not exist yet
fails inside SlateDB with "failed to find latest transactional object (e.g.
manifest)". That is the most likely state the first time anybody runs the flag,
and it reads as broken storage rather than an empty bucket. `StorageError` is
an opaque box so there is no variant to match, and matching the message would
pin this to a string in another crate's error path — so it asks the object
store whether anything is there, which also keeps "unreachable" (credentials, a
wrong bucket) apart from "empty".

## Evidence

Seven tests in `crates/slate-serverd/tests/plan.rs`, and seven mutations, all
caught on the first pass:

| mutation | caught by |
| --- | --- |
| never report a fresh keyspace | `a_keyspace_that_does_not_exist_yet_…` |
| a blocked plan exits zero | `a_layout_change_under_stored_rows_…` |
| plan against memory as if stored | `a_memory_backend_says_it_has_no_stored_state_…` |
| **open a writer while previewing** | `previewing_does_not_fence_the_node_that_holds_the_lease` |
| drop the backfill cost from the output | `an_index_added_since_the_rows_were_written_…` |
| let `--plan` and `--check` combine | `plan_and_check_cannot_be_asked_for_together` |
| announce only index builds | `a_keyspace_that_does_not_exist_yet_…` |

The fence test writes through the leader *after* the preview rather than
reading, following `a_second_node_over_one_local_database_…`: `handover.rs`
shows a fenced SlateDB can still be read from, so a read would have passed
against a fenced node and proved nothing.

Run by hand against a local keyspace in all four states — never created,
up to date, one index pending, layout changed — before any test was written,
which is how the missing-manifest case was found.

## What this does not do

It does not show how long a backfill would take, only that one is coming: the
plan holds no row count and the step does not estimate. A table's statistics
could answer it and this does not ask them.

It does not cover `backend = "memory"`, which has no state a separate
invocation can read; the flag says so rather than printing a plan.

~~It is not wired into any deployment in this repository — `examples/deployed`
still restarts and lets the node reconcile. Nothing here proves the exit code
is usable as a gate beyond the test that asserts it.~~ **Closed** — see
`2026-09-20-the-exit-code-a-deployment-now-uses.md`. The deployed harness
previews before its restart and fails on anything but an empty plan, which also
makes it the only place `--plan` meets object storage rather than a temp dir.

`DropIndex` prints an index id and not a name, because the index is by
definition one the current catalog no longer declares, so there is nothing to
look the name up in. A reader has to match the number against the previous
configuration.

> **Checked, and the reason is stronger than "the current catalog".** The
> stored side does not have the name either: `TableState::built` is a
> `Vec<IndexId>`, persisted as four bytes per id, so the keyspace has never
> recorded what an index was called. Printing a name would mean changing that
> encoding and migrating the migration state, which is disproportionate to the
> annoyance of matching a number against version control. Not a gap left open
> by inattention — a consequence of what is stored.
