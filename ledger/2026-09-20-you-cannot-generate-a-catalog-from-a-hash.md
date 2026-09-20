# The one gap row that cannot be built as written

- **Date:** 2026-09-20
- **Author:** Claude, starting the first of the seven remaining feature gaps and stopping
- **Touches:** `docs/orm-comparison.md`
- **Kind:** docs

## What changed

One gap row, reclassified from "not built" to "blocked, and here is by what".
No code: the finding is that the row as written names work that cannot be done
in the order it implies.

## Why

Nine feature rows remain in `orm-comparison.md` after today's corrections, and
the first of them — *generated migrations from a schema diff* — looked like the
most tractable, because `migrate.rs` already plans and applies a diff. The row
says the missing half is that "nothing *writes* the target catalog for you".

Reading `migrate.rs` before starting: **the store does not know its own
schema.** A table's meta record is `TableState`, which is

```rust
pub struct TableState {
    pub schema_version: u32,
    pub fingerprint: u64,
    pub built: Vec<IndexId>,
}
```

and the fingerprint is FNV-1a over the column count, each column's *type
discriminant*, nullability, dropped flag and decimal scale, then the primary
key's ordinals. No names. No types recoverable from a hash. The function's own
comment explains why it hashes the discriminant rather than the name — "a
rename of the *variant* upstream would change a name-based hash and invalidate
every deployed database for no reason on disk" — which is exactly right and
exactly what makes it one-way.

`stored_state_of` seals it twice over. It iterates `catalog.tables()` and reads
`meta_key(table.id())` for each, so it cannot enumerate a table the catalog
does not already name; and `decode_state(table, &bytes)` takes the `&TableDef`
as an argument, because the stored bytes are not self-describing.

So a generator has nothing to read. **You cannot generate a catalog from a
hash**, and no amount of work on the generator changes that.

The other reading of the row does not apply either. Drizzle Kit and Prisma
Migrate generate a *migration file* from the difference between two schemas you
hold. Here there is no file: the catalog is the artifact, both sides are in
hand at startup, and the diff is computed then — which `--plan` already prints
before applying. There is nothing to generate because there is nothing to
store.

## Alternatives rejected

**Build the generator against the fingerprint anyway**, inferring what it can.
It can infer nothing: a `u64` does not yield a column list, and searching the
preimage space of FNV-1a over arbitrary schemas is not a feature.

**Persist the whole `TableDef` in the meta record, then generate.** The real
answer, and much larger than the row suggests: a serialisation format for
`TableDef` that is stable across versions, a migration for every deployed
keyspace to write one, and a decision about what happens when the stored schema
and the configured catalog disagree — which is the question the fingerprint
exists to answer cheaply. `slate-schema` has no `serde` on `TableDef` today,
which is itself a choice this would reverse. Worth doing if introspecting an
existing bucket becomes a goal; not worth starting inside a row that says
"write the generator".

**Leave the row alone and pick a different one of the nine.** What I would have
done if I had started building rather than reading, and the row would still be
telling the next reader that a morning's work is available here. The whole
value of this file is that its claims are checkable; a row that is checkably
wrong about *what kind of work it is* is the same defect as the three corrected
an hour ago, one level up.

**Move it to "Forbidden by the architecture".** Too strong. Nothing forbids
storing the schema; it is a decision that was made the other way for good
reasons, and reversing it is work rather than an architectural impossibility.
The row now says "blocked, not unbuilt", names the blocker, and says what the
real first item is.

## Evidence

Read, and quoted above: `TableState` (`migrate.rs:95`), `fingerprint`
(`migrate.rs:255`, FNV-1a over discriminants and ordinals), `stored_state_of`
(iterates `catalog.tables()`), `decode_state(table, &bytes)` (takes the
definition it is supposedly recovering). `grep -rn "serde\|Serialize"
crates/slate-schema/src/table.rs` returns nothing, so no `TableDef` is
serialised anywhere.

`python3 site/check/docs.py`: every relative link resolves.

No behaviour changed, so there is nothing to mutation-test. This is a claim
about what the code does not store, and the evidence for it is that the struct
has three fields and none of them is a schema.

## What this does not do

**It does not decide whether to persist the schema.** That is a design
conversation with a real trade — a second copy of the truth, which can disagree
with the catalog, against the ability to adopt an existing bucket and to
introspect one. The fingerprint's whole appeal is that it detects disagreement
without storing a second copy. I have stated the option and not argued for it.

**It does not touch the other six open rows**, which I verified as accurate an
hour ago but only as "the feature is absent" — not as "the work is the shape
the row implies". Given that the very first one I looked at closely was not,
the same reading is owed to window functions, CTEs, views, full-text search,
the array type and seed factories before any of them is picked up.

**`--print-schema` was not examined as a partial answer.** It prints the
*configured* catalog as JSON, which is the thing you already have; whether it
could be the output format of a future generator is a question I did not ask.
