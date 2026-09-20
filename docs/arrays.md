# An array column, and the four decisions it needs

`orm-comparison.md` lists an array / list column type as missing, with the
evidence that `ValueType` has no `Array`. That is true, and it is the only one
of the nine remaining gap rows that is ordinary work rather than a blocked item
or a decision already made elsewhere — which is exactly why it is worth writing
the decisions down before touching six crates.

This note settles four of them and leaves two open. It concludes that every
decision has a precedent in this codebase already, that three of the four are
forced by properties the tuple codec must not lose, and that the smallest
honest first version **refuses an array in a key and in an index** rather than
guessing at what either should mean.

## 1. The element type lives on the column, not in the type

`ValueType` is a fieldless, `Copy`, `#[non_exhaustive]` enum with a
`const fn name(self)`. The obvious spelling — `ValueType::Array(Box<ValueType>)`
— breaks all four of those at once: no `Copy`, no `const fn`, no finite
`ValueType::ALL`, and `type_code` in `migrate.rs` stops being a `const fn` too.
That is a large change to the whole type system to describe one column.

**The precedent is `Decimal`, and it is exact.** A decimal's scale is not in
its type either:

```rust
pub const fn scale(&self) -> Option<u8> {
    match self.ty {
        ValueType::Decimal => Some(self.scale),
        _ => None,
    }
}
```

It lives on `ColumnDef`, is declared by `TableBuilder::decimal_column(name,
scale)`, returns `None` for a column that is not a decimal so "a caller cannot
read a scale off a type that does not have one", and is hashed into the
fingerprint from the column: `byte(column.scale().unwrap_or(0), &mut hash)`.

So: `ValueType::Array` stays fieldless, `ColumnDef` gains
`element_type() -> Option<ValueType>`, the builder gains
`array_column(name, element)`, and the fingerprint hashes the element's
`type_code` the way it hashes the scale. Every property above survives.

**What this costs** is the same thing the scale costs: `ValueType` alone no
longer fully describes a column. That was already true, and the alternative is
to make it true for one type by a mechanism that does not fit any other.

## 2. Ordering is lexicographic, with shorter-is-less

The codec is order-preserving — that is the whole point of it — so an array's
encoding has to sort the way arrays should sort. The standard answer, and the
one every ordered store uses, is element-wise lexicographic with a shorter
prefix sorting first: `[1]` < `[1, 2]` < `[2]`.

**A length prefix cannot give that.** Encoding the count first makes `[2]`
sort before `[1, 2]`, because one is less than two before any element is
compared. Length-prefixed is what a *serialisation* format does; this is a key
encoding.

**The technique already exists here, for strings.** A `Str` is written as a
tag, then the bytes, then a NUL terminator, with `0xFF` escaping so no byte of
the payload can be confused for the end. `read_escaped` is that decoder. An
array is the same shape one level up: a tag, then each element encoded in full
(its own tag and body), then a terminator byte chosen to sort below every
element tag. Shorter-is-less falls out: the terminator is reached first and is
smaller than whatever the longer array has there.

**The terminator has to be below every tag, and the tag space is not free.**
`codes` already assigns `0x00` upwards, and the escape machinery reserves
`0x00` and `0xFF`. Which byte is available, and whether the element encodings
need their own escaping inside an array, is the one part of this that has to be
read out of `codec.rs` rather than reasoned about here.

## 3. No arrays of arrays, in the first version

Nesting is expressible — element-wise encoding recurses naturally — and it is
refused anyway, for two reasons that are not about the encoding.

**The element type is a `ValueType`,** which after decision 1 cannot name an
array's own element type. `Array(Array(Str))` would need the payload that
decision 1 declined to add. Allowing one level of nesting with an unspecified
inner element is worse than allowing none.

**Depth is an input an attacker chooses.** The wire already carries an explicit
depth limit on expression conversion, added because unbounded recursion over a
caller-supplied structure is a denial of service. A nested array is the same
shape, and the first version should not open it without the same ceiling.

## 4. An array cannot be a key or an index, in the first version

`slate-schema` already refuses a vector in a primary key or an index, and
`pred.rs` explains why in terms worth reusing: a vector's "order is
deterministic but says nothing about similarity, which is why the schema layer
refuses one in a key or an index".

An array is a weaker version of the same problem. Its order *is* meaningful,
so the vector's argument does not transfer directly — but what a user wants
from an indexed array is almost never "find rows whose array sorts near this
one". It is containment: *which rows have `tag` in their `tags`?* That is an
inverted index, which is a **new index cardinality** — one entry per element
per row, against a write path where `entry_for` returns exactly one
`IndexEntry` per row. That is the same structural work full-text search needs,
and it is a separate item.

So the first version stores and returns arrays, compares them for equality and
ordering in a predicate evaluated per row, and refuses one in a key or an
index — with an error message that says containment is the thing that would
need an index and that it is not built, rather than a bare "not supported".

## Open, and deliberately not decided here

**Whether the element type may be nullable.** A `[1, null, 3]` is meaningful in
Postgres and a nuisance everywhere else. The encoding handles it — `Value::Null`
has a code — but the question is whether a non-nullable array column should
accept a null element, and that is a semantics question with no obvious right
answer.

**What `SUM` and `COUNT` do with an array column.** Nothing, presumably, the
way they do nothing with a vector; but "presumably" is how three of today's
findings started, and the aggregate path has a closed enum that will need an
arm or an explicit refusal either way.

## What this note does not do

**It does not check the tag space.** Decision 2 depends on a terminator byte
below every element tag being available in `codes`, and on whether an element's
own escaping composes inside an array. I reasoned about the technique from the
string encoding and did not read the tag assignments, which is the one place
this note could be wrong in a way that changes the design rather than the
prose.

**It does not size the client work.** Three clients, the generated decoders and
encoders, the wire `Value` message and the SQL front end's literal syntax all
need a case. The gap row's evidence is about `ValueType`; the cost is mostly
outside the kernel, and this note is entirely about the kernel.

**It does not argue that an array column is worth building.** The comparison
lists it because Drizzle, SQLAlchemy and Ecto have one. Whether the answer here
should instead be a child table — which this schema layer is good at, and which
gives containment for free through an ordinary index — is a product question
this note assumes has been answered yes.
