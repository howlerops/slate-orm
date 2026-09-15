# A migration runner, because adding an index made queries wrong

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-kernel/src/migrate.rs` (new), `crates/slate-kernel/src/keys.rs`, `crates/slate-kernel/src/error.rs`, `crates/slate-kernel/src/lib.rs`, `crates/slate-kernel/tests/migrations.rs` (new), `crates/slate-orm/src/lib.rs`, `crates/slate-wasm/src/lib.rs`
- **Kind:** defect + feature

## What changed

A third keyspace, `0x03 <table id>`, holding one record per table: the schema
version it was last migrated at, a fingerprint of its column layout, and which
indexes have actually been built. `migrate::{plan, apply, migrate, verify}`
compare that against the code's catalog, back-fill an index that has no entries,
delete the entries of one the schema has dropped, and refuse a layout change
that would read stored rows as something they are not.

## Why

The task on the list was "migrations have no runner". What it turned out to be
was a correctness bug, measured before anything was written:

```
three rows, filter on an indexed column, index added after the rows were written

rows found through the new index = 0
rows found by a scan             = 1
```

Adding an index to a table that already holds rows does not make queries slower.
It makes them **return nothing**. The planner sees an index whose columns cover
the predicate, costs it as the cheap option, scans a key range that no write ever
wrote into, and returns an empty result. Every layer behaves correctly and the
answer is wrong — no error, no warning, and the rows are still on disk.

**It is also intermittent, which is worse.** Reproducing it needed a table two
columns wide, where the index plus the primary key covers the query and an
index-only scan is the cheapest plan. The first draft of the test used a
three-column table and *failed* — a full scan won on cost and the answer came
back right. So whether an unmigrated deploy serves correct answers or empty ones
depends on the cost model, the table's width and how many rows it holds. That
discovery is in the test as a comment, because it is the reason the fix is a
refusal at startup rather than a note about when it matters.

Nothing in the stored bytes could have caught this: a row carries no record of
which indexes existed when it was written. The fix has to be something written
down on purpose.

## Alternatives rejected

**A per-query guard.** Check, on each read, that the index the plan chose is
built. It is where the damage happens, so it looks like the right place.
Rejected: it means reading the state key on every query, or caching it and being
wrong immediately after a migration, to defend against a condition that can only
be introduced by *starting a new binary*. `verify` says the same thing once,
before the first wrong answer is served, and costs one point get per table at
boot. The cost of the choice is real and is recorded below: a process that skips
`verify` gets no protection at all.

**Storing the schema instead of a fingerprint.** A stored copy of every column
would let the refusal say "the type of `price` changed from `i64` to `f64`",
which is a materially better message. Rejected because it puts the schema format
on disk in a *second* encoding that has to keep step with the first for ever —
and a migration runner whose own record can drift from the thing it describes is
worse than a worse error message. The fingerprint is eight bytes and is computed
from the table, so it cannot drift. The cost is paid down by having the refusal
name the specific things the fingerprint covers.

**Fingerprinting names.** Rejected, and this is load-bearing: a name appears
nowhere on disk, which is exactly why `renamed_column` is free. If names were in
the fingerprint, a rename would be indistinguishable from a type change and the
cheapest schema operation in the system would become the most alarming. There is
a test asserting a rename asks for no work, and a mutation adding names to the
fingerprint fails it.

**Rewriting rows on a column change.** The whole schema layer is deliberately
rewrite-free — a new column with a `DEFAULT` is supplied by the decoder, a
dropped column keeps its ordinal for ever, a rename moves nothing. Building an
index is the one operation that cannot be done by reading differently, because
an index entry is a key that has to exist. So the runner has exactly one step
that costs anything, and adding a rewriting step would undo the property the rest
of the design is built on.

**Applying a plan up to its first refusal.** Rejected: half a migration is a
state nobody designed, and the operator who has to reason about it has strictly
less information than the one who was told no. `apply` refuses the whole plan.

**Deleting a dropped index's entries lazily, as rows are written.** Rejected
because the write path only deletes entries for indexes it can *see*, and an
index removed from the schema is invisible to it — the keys would outlive every
row they described. It is not a correctness problem (nothing reads them) which is
why it is a step rather than a refusal.

## Evidence

Fifteen tests in `crates/slate-kernel/tests/migrations.rs`. The first is the
measurement above, kept as a test so that if the defect is ever fixed elsewhere
this fails loudly rather than passing quietly.

Several assertions are made **against the keyspace** rather than against a
query, through a helper that counts the keys under an index's prefix. That is
deliberate and it is the lesson of the first draft: whether a query finds a row
depends on which plan the cost model picked, which changes with the width of the
table. The number of index entries does not depend on anything but the backfill.

The batching is exercised for real: 2,500 rows against a batch size of 1,000, so
the resume path runs twice. It asserts the exact entry count rather than "more
than one", because a resume that restarted from the beginning would never
terminate and one that skipped a key at each boundary would be short by exactly
the number of batches — both of which a loose assertion would miss. Changing the
resume bound from exclusive to inclusive hangs the suite, which is how it is
confirmed to be doing something.

`a_migration_with_nothing_to_do_writes_nothing_at_all` wraps the store in a
counter, because "the report has no steps" and "nothing was written" are
different claims and only the second is what a deploy wants. The test also
asserts that a migration *with* work to do does write, so a counter that stopped
counting fails rather than passing everything.

The full `slate-kernel` suite is green (48 test binaries); `cargo clippy` on
`slate-kernel`, `slate-orm` and `slate-wasm` is clean.

### The mutation pass

Fifteen mutations, **three survivors**, two of which were real missing tests:

| mutation | result |
| --- | --- |
| no uniqueness check during a backfill | caught |
| backfill ignores a partial index's predicate | caught |
| fingerprint ignores nullability | caught |
| fingerprint ignores column type | caught |
| fingerprint covers names | caught (by the rename test) |
| a layout change is not refused | caught |
| `apply` runs a blocked plan | caught |
| the built-index list is ignored, so indexes rebuild every start | caught |
| `verify` ignores unbuilt indexes | caught |
| a dropped index is not noticed | caught |
| the backfill resume bound is inclusive | caught (the suite hangs) |
| an unknown state format is accepted | caught |
| fingerprint ignores the primary key's *ordinals* | **survived** → new test |
| state is written for every table, not only changed ones | **survived** → new test |
| the tenant `None`/`Some` tag is removed | **survived** — see below |

The first survivor is a good example of a test that was nearly right: the
fingerprint test changed the primary key from `["id"]` to `["id", "email"]`,
which changes its *length*, and the length was still hashed. A key of the same
length over a different column was not tested, so a fingerprint that counted key
columns without looking at which ones passed. Now both cases are there.

The second is the difference between "no steps" and "no writes", above.

**The third survivor is not a missing test, and it is recorded rather than
manufactured away.** The tenant column is hashed as a `0`/`1` tag followed by the
ordinal; removing the tag changes nothing observable, because the ordinal is
always written as eight bytes and so `Some(Ordinal(0))` can never hash like
`None` anyway. The tag is kept deliberately as a guard against `number` ever
becoming a varint, at which point it would matter — and the comment in the source
now says exactly that instead of claiming the tag is what separates the two
cases. A separate mutation removing the tenant from the fingerprint *entirely* is
caught, so the tenant is covered; it is only the redundant tag that is not.

## What this does not do

**`verify` is not called by anything yet.** It is the guard and nothing invokes
it: `slate-serverd` does not run it at startup, and neither does the deployed
example. Wiring it in is a change to how the daemon boots, with its own failure
mode (a daemon that will not start), and belongs in its own commit with its own
test. Until then the runner is opt-in and the defect is reachable by anyone who
does not call it.

**The SDKs cannot run a migration.** No wire message, no Python/Go/TypeScript
surface. A migration is a writer-side operation and the head node is the writer,
so the shape is probably an admin RPC rather than a client call — which is a
protocol decision, not a client one.

**Nothing in the browser writes a schema-state key.** The wasm keyspace viewer
now labels space `0x03` as `schema` instead of calling it an index and trying to
decode it as one, but the workbench never runs a migration, so that branch is
unexercised. It is a wrong label removed, not a tested path — said plainly
because a check that never fires is a check nobody has debugged, and this is one.

**An additive column change is refused, not applied.** Appending a nullable
column with a `DEFAULT` is genuinely safe and the fingerprint still refuses it,
because the runner cannot tell an appended column from a retyped one and the safe
answer to "I cannot tell" is no. This is the sharpest limitation of the
fingerprint-instead-of-schema choice and there is a test pinning the current
behaviour so that it is a decision rather than a surprise. Fixing it properly
means storing enough of the layout to diff it, which is the alternative rejected
above.

**A backfill is not concurrent-safe against writers.** It commits in batches, and
a row written into a range the backfill has already passed gets its index entry
from the write path — which is correct. A row *deleted* from a range it has not
reached yet is also fine. What is not analysed is a unique index being built
while a writer inserts a colliding row; the backfill's check and the write path's
check are separate reads. Not tested, not claimed.

**`BACKFILL_BATCH` is a guess.** 1,000 rows, not tuned, deliberately: the step is
once-per-deploy and bounded by the table, so the difference between 1,000 and
10,000 is not worth the measurement it would take to justify. Said here so the
number is not mistaken for a result.
