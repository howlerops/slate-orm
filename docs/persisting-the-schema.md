# Persisting the schema, and the one-way function in the way

`orm-comparison.md`'s generated-migrations row is blocked on a single sentence:

> `TableState` — the whole of what a table's meta record holds — is a schema
> version, a `u64` FNV-1a **fingerprint** and the list of built index ids.
> There are no column names, no types and no primary key anywhere in the
> keyspace … A fingerprint is one-way by construction; you cannot generate a
> catalog from it.

So the work is *"persist the schema, not only its fingerprint"* — a storage
format change with its own migration. This note settles what to store, how the
change migrates itself, and what it buys **before** any generator exists.

> **Built.** `TableState::schema`, `STATE_FORMAT_V2`, `StoredSchema`,
> `layout_changes` and a refusal that names the column are in `migrate.rs`, and
> `crates/slate-kernel/tests/migrations.rs` is the demonstration. Three things
> this note left open or got wrong are marked inline below; the largest is §2,
> where the lazy upgrade it describes did not happen and needed a step of its
> own.

It concludes that the right amount to store is exactly the fingerprint's
inputs, that the format change should be lazy and per table rather than a
flag day, and that the first deliverable is not the generator at all.

## What the fingerprint covers, quoted from the refusal

The store already tells a reader precisely what it can and cannot detect:

> the fingerprint covers the number of columns, each one's type and
> nullability, which columns are dropped, the primary key and the tenant
> column — one of those is not what it was. Renames, CHECKs and foreign keys
> are not covered and are not this.

That sentence is the specification. Everything below follows from taking it
literally.

## 1. Store the fingerprint's inputs — no more, and no less

Per live column: name, type code, nullable, dropped, scale, element type.
Then the primary key ordinals and the tenant column. That is the whole of
`fingerprint()`'s input, and stopping exactly there is the decision.

**Why not less.** Anything omitted is a difference the fingerprint can detect
and the stored schema cannot explain, which leaves the refusal saying "one of
these six things changed" — which is what it says today.

**Why not more.** A `CHECK`, a `DEFAULT` or a foreign key is deliberately *not*
in the fingerprint, for the reason the fingerprint's own comment gives: hashing
them would make an unrelated schema change break every client. Store them and
the diff can report a change that is **not a migration** — a message about a
`CHECK` that moved, at startup, in a refusal path, about something that needs
no action. Two different questions ("has the layout moved under stored rows?"
and "what does the declaration say?") would share one answer and the sharper
one would lose.

The tight version buys a property worth having: **every difference the
fingerprint detects is one the stored schema can name, and every difference the
stored schema can name is one the fingerprint detects.** That is a test, not a
hope — two catalogs, assert the fingerprints differ if and only if the stored
forms do.

Names are the one addition to the hash's *inputs*. The fingerprint hashes a
column's type and position and not its name — a rename moves no bytes and must
not look like a migration. But a diff that cannot say `email` is a diff that
says "column 3", so the name is stored and **not** hashed. That asymmetry is
the point of storing rather than extending the hash.

## 2. The format change migrates itself, lazily, per table

`decode_state` already refuses what it cannot read:

```rust
if *format != STATE_FORMAT_V1 {
    return Err(Refusal::UnknownFormat { … });
}
```

So a naive `STATE_FORMAT_V2` is a flag day: every existing table becomes
unreadable and every deployment needs a dump and reload. Unacceptable for a
store that already holds rows.

**Read both, write the new one.** `format == 1` decodes as today and yields a
state with *no* stored schema; `format == 2` carries one. A table's record is
rewritten as V2 the next time it is migrated, which is already a transaction
that writes the record. The upgrade needs no separate step and no downtime, and
it arrives per table rather than all at once.

> **"Needs no separate step" was wrong, and a test caught it.** `apply` writes
> a table's record only when the plan has a step for that table — so a table
> with nothing to do, which is every table on most deploys, kept a version 1
> record for ever and kept refusing with two hex numbers. The upgrade is now
> `Step::RecordSchema`, planned whenever a record carries no schema. A step
> rather than a silent rewrite inside `apply`, because everything else this
> module does is visible in `--plan` beforehand, and a format change that makes
> the record unreadable to an older binary is exactly what an operator would
> rather be told about than discover.

**The unknown-schema case is permanent, not a transition.** That is the part
worth designing for rather than tolerating: a table registered by an older
binary, a restored backup, a table nobody has migrated since the upgrade. The
differ has to handle "I do not know this table's previous shape" for ever, so
it is a first-class answer — fall back to today's fingerprint refusal, and say
which of the two situations the reader is in.

**A downgrade is one-way per table.** An older binary meeting a V2 record gets
`UnknownFormat` and refuses to start. Fail-closed and correct, and worth saying
in a release note rather than discovering.

## 3. One record, not two

The alternative is a second key holding the schema beside the state record.
Both are written inside the migration's transaction, so atomicity is not the
argument — the argument is that two records are two things that can disagree,
and the disagreement would be silent. `TableState` grows a field.

The shape follows what is already there: `built` is a `Value::Bytes` holding
packed index ids, and the schema is a second `Bytes` holding its own
tuple-encoded blob — a count, then per column a name, a type code, a flags
byte, a scale and an element code. Nested rather than flattened because the
outer decode is a fixed type list and a variable column count does not fit one.

## 4. The first deliverable is a better refusal, not a generator

Today, changing a column's type gets you two hex numbers and a list of six
things one of which moved. With the schema stored it can be:

> `users`: column 2 `email` was `string` and is now `u64`.

**That is worth the storage change on its own**, it exercises every part of the
persistence on the path where being wrong is cheapest, and it is a
self-checking deliverable: the refusal can only name a difference the stored
schema actually holds.

Sequencing matters here because the generator is the part with the least
information. A generator emits a *migration* from a diff, and this store has no
migration files — the catalog is the artifact and the diff is computed at
startup, which `--plan` already prints. So "generated migrations" here most
likely means `--plan` naming the column-level changes it currently can only
hash. The row should be re-read with that in mind before anyone writes a
generator for a file format that does not exist.

## 5. The second deliverable: telling an append from a retype

The refusal in §4 was the cheap half. The expensive half is that a stored
schema changes what can be *allowed*, not only how a refusal reads.

Before it, the test pinning this behaviour was called
`adding_a_nullable_column_is_not_a_migration` and asserted a refusal, with a
comment giving the reason: "the runner cannot tell an appended column from a
retyped one and the safe answer to 'I cannot tell' is no… **This is the
sharpest limitation of the fingerprint and it is recorded rather than papered
over: a genuinely additive change needs a hand.**"

The fingerprint moves for both, identically, because it is one number. The
stored schema removes the cannot-tell, and `evolution()` answers it:

- Every change that is *not* the column count reinterprets a column that
  already exists, or the key. Refused.
- A count that went **down** is refused too. An append is safe because
  `present_at(column, written_version)` reads the row's own written version and
  hands back a default for a column that did not exist yet; a narrowing has no
  such mechanism working for it, since every stored row was encoded with the
  wider list and the trailing value is still in the bytes with nothing
  declaring what it is.
- A count that went **up**, with the prefix untouched, is an append — provided
  every new column's `added_in` is *strictly after* the version rows were
  written at. That proviso is the whole of what makes it safe, and it is where
  a plausible-looking append is still wrong: a column declared `added_in = 2`
  on a table whose rows were written at 2 would have `present_at` say it was
  already there, and the decoder would read a value nobody wrote.

Nothing is read and nothing is rewritten. `Step::WidenSchema` writes one key —
the new stored schema — and names which columns it adds and which it retires,
because a plan saying "3 columns changed" sends the operator to the config file
to find out which.

**This required one change to the differ that reads like a bug fix and is not.**
`layout_changes` originally reported the column count and stopped, on the
argument that a count difference makes every later comparison meaningless. That
is true of a general diff and wrong here: "two columns were appended" and "two
columns were appended *and* column 1 was retyped" are the two answers
`evolution` exists to separate, and only the common prefix separates them.

## What this note does not do

~~**It does not decide the inner encoding's own versioning.**~~ **Decided: yes.**
The outer format says how the record is laid out and the inner says what a
column record holds, and those change for different reasons — adding a
per-column property with only an outer version would need `STATE_FORMAT_V3`
and a third branch in `decode_state`, where with `SCHEMA_FORMAT_V1` it is one
branch in `decode_schema`. A blob claiming a version this binary does not know
is refused rather than misread, which is the outer format byte's own argument
one level down.

**It does not size anything.** A state record is read once per table at
startup, so a few dozen bytes per column is very likely irrelevant — and that
is a judgement, not a measurement. Nothing was timed and no record was
measured.

**It does not say what happens to a table whose stored schema and whose
fingerprint disagree.** They are written together and should not, but "should
not" is how corruption gets described before it is found. The honest answer is
that recomputing the fingerprint from the stored schema and comparing is cheap
and would turn an impossible state into a `Corrupt` refusal — and that is a
suggestion, not a settled design.

~~**It does not examine the wasm or in-memory backends.**~~ `MemoryStore` does
follow, and every test here runs against it. The count was right and the
conclusion was not: `TableState { … }` was constructed in exactly two places
inside `migrate.rs` and adding the field broke exactly those two — but `Step`
is a public enum that `slate-serverd`'s `--plan` renderer matches exhaustively,
so the new *variant* broke a third site this note did not think to look for.
The compiler found it, which is the right outcome and is the reason neither
enum is `#[non_exhaustive]`.

**It resolves nothing about the comparison row's wording.** §4 argues the row
may be asking for the wrong artifact. Confirming that means re-reading what
Drizzle Kit and Alembic actually emit and deciding what the equivalent is when
there is no migration file — and that is a product question, not a storage one.
