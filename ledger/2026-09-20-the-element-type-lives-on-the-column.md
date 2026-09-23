# An array column: declared, stored, validated, fingerprinted, refused in a key

- **Date:** 2026-09-20
- **Author:** Claude, building the second increment of the F3 gap row
- **Touches:** `crates/slate-schema/src/{table.rs,row.rs,error.rs}`, `crates/slate-kernel/src/migrate.rs`, `crates/slate-kernel/tests/{arrays.rs,migrations.rs}`, `docs/{arrays.md,orm-comparison.md}`
- **Kind:** feature

## What changed

`ColumnDef::element_type()`, `TableBuilder::array_column(name, element)` and
`nullable_array_column`, an `element_for` escape hatch matching `scale_for`, and
four refusals: an array column with no element type, an array of arrays, an
array in a primary key or an index, and an array element whose type is not the
declared one. The element type is hashed into the schema fingerprint beside the
decimal scale. `crates/slate-kernel/tests/arrays.rs` is nine tests running the
whole thing through a real `RecordStore`.

The previous commit made `Value::Array` exist. This one makes a *column* able to
be one.

## Why

`docs/arrays.md` decisions 1 and 4. Decision 1 puts the element type on the
column rather than in `ValueType`, because `ValueType::Array(Box<ValueType>)`
costs `Copy`, `const fn name`, a finite `ValueType::ALL` and `type_code`'s
`const fn` — four properties, to describe one column. The precedent is exact:
a decimal's scale is already arranged this way, down to `scale()` returning
`None` for a non-decimal so a caller cannot read one off a type that has none.
`element_type()` is that function with the type changed.

Decision 4 refuses an array in a key or an index. The refusal existed for
vectors and the temptation was to reuse it. That would have been wrong in the
*message*, which is the part a user reads: a vector is refused because its
order says nothing about similarity, and an array's order says plenty. What an
indexed array is actually asked is containment — *which rows have this tag* —
and that is one index entry per element against a write path that produces
exactly one. `ArrayInKey` says so, and the test asserts the word "containment"
appears, so the distinction cannot be lost to a later tidy-up that merges the
two arms.

## Alternatives rejected

**Reuse `VectorInKey` for both.** One arm, one error, half the diff. Rejected
because the two refusals are not the same refusal and a shared message would
have to be vague enough to cover both — which means telling somebody with an
array column that its order is meaningless, when it is not, and leaving them no
idea that containment is the thing that would need building. The shared
*function* is kept; the errors are not.

**Store the element as `ValueType` with a sentinel rather than `Option`.** The
scale does this — `scale: u8`, zero for a non-decimal — because zero is a
meaningful scale and the accessor's `match` on the type is what makes it safe.
There is no natural sentinel `ValueType`, so `Option` it is, with the same
`match self.ty` guard so a stray element set through `element_for` still reads
back as `None`. That guard is not decoration: a mutation returning the field
directly survived until the test covered a column that had one *and* was not an
array.

**Refuse a stray element type at build time rather than ignoring it.** Tempting
— silently ignoring is how things hide. Rejected for consistency with
`scale_for`, whose doc comment already argues it: the builder reports nothing
until `build()`, and the column such a call would have named is refused there
under its own error, which is a better message than one about an element type.
The safety of ignoring it is asserted rather than assumed: the stray value does
not reach the fingerprint either, so two schemas differing only in it do not
look like a migration.

**Fold the element type into the column's type code** — one number,
`type_code(Array) * 16 + type_code(element)`, no new contribution to the hash.
Rejected because `type_code` is a stable on-disk numbering that exists
precisely so a variant added upstream cannot renumber the others, and an
arithmetic combination reintroduces exactly that coupling. Hashed beside it
instead, with `0` for "no element type" — safe because every type's code is
non-zero, which `every_type_has_its_own_non_zero_code` now holds it to.

**Accept a null array element.** `Row::validate` runs on every write and cannot
decline to have an opinion, and `docs/arrays.md` left nullable elements open.
Refusing is the reversible half: it can be relaxed later without breaking a
stored row, whereas accepting is a promise the wire and three clients would
have to keep from the moment it ships. It is a separate knob from the column's
own nullability, which still works.

**Validate elements at encode time rather than in `Row::validate`.** The codec
is deliberately indifferent to whether an array is homogeneous — nothing in the
encoding says the elements share a type, and that is what lets it stay a codec
rather than a schema. The column is the only thing that knows, so the column is
where it is checked, on the path every write already takes.

## Evidence

Thirteen mutations through `scripts/mutate.py` across three files. Eleven caught
on the first run, over `--test arrays --test vectors --test migrations`:

```
ok  an array is allowed in a key and an index      -> an_array_cannot_be_a_key_or_an_index
ok  a vector is no longer refused, only an array   -> a_vector_cannot_be_a_key_or_an_index
ok  the key refusal looks at the wrong column      -> both of the above
ok  an array column need not declare an element type -> an_array_column_declares_its_element_type_and_cannot_nest
ok  an array of arrays is accepted                 -> the same
ok  array_column forgets to record the element type -> four tests
ok  element types are not checked at all           -> an_element_of_the_wrong_type_is_refused
ok  a null element is accepted                     -> the same
ok  only the first element is checked              -> the same
ok  the element type is left out of the fingerprint -> the_element_type_is_part_of_the_fingerprint
ok  the element type is hashed as the column's own type code -> the same
```

**Two survivors, both mine to fix, and only one of them was the code's fault.**

*`element_type` returning the field directly* survived
`only_an_array_column_reports_an_element_type`, because that test's "not an
array" case was a plain `Str` column that had never been *given* an element
type — the field was `None` anyway, so the assertion proved nothing. A vacuous
case reads exactly like a real one. The test now builds a column with a stray
element type through `element_for` and asserts both that it reads back `None`
and that it does not move the fingerprint.

*"a vector is no longer refused"* was not a survivor at all: my `mutate.py`
invocation was `-p slate-kernel --test arrays -p slate-schema`, which cargo
resolved to one test binary, so `vectors.rs` never ran. The harness said "1
suites reported" and I read past it. Re-run with the three suites named, it is
caught. **Failure mode 2 — "nothing ran" looking like "nothing failed" — met
again, and the harness's suite count is what made it visible.**

**The mutation run also found a real interaction in a test written an hour
earlier.** `every_value_type_fingerprints_apart` loops `ValueType::ALL` building
a one-column table per type, and once an array column had to declare an element
type, that loop could no longer build one. The harness reported `the suite is
red before any mutation` rather than scoring anything against a broken baseline.
The test now special-cases the array arm and says why.

Suites: `-p slate-tuple -p slate-kernel -p slate-schema --no-fail-fast`, no
failures. `cargo clippy --workspace --all-targets`: clean.
`cargo fmt --all -- --check`: clean. `scripts/check.sh`: 29 of 29.

**One documented guess was run instead of recorded.** `docs/arrays.md` said
`SUM` over an array column would "presumably" do nothing, the way it does for a
vector — two sentences after warning that "presumably" is how three of this
session's findings started. It does refuse, with `NotSummable` naming the type,
and `an_array_cannot_be_summed_but_can_be_counted` now pins it. The guess was
right; the note has been corrected to say it was checked, because a wildcard
could as easily have produced a zero. `COUNT` deliberately still counts arrays.

## What this does not do

**Nothing outside the kernel can use an array.** This is the whole of what is
left of the F3 row, and two concrete edges were found while building:
`slate-server`'s `value_to_proto` has a wildcard that sends
`<unrepresentable array>` — loud, which is the right failure, but a failure —
and `slate-serverd`'s TOML type parser refuses `type = "array"` with a message
listing the nine types it knows. The wire `Value` message, the three clients,
the generated row types and a SQL array literal all have no case.

**That TOML message is a hand-written roster of types.** It is correct today
and it is exactly the shape this repository keeps finding stale. Nothing holds
it to `ValueType::ALL`. Left as is because the next increment rewrites it
anyway, which is a reason and not an excuse.

**No `#[derive(Record)]` support.** An array field in a derived record has no
attribute to declare its element type, so the macro cannot produce an
`array_column` call. Not attempted, not designed.

**Equality and ordering only.** There is no containment predicate, no
`ARRAY_AGG`, no element access, no concatenation. Decision 4 says containment
needs an index cardinality this store does not have, and the rest was never in
scope; but a user with an array column today can compare whole arrays and
nothing else.

**No measurement.** An array column's row body is longer by a tag, its elements
and a terminator, and nothing was timed. `crates/slate-kernel/tests/footprint.rs`
measures row size for other types and was not extended.

**The element check is O(elements) on every write, and that is not bounded.**
A row carrying a million-element array is validated element by element before
anything else looks at it. There is a per-request size ceiling in the daemon,
but nothing in the kernel caps an array's length, and no test explores a large
one.
