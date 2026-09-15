# The daemon reconciles its schema before it opens the socket

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-serverd/src/main.rs`, `crates/slate-serverd/src/config.rs`, `crates/slate-kernel/src/migrate.rs`, `crates/slate-serverd/tests/migrations.rs` (new)
- **Kind:** feature

## What changed

`slate-serverd` runs the migration from the previous commit before it binds its
listener. A new `[schema] migrate_on_start` controls it, defaulting to **true**.
With it off the node *verifies* instead and refuses to start on anything
outstanding. A read-only node, which has no writer and cannot migrate, checks a
replica and warns.

`migrate::{stored_state, plan, verify}` grew snapshot-taking twins —
`stored_state_of`, `plan_of`, `verify_of` — because reading the state needs only
`get`, and a read replica has a `KvSnapshot` and no way to open a transaction.

## Why

The previous commit's own entry named this as its largest gap, in those words:
*"`verify` is not called by anything yet. It is the guard and nothing invokes
it."* A guard nothing calls is the repository's recurring failure — the CI
workflow that was marked active and had run zero times, the Python suite that
skipped itself and read as green. A migration runner that has to be invoked by
hand protects exactly the deployments whose operators already knew.

**Before the socket, not after.** A node that binds and then migrates is a node
that accepts a connection and answers it wrongly. That is the failure being
prevented, not a smaller version of it.

**On by default**, which is the opposite of `analyze_on_start` sitting beside it
in the same file. The reason they differ is the reason this commit exists:
statistics are an optimisation, and a node that skips them answers correctly and
more slowly. An unbuilt index is not — the query returns no rows. So here the
expensive case and the case you need are the *same case*, and defaulting to off
would make the safe configuration the one nobody writes down.

## Alternatives rejected

**Verify by default, migrate behind a flag.** What Django and Rails do, and the
first design here. It is right when a migration can be arbitrarily destructive;
it is wrong when the only migration is "write the index entries that should have
existed all along". Rejected for a concrete reason as well as a principled one:
every client harness, the conformance runner and the deployed example start this
binary against a fresh store whose tables declare indexes, so a verify-by-default
node would refuse to start in fourteen CI jobs and the fix in every one of them
would be to pass the flag. A default that every caller immediately overrides is
not a default, it is a papercut with a rationale.

**`migrate_on_start = false` meaning "skip it".** Rejected, and the distinction
is the whole value of the setting: off makes the node *verify and refuse*, so it
buys an operator control over **when** a large backfill runs and on which node —
never permission to serve without one. A mutation making `false` a plain skip is
caught by a test.

**Refusing to start on a read-only node.** It is the correct answer — a follower
serving reads through an unbuilt index returns the same empty results — and it
was rejected anyway. A follower started beside a leader can look before the
leader has finished migrating, and the failure mode is a restart loop on a node
that is about to become correct. It warns instead, and names both facts: that
reads through an unbuilt index return no rows, and that it fixes itself once the
writer migrates and the replica catches up. **The window is real and is recorded
below rather than argued away.**

**Putting the snapshot-taking twins behind a trait rather than duplicating three
signatures.** `KvStore` and `KvReadStore` are deliberately separate — a replica
lacks the write methods *at the type level*, which is the property that makes
replica routing safe — so a trait spanning them would exist only to let this
module pretend they are the same. Three four-line wrappers, each of which opens a
transaction and delegates, cost less than that.

## Evidence

Three process-level tests in `crates/slate-serverd/tests/migrations.rs`. They
start the real binary, over a `local` (file-backed) store, twice: once under a
schema with no index that writes two rows, then again under a schema that
declares one. `memory` would have been useless — it is a map in the process, so
every start is a fresh keyspace and the second start could not disagree with the
first.

The table is deliberately two columns wide, and the test says why: the index plus
the primary key then covers a filter on `kind`, so the planner picks an
index-only scan. That is what makes an unbuilt index return *nothing* rather than
merely cost more. A third column and the same query answers correctly off a full
scan, and the file would pass while testing nothing — which is exactly what
happened to the kernel suite's first draft.

Three mutations of the wiring, all caught after one fix:

| mutation | result |
| --- | --- |
| the reconcile step is removed entirely | caught (2 tests) |
| `migrate_on_start = false` becomes a plain skip | caught |
| the "building …" announcement is reworded | **survived** → assertion tightened |

The survivor is worth the line it cost. The test asserted that the startup output
named `by_kind`, and the index name was interpolated into the message from a list
— so any sentence at all containing it passed, including one an operator would
not recognise. It now asserts the verb as well, and the verb is the same word the
no-op test requires *not* to appear, so the pair cannot drift apart.

`slate-serverd` (128 + 39 + 18 + 3 + 3 + 2 tests), `slate-server` (14 binaries)
and `slate-kernel` are green; `cargo clippy --workspace --all-targets` is clean.

## What this does not do

**The read-only window is open.** A follower that starts while the leader has not
yet migrated warns and serves. During that window a query through an unbuilt
index returns no rows. Closing it properly means the follower waiting for the
leader, which is a coordination problem — it needs a way to distinguish "the
leader has not migrated yet" from "there is no leader" — and inventing one inside
a startup path is how you get a node that hangs instead of one that is briefly
wrong. Stated here so the next person finds the decision rather than the
symptom.

**The warning is not tested.** The two process tests cover the writer path. The
read-only path needs a second node, a shared backend and a lease contest, which
the `slate-server` suite has machinery for and this file does not. It is the
least-exercised branch in the change.

**Nothing measures a large backfill.** The startup message reports elapsed time,
and no test asserts anything about how long a real backfill takes, because none
of them build more than a handful of entries. `BACKFILL_BATCH` remains the
untuned guess the previous entry admitted to.

**The SDKs still cannot run or ask about a migration.** Unchanged from the
previous entry. A client cannot tell whether the node it is talking to is
migrated, which matters most for exactly the read-only window above.
