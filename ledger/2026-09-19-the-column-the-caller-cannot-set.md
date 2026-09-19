# Timestamps the store writes, and the clock that makes them testable

- **Date:** 2026-09-19
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-kernel/src/{clock.rs,record.rs,lib.rs}`, `crates/slate-schema/src/{table.rs,row.rs,error.rs,lib.rs}`, `crates/slate-derive/src/lib.rs`, `crates/slate-orm/src/lib.rs`, `crates/slate-serverd/src/{config.rs,schema.rs,main.rs}`, `crates/slate-kernel/tests/managed.rs`, `crates/slate-orm/tests/timestamps.rs`, `site/docs/features.html`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

A column can be declared **managed**, and then the store writes it:

```rust
#[record(created_at)] created_at: i64,
#[record(updated_at)] updated_at: i64,
```

```toml
{ name = "created_at", type = "i64", managed = "created_at" }
```

Both are filled on insert. On every write after that `updated_at` moves and
`created_at` holds. The value the caller supplies is discarded.

Nothing crosses the wire and no client changed: the server stamps, and a client
sends whatever it likes into a slot that is overwritten.

## Why

Every ORM in the comparison has this, and the gap table said "nothing in the
derive macro or the kernel". `DEFAULT` cannot express it — a default is a
stored `Value` and the value wanted is "whatever the clock says at the moment
of the write".

## Alternatives rejected

**Widen `DEFAULT` to hold an expression.** The obvious move, and it means a
second expression language in the schema layer, evaluated on a write path that
evaluates nothing today, to express two cases. A `Managed` enum with two
variants is the whole feature.

**Honour a value the caller supplied, filling only where they left null.** This
is what makes importing rows with their original timestamps possible, and it is
the wrong default. The column's entire promise is that it says when the row was
written; a client that can set it breaks that promise with nothing downstream
able to tell a real timestamp from a claimed one. An import declares the column
unmanaged — one word in a schema, against a hazard on every write of every
managed column.

**Stamp in the five write entry points.** Rejected for one choke point:
`write_row_with` is where `insert`, `insert_partial`, `upsert`, `update`,
`update_if_unchanged`, `update_where` and the bulk path all converge, and it
already takes `previous`, which *is* the insert/update distinction `CreatedAt`
needs — the bulk path derives its `Action` from exactly that. Five call sites
would be five chances to forget, and the sixth added later would be the one
that forgot.

**Preserve `created_at` by leaving the caller's copy alone.** Not the same
thing, and it is the subtle one: an update carries a *full* row, so "leave it
alone" means taking a field the caller could have edited. It is read from the
stored row instead, which costs nothing because `update` already read it.

**Milliseconds.** More useful for ordering, and wrong here: every time in this
system is `i64` seconds — `date_trunc`, `CalendarPart`, `Round`, the timezone
tables, every example — so a managed column in milliseconds reads back a
thousand times too large from all of them with nothing reporting it. The cost
of seconds is stated rather than hidden: two writes in the same second share an
`updated_at`, so it cannot order writes within a second or act as a concurrency
token. `update_if_unchanged` is that, and it compares the whole row.

**`SystemTime::now()` in the write path.** Three lines, and it makes every test
of this feature a test of the machine's clock. "Two numbers near now" passes on
an implementation that stamps both on every write, on one that stamps neither
after the first, and on one that takes the caller's value — all three produce
plausible numbers. A settable clock makes the assertion `[1000, 2000]`, which
separates them. `RecordStore::with_clock` follows the existing `with_limits`
and `with_statistics` builders, so no construction site changed.

**Put it in the schema fingerprint.** Rejected on the rule the fingerprint
already follows rather than as an exception to it: a client that disagrees
still reaches the right column, and the disagreement is *visible* — it reads
back a value it did not write, on the first row. That is the test a `CHECK` and
a `DEFAULT` pass and a decimal's scale fails, which is why the scale is hashed
and these are not. It also means the wire, the three clients and the generated
declarations are untouched.

## Evidence

**Seven mutations, all killed on the first pass:**

| # | mutation | outcome |
|---|---|---|
| M1 | `created_at` is restamped on every write | killed — three tests |
| M2 | `created_at` is taken from the caller's row, not the stored one | killed |
| M3 | `updated_at` is preserved instead of restamped | killed |
| M4 | nothing is stamped at all | killed |
| M5 | the per-table opt-in is ignored, so every `i64` is stamped | killed |
| M6 | the schema accepts a managed column in the primary key | killed |
| M7 | an unset managed column becomes a null again | killed |

M2 is the one the design is shaped around, and it is why the clock is settable:
against a wall clock, "taken from the caller" and "preserved from the store"
both produce a number near the insert time, and only an exact `1000` separates
them.

M5 covers the other half of the opt-in. Every existing table in this repository
has no managed column, and a stamp that fired on all of them would overwrite
real data in any column that merely happened to be an `i64` — `books.released`,
`books.year`, every seeded timestamp.

**Three schema refusals**, all at build time: a managed column that is not an
`i64`, one that is nullable, one that is in the primary key. At build rather
than at write because a schema that cannot be served should not start a node,
and the write-time version of each is a surprise on somebody's first insert.
The nullable one is refused rather than tolerated — it would be harmless, since
the store always writes a value, and it would be a lie in the schema that sends
a reader down a branch that can never run.

**A gap the tests found.** `insert_partial` is the entry point that knows a
column was not supplied, and an unset managed column became a null, which
`validate` refused — so a caller using the one API that should let them ignore
these columns had to name them. `PartialRow::into_row` fills a managed column
with a placeholder now, which the store replaces before the write.

`cargo test -p slate-kernel -p slate-schema -p slate-orm --no-fail-fast`,
`cargo test --workspace`, `python3 site/check/docs.py`: green.

## What this does not do

- **No client can express it.** Nothing needs to — the server stamps — but a
  client reading `--print-schema` can now *see* it, because the dump carries
  `managed`. A generated declaration ignores it, correctly.
- **Nothing backfills.** Declaring a column managed on a table that already has
  rows leaves those rows as they were until each is next written. There is no
  migration that stamps them, and inventing a creation time for a row whose
  real one is unknown would be worse than leaving it.
- **No soft delete, and no other hooks.** `deleted_at` is the obvious next
  managed column and is a different feature: it needs a default filter on every
  read, not a value on every write. Validations and lifecycle callbacks are
  their own gap-table row and are untouched.
- **The clock is per store, not per request.** Two nodes writing the same table
  stamp from their own clocks, so a skewed pair produces `updated_at` values
  that do not order across them. That is true of every distributed timestamp
  and is not fixed here.
