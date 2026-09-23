# The keyspace remembers a table's layout, so a refusal can name the column

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `crates/slate-kernel/src/migrate.rs`, `crates/slate-kernel/tests/migrations.rs`, `crates/slate-serverd/src/main.rs`, `scripts/codegen.py`, four generated files, `docs/persisting-the-schema.md`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`TableState` carries a `StoredSchema`: per column a name, type code, nullable,
dropped, scale and element code, plus the primary key ordinals and the tenant
column. Records are written as `STATE_FORMAT_V2` and read as either version; a
record with no schema is upgraded by a new, visible `Step::RecordSchema`. A
layout refusal now says

> `users`: the column layout changed under rows that are already stored;
> column 1 `email` was string and is now u64.

where it used to print two hex numbers and a list of six things one of which
had moved.

## Why

`docs/persisting-the-schema.md` designed all of this and argued the sequencing:
the first deliverable is the better refusal, not a generator, because it
exercises every part of the persistence on the path where being wrong is
cheapest. That is what this is. It also unblocks the comparison table's
generated-migrations row, which was blocked on one sentence — *a fingerprint is
one-way by construction; you cannot generate a catalog from it.*

## Alternatives rejected

**A flag day: bump the only format version.** `decode_state` refuses a format
it does not know, so every existing table would become unreadable and every
deployment a dump and reload. Read both, write the new one.

**A second key beside the state record.** Both would be written inside the
migration's transaction, so atomicity is not the argument. Two records are two
things that can disagree, and the disagreement would be silent.

**Storing more than the fingerprint's inputs.** A `CHECK`, a `DEFAULT` or a
foreign key is deliberately *not* hashed, because hashing them would make an
unrelated schema change break every client. Storing them would let the diff
report a change that is **not a migration** — announced at startup, in a
refusal path, about something needing no action. The tight version buys a
property instead: every difference the fingerprint detects is one the stored
schema can name, and conversely.

**Storing less.** Anything omitted is a difference the fingerprint detects and
the stored schema cannot explain, which is exactly today's message.

**Hashing the names.** A rename moves no bytes and must not look like a
migration, so the name is stored and not hashed. That asymmetry is the whole
point of storing rather than widening the hash: a diff that cannot say `email`
is a diff that says "column 3".

**Leaning on the outer format byte for the nested blob.** The design note left
this "probably yes"; it is yes. The outer format says how the record is laid
out and the inner says what a column record holds, and those change for
different reasons — a per-column property added later would otherwise need
`STATE_FORMAT_V3` and a third branch in `decode_state`.

**Decoding by trying one shape and falling back.** `decode_prefix` reads the
four version-1 values and lets the *format* decide how much more to expect.
Guessing would report a version-2 parse error for a corrupt version-1 record,
which is a worse message for the more likely failure.

## Evidence

22 tests in `crates/slate-kernel/tests/migrations.rs`, all passing; the whole
`slate-kernel` suite is 56 binaries green, `slate-serverd` is 8, and
`sh scripts/check.sh` is 32/32.

Ten mutations through `scripts/mutate.py`, **all ten caught**:

| mutation | caught by |
| --- | --- |
| every record is written as version 1 | four tests |
| a version 2 record ignores its schema blob | four tests |
| the column name is not stored | three tests |
| the diff ignores a column's type | four tests |
| the diff reports a rename as a layout change | four tests |
| the diff ignores the primary key | `every_layout_change_moves_the_fingerprint` |
| the diff ignores the tenant column | the same |
| a record with no schema is never upgraded | `a_record_with_no_stored_schema_falls_back_to_the_hash` |
| a version 1 record with trailing bytes is accepted | `a_version_one_record_with_more_after_it_is_refused` |
| the blob's own version is not checked | `a_schema_blob_from_a_newer_binary_is_refused_rather_than_misread` |
| trailing bytes after the blob are accepted | the same |

Three of those started as survivors and are the reason three tests exist.

`every_layout_change_moves_the_fingerprint` is the property the design note
asked for as a test rather than a hope, and it runs in both directions over one
list: eight catalogs, each asserted to move the fingerprint if and only if
`layout_changes` reports something. A rename is in the list with the answer
"same", which is the one difference the stored schema holds and deliberately
does not report.

**The design note was wrong about the lazy upgrade, and a test caught it.** It
said a record is rewritten "the next time it is migrated, which is already a
transaction that writes the record" — true of a table that has something to do,
and false of every other one, because `apply` skips a table with no steps. A
table nobody ever changes would have kept a version 1 record for ever and kept
refusing with two hex numbers, which is the entire defect this change is
against. `Step::RecordSchema` fixes it, and `--plan` shows it rather than doing
it quietly.

**The compiler found a site the note said did not exist.** The note checked
that `TableState { … }` is built in exactly two places, both in `migrate.rs`,
and concluded a new field breaks those two "and nothing else". Right about the
field and wrong about the change: `Step` is a public enum matched exhaustively
by `slate-serverd`'s `--plan` renderer, so the new *variant* broke a third
site. That is the right outcome — it is why neither enum is
`#[non_exhaustive]` — and it is a reminder that "what does this struct break"
and "what does this change break" are different questions.

**Two CI failures from earlier commits were fixed here, and one was a real
generator defect.** CI reported `examples/retention/schema.py` as drifting from
its catalog, which looked like the same miss as the demo's two — a generator
preamble change with no regeneration. Regenerating it produced a file `ruff`
rejected, and that was the actual defect: the `_elements` helper was emitted
unconditionally while `Array` was imported only for a catalog with an array
column. With the two gated together the committed file needs no change at all,
and `--check` passes on it as it stands; the fix is in the generator, and the
regeneration that found it is not in the diff. The demo has one, so the two conditions coincided there and its
file was fine; the retention example has none. Both now derive from
`has_array_column`, the TypeScript helper is gated the same way, and
`test_a_catalog_with_no_array_column_gets_no_array_helper` covers the case that
had none — a catalog *without* the feature. The demo frontend's table guard
also demanded exact equality with `head.toml` and now takes a named
`NOT_IN_THE_UI` exemption, in the idiom this repository uses elsewhere.

## What this does not do

**No generator.** That is the rest of the comparison row, and the design note
argues the row may be asking for the wrong artifact: a generator emits a
migration *file* and this store has none, so "generated migrations" here most
plausibly means `--plan` naming column-level changes, which it now does.
Confirming that means re-reading what Drizzle Kit and Alembic actually produce,
which is a product question.

**Nothing recomputes the fingerprint from the stored schema.** The note
suggested it and called it a suggestion: the two are written together and
should agree, and a record where they do not is a state nothing can currently
name. The refusal's fallback branch mentions the possibility and that is all it
does — a table whose stored schema agrees with the code while its hash does not
gets a message saying so, and no `Corrupt` refusal.

**No measurement.** A state record is read once per table at startup and now
carries a few dozen bytes per column. Nothing was timed and no record was
measured; the note said so and it is still true.

**The V2 record is one-way per table.** An older binary meeting one gets
`UnknownFormat` and refuses to start. Fail-closed and correct, and it belongs
in a release note rather than in a rollback.

**No end-to-end `--plan` case.** `crates/slate-serverd/tests/plan.rs` runs the
binary and covers the other four steps, and it cannot reach this one: every
store it makes is written by *this* binary, so every record already carries a
schema and the step is never planned. The line is asserted by a unit test in
the binary instead, which checks the three things an operator is deciding on —
what it writes, what it buys and what it costs on a rollback — and does not
prove the flag prints it.

**The refusal's fallback branch still prints hashes, deliberately.** A record
with no stored schema has nothing else to say, and the message says which of
the two situations the reader is in. That branch will stop being reachable on
any deployment that has migrated once since this change, and nothing removes
it, because "a restored backup" has no expiry.
