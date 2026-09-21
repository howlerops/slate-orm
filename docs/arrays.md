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

> **Built, in the kernel.** All four decisions are code: `ValueType::Array`,
> `ColumnDef::element_type`, `TableBuilder::array_column`, tag `0x24` with a
> terminator, and both refusals. `crates/slate-kernel/tests/arrays.rs` is the
> demonstration and `crates/slate-tuple/tests/ordering.rs` is the oracle.
> **Nothing outside the kernel knows about arrays yet** — not the wire, not the
> three clients, not `slate-serverd`'s TOML schema, not the SQL front end. The
> sections below are kept as written because they are the reasoning; where a
> decision was sharpened by building it, that is marked inline.

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

**The terminator already exists, and the tag space cooperates.** Read out of
`codec.rs` rather than assumed: `NUL = 0x00` is reserved, and the lowest type
tag is `NULL = 0x01`, so `NUL` sorts below every element encoding by
construction. Tags run `0x01`–`0x05`, `0x0D`–`0x1D` for the integers, then
`0x1E` decimal, `0x21` f64, `0x22` uuid, `0x23` vector — **`0x24` is the next
free one** for `ARRAY`.

**No escaping is needed at the array level**, which is the part that could have
gone wrong. An element's body may contain a `0x00`, but the array decoder never
scans for the terminator: it decodes elements one at a time with the same
reader, and each element consumes exactly its own bytes — a string by its own
terminator-and-escape, an integer by the length its tag declares. So `0x00` is
only ever examined at an element boundary, where it cannot be anything but the
end of the array.

Checked against the real tag values rather than argued:

```
lhs          rhs          bytes   lists   agree
[]           [1]            <       <      OK
[1]          [1, 2]         <       <      OK
[1]          [2]            <       <      OK
[1, 2]       [2]            <       <      OK
[0]          [0, 1]         <       <      OK
```

`[1, 2] < [2]` is the case a length prefix gets wrong, and `[0] < [0, 1]` is
the one a naive terminator gets wrong when the terminator is not below every
tag. Both hold.

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

> **Still open, and the build had to answer it anyway.** `Row::validate` runs on
> every write and cannot decline to have an opinion, so it **refuses a null
> element**, for a reason that is about reversibility rather than about
> semantics: refusing can be relaxed later without breaking a stored row,
> whereas accepting is a promise the wire and three clients would have to keep
> from the moment it ships. It is also a separate knob from the column's own
> nullability, which still works — the whole value may be absent, and
> `an_element_of_the_wrong_type_is_refused` asserts both.

**What `SUM` and `COUNT` do with an array column.** Nothing, presumably, the
way they do nothing with a vector; but "presumably" is how three of today's
findings started, and the aggregate path has a closed enum that will need an
arm or an explicit refusal either way.

> **Answered, by running it rather than reasoning about it.** An array reaches
> `Total::add`'s wildcard and is refused with `NotSummable`, naming the type —
> `an_array_cannot_be_summed_but_can_be_counted`. The paragraph above guessed
> exactly that and hedged it "presumably", which is the word this note warned
> about two sentences earlier; the guess was right and is now a test, because a
> wildcard could as easily have produced a zero. `COUNT` is deliberately *not*
> a refusal: counting rows that have a value means something for any type.
>
> What remains open is the aggregate *surface* rather than its behaviour —
> whether an array column should eventually get an array-shaped aggregate
> (`ARRAY_AGG`, a union, a concatenation) is a question nobody has asked here.

## What this note does not do

~~**It does not check the tag space.**~~ **Checked**, and the design survives —
see decision 2. `NUL = 0x00` is already reserved and the lowest tag is `0x01`,
`0x24` is free for `ARRAY`, and no array-level escaping is needed because
element boundaries are self-delimiting. The five ordering cases above were run
against the real tag values, including the two that a length prefix and a naive
terminator respectively get wrong.

**What the check does *not* cover** is a heterogeneous array — the ordering
above is all `I64`. Cross-type order is a property the codec already has (the
tags are assigned in type order on purpose), so element-wise comparison should
inherit it, but decision 1 makes an array homogeneous anyway, so the case
cannot arise through the schema. It could arise through a hostile encoding,
which is the decoder's problem and belongs in `untrusted.rs` when the code is
written.

**It does not size the client work.** Three clients, the generated decoders and
encoders, the wire `Value` message and the SQL front end's literal syntax all
need a case. The gap row's evidence is about `ValueType`; the cost is mostly
outside the kernel, and this note is entirely about the kernel.

> **Both edges closed.** `value_to_proto` sends an `ArrayValue`, and
> `slate-serverd` takes `type = "array", element = "str"` with each half of
> that pairing refused without the other. All three clients carry an array and
> each proves it against a real node.
>
> What the note got right is that the cost was mostly outside the kernel, and
> the sharpest part of it was not the clients: the element type had to be added
> to **four** independent implementations of the `SchemaCheck` fingerprint, one
> per language, because that hash is the only thing that can catch a client
> whose idea of an element type is wrong — it reads the *right* column and
> decodes every element as the wrong type.
>
> One thing is genuinely left. `scripts/codegen.py` refuses an array column
> rather than generating one, deliberately and with a message saying what it
> would need: the declaration must carry the element type, and the decoded form
> is element-typed in all three languages, neither of which its type tables can
> express since they are keyed by the column's type alone.
>
> **The predicate parser now has an array literal**: `tags = ['a', 'b']`, with
> the *column* deciding the element type exactly as it decides a scalar
> literal's type, so `[1, 2]` is a list of `u64` opposite `array<u64>` and of
> `i64` opposite `array<i64>` and neither needs saying in the text. `[]` is
> accepted and is not null, which keeps the surface able to write the
> distinction the kernel makes. Three things are refused with their own
> reasons, and each is a decision this note already made one level down: a list
> inside a list, because an element type is a single `ValueType` and there is
> no array-of-arrays column for it to compare with; a `NULL` element, because
> `Row::validate` refuses one on every write so the predicate could match
> nothing, forever, silently; and a column reference or `:principal` inside a
> list, which are expressible and mean nothing anybody asked for.
>
> Two things were found by writing the tests rather than by reading the code. A
> negative element — `sizes = [1, -2]` — did not parse, because the lexer makes
> `-` its own token and the list loop read one token per element. And the
> refusal for a missing separator was untested in a way that mattered: dropping
> the check entirely *still* produced an error, because the stray element is
> eaten as a separator and the closing `]` then looks like a trailing comma.
> Same refusal count, wrong sentence, green test. The test now pairs each
> malformed input with the words its own refusal has to contain.

**It does not argue that an array column is worth building.** The comparison
lists it because Drizzle, SQLAlchemy and Ecto have one. Whether the answer here
should instead be a child table — which this schema layer is good at, and which
gives containment for free through an ordinary index — is a product question
this note assumes has been answered yes.
