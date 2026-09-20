# Four decisions written down before touching six crates

- **Date:** 2026-09-20
- **Author:** Claude, starting the one remaining gap row that is ordinary work
- **Touches:** `docs/arrays.md`, `docs/orm-comparison.md`
- **Kind:** docs

## What changed

`docs/arrays.md`, a design note settling four decisions an array column needs
and leaving two open, plus the gap row pointing at it. No code.

## Why

Nine gap rows remain after today's triage. Seven are blocked, refused, or need
a design decision that is somebody else's to make. The array column is the one
that is ordinary work — so it is the one to start, and starting it means
touching `slate-tuple`, `slate-schema`, `slate-kernel`, `slate-serverd`, the
wire, three clients and the code generator.

`validation.md` is the precedent for what to do first. It exists because
`orm-comparison.md` said of validations: *"That is a design note somebody
should write before any of it is built."* That note then found that the guess
about where validation lives was right and the guess about what the hard part
was, wrong. The same risk applies here, and the cost of finding out mid-build
across seven crates is much higher than the cost of a note.

Three of the four decisions turn out to be forced rather than chosen, which is
the useful result:

**The element type cannot live in the type.** `ValueType::Array(Box<ValueType>)`
breaks `Copy`, the `const fn name(self)`, the finite `ValueType::ALL` added
today, and `type_code`'s `const fn` in `migrate.rs` — four properties, to
describe one column. `Decimal` already faced this and answered it: the scale
lives on `ColumnDef`, `scale()` returns `None` for a non-decimal so "a caller
cannot read a scale off a type that does not have one", and the fingerprint
hashes it from the column. An array's element type follows exactly.

**A length prefix cannot be used.** The codec is order-preserving, and encoding
the count first makes `[2]` sort before `[1, 2]`. Element-wise with a
terminator gives lexicographic order and shorter-is-less for free — and the
technique is already in the file, as the NUL terminator and `0xFF` escape that
make a `Str` sort correctly.

**Nesting has to wait on decision 1.** `Array(Array(Str))` needs the payload
that decision 1 declined to add, so one level of nesting with an unspecified
inner type would be worse than none. Depth is also an attacker-chosen input,
and the wire already carries an explicit depth limit on expression conversion
for that reason.

The fourth is a choice: **refuse an array in a key and an index.** What a user
wants from an indexed array is containment — *which rows have `tag` in
`tags`?* — and that is an inverted index, one entry per element per row,
against a write path where `entry_for` returns exactly one `IndexEntry`. That
is the same new cardinality full-text search needs, established earlier today,
and it is a separate item.

## Alternatives rejected

**Start coding and decide as I go.** What the schedule wants. Two of these
decisions are irreversible once written: the encoding is on disk, and a
length-prefixed array shipped for one release cannot be reordered without a
migration of every row.

**`ValueType::Array(Box<ValueType>)` anyway**, accepting the four broken
properties. It is the self-describing spelling and it is what most codebases
would do. Here it would mean `ValueType::ALL` cannot exist — the set becomes
infinite — and that constant is a day old and exists because a hand-written
list of types went stale and left `Decimal` unfuzzed. Trading a
compiler-enforced enumeration for a nicer type signature is the wrong
direction.

**Allow an array in an index, ordered.** Cheap: the encoding already sorts, so
a b-tree over it works. Rejected because it would answer the wrong question
convincingly — a range scan over array order finds rows whose arrays sort near
each other, which nobody wants — and a plausible wrong answer is worse than a
refusal. `pred.rs` makes the same argument for vectors in almost the same
words.

**Write the note and also start the codec.** The note's own last section says
it may be wrong about the tag space, which is the one thing that would change
the design. Writing code on top of an unverified assumption is how the note
stops being worth having.

**A child table instead of an array column.** The honest alternative to the
whole feature, and the note says so in its last paragraph: this schema layer is
good at child tables, and one gives containment for free through an ordinary
index. I did not argue it either way, because "should this exist" is a product
question and the row assumes it has been answered.

## Evidence

Read and cited in the note: `ColumnDef::scale` (`slate-schema/src/table.rs:122`,
`Option<u8>` keyed on the type), `TableBuilder::decimal_column` (line 924), the
fingerprint's `byte(column.scale().unwrap_or(0), &mut hash)`, `read_escaped`
and the `STR` arm in `slate-tuple/src/codec.rs`, `entry_for`
(`slate-kernel/src/record.rs:2774`, one `IndexEntry` per row), and `pred.rs`'s
refusal of a vector in a key.

`python3 site/check/docs.py`: the docs site holds together — the note is a
repository design note beside `validation.md`, not a site page, so the
sidebar's "every page on disk is listed" rule does not apply to it. I checked
that rather than assumed it: the check globs `*.html` under the site's `docs/`,
not this directory's markdown.

`scripts/check_cited_tests.py`: 10 documents now, every cited test resolves.
`scripts/check.sh`: 29/29.

No code changed, so there is nothing to mutation-test.

## What this does not do

**It does not check the tag space, and that is the one thing that could change
the design.** Decision 2 assumes a terminator byte below every element tag is
available in `codes`, and that an element's own escaping composes inside an
array. I reasoned from how `Str` works and did not read the tag assignments.
The note says so in its own caveats; a build should start by confirming it.

**It settles nothing about the clients.** Three clients, the generated decoders
and encoders, the wire `Value` message and the SQL front end's array literal
all need a case, and the note is entirely about the kernel. The gap row's
evidence is `ValueType`, but most of the cost is outside it.

**Two decisions are left open on purpose** — whether an element may be null,
and what the aggregates do with an array column. Both have a defensible answer
either way and neither blocks the encoding, which is what the note was for.

**No code was written.** The task is not done; what is done is that it can now
be started by somebody who has not spent an afternoon reading this codebase.
