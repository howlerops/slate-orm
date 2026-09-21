# What to persist beside the fingerprint is "its own inputs", and the argument is the stopping point

- **Date:** 2026-09-21
- **Author:** Claude, designing F1a, which blocks F1
- **Touches:** `docs/persisting-the-schema.md`, `docs/orm-comparison.md`
- **Kind:** docs

## What changed

`docs/persisting-the-schema.md`: what to store, how the format change migrates
itself, and what the first deliverable is. The comparison's generated-migrations
row points at it.

No code. The row asked for the design and this is the design.

## Why

`F1` (generated migrations) is blocked on `F1a`, and `F1a` was one sentence —
"persist the schema, not only its fingerprint" — with no decisions in it. The
decisions turn out to be three, and two of them are about *stopping*.

**How much to store** is the one with a real argument on both sides. Too little
and the diff cannot explain a refusal the fingerprint raised; too much and it
explains differences that are *not* migrations. `fingerprint()`'s own comment
already argues that hashing a `CHECK` or a `DEFAULT` would make an unrelated
schema change break every client — and storing them has the matching flaw one
layer up: a startup refusal path reporting that a `CHECK` moved, about
something needing no action.

Store exactly the hash's inputs and a property falls out that can be tested
rather than hoped for: **every difference the fingerprint detects is one the
stored schema can name, and every difference the stored schema can name is one
the fingerprint detects.**

Column names are the single deliberate exception, and they sharpen the rule
rather than break it. A name is stored and *not* hashed — the fingerprint
ignores it because a rename moves no bytes, and the diff needs it because
"column 3" is not something a reader can act on. That asymmetry is the whole
reason to store rather than to extend the hash.

## Alternatives rejected

**Bump `STATE_FORMAT_V1` to `V2` and be done.** `decode_state` refuses an
unrecognised format outright, so every existing table becomes unreadable at
once and every deployment needs a dump and reload. Reading both and writing the
new one on the next migration costs one branch and no downtime — and it makes
the interesting case explicit: a record with no stored schema is not a
transitional wart but a **permanent** state (an older binary's table, a
restored backup), so the differ has to answer for it for ever rather than
assume it away.

**A second key holding the schema.** Both would be written in the migration's
transaction, so atomicity is not the objection. Two records are two things that
can disagree, and the disagreement would be silent; `TableState` grows a field.

**Store the whole `TableDef`, `CHECK`s and foreign keys included.** Tempting
because a generator would want them, and it is the wrong trade for the reason
above — and the generator is the part of this row whose requirements are least
understood. Storing for a consumer nobody has specified is how a format grows
fields nobody reads.

**Write the generator first.** The row is named for it. §4 of the note argues
the row may be asking for the wrong artifact entirely: a generator emits a
migration *file*, and here the catalog **is** the artifact and the diff is
computed at startup, which `--plan` already prints. The first deliverable is
instead the refusal — `column 2 \`email\` was string and is now u64` in place of
two hex numbers — which exercises every part of the persistence on the path
where being wrong is cheapest, and can only name a difference the stored schema
actually holds.

## Evidence

Read from source rather than assumed. `TableState` is three fields;
`encode_state` writes four `Value`s — a format tag, a version, the fingerprint
and packed index ids; `decode_state` takes a `&TableDef` as an argument
*because its own bytes are not self-describing*, which is the row's claim
verified at the definition. `fingerprint()` is FNV-1a over a canonical byte
string, so "one-way by construction" is not an approximation.

The refusal text is quoted rather than paraphrased, because it is the
specification: *"the fingerprint covers the number of columns, each one's type
and nullability, which columns are dropped, the primary key and the tenant
column … Renames, CHECKs and foreign keys are not covered and are not this."*
Decision 1 is that sentence taken literally.

`python3 site/check/docs.py`: holds together. `check_cited_docs.py`: 53
citations, all openable.

n/a for measurements: nothing was built and nothing was timed.

## What this does not do

**It builds nothing**, and it does not unblock `F1` by itself — it makes `F1a`
a task with decisions in it rather than a sentence.

**The inner blob's own versioning is undecided.** The outer record gets a new
format number; whether the nested schema needs a version byte of its own, so a
column property can be added without a third outer format, is raised and not
settled.

**Nothing is sized.** A state record is read once per table at startup, so tens
of bytes per column is very likely irrelevant — a judgement, not a measurement.

**The disagreement case is a suggestion, not a design.** If a stored schema and
its fingerprint ever disagree, recomputing one from the other would turn an
impossible state into a `Corrupt` refusal. Cheap and probably right, and I did
not work through when it would run or what it would cost.

~~**I did not check whether any test constructs a `TableState` literal.**~~
**Checked, while writing this.** `TableState { … }` appears in exactly two
places, both inside `migrate.rs` itself: the construction in `decode_state` and
the one in `apply`. No test anywhere builds one, so adding a field breaks two
in-module sites and nothing else, and the struct's `pub` fields are not
constructed from outside the crate. The note still says `MemoryStore` "should
follow" — that one is unverified, and it is a different claim: it is about
whether the *records* go through the same code, not about literals.

**§4's premise is unconfirmed.** The claim that this row may be asking for an
artifact that does not exist here rests on what Drizzle Kit and Alembic emit,
from memory rather than from their documentation.
