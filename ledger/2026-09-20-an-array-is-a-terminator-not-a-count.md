# An array value in the tuple codec, terminated rather than counted

- **Date:** 2026-09-20
- **Author:** Claude, building the first increment of the F3 gap row
- **Touches:** `crates/slate-tuple/src/{value.rs,codec.rs}`, `crates/slate-tuple/tests/{ordering.rs,untrusted.rs}`, `crates/slate-kernel/src/migrate.rs`, `crates/slate-kernel/tests/migrations.rs`
- **Kind:** feature

## What changed

`Value::Array(Vec<Value>)` and `ValueType::Array`, encoded as tag `0x24`, every
element in full, then a single `NUL`. `ValueType::ALL` grows to ten and
`type_code` gains its arm. Both property generators — the ordering oracle's
`any_value` and the adversarial suite's `any_scalar` — now draw arrays, so every
existing property covers them, and each generator gains a check that it draws
every type in `ValueType::ALL`. Two guard tests exist only because a mutation
survived: `every_type_has_a_name_of_its_own` and
`every_type_has_its_own_non_zero_code`.

This is the kernel half only. Nothing outside `slate-tuple` can declare an array
column yet.

## Why

`docs/arrays.md` settled four decisions and the gap row assessed it as the one
remaining item that is ordinary work rather than a blocked one. The note's own
caveats are the reason this increment looks the way it does: it argued
shorter-is-less from five hand-picked cases, all `I64`, compared against a
stand-in that *modelled* the integer encoding rather than calling it. That is an
argument about a model. Putting arrays into `any_value` turns
`byte_order_matches_value_order` — 4096 cases, both directions, heterogeneous
elements, real bytes — into the oracle the argument wanted, and it passed on the
first run.

## Alternatives rejected

**Length-prefix the elements, as a vector is.** The obvious shape, already in
the file, and wrong here. A count sorts `[2]` below `[1, 2]`: one is less than
two before any element is compared. A vector gets away with it because its order
is admitted to be meaningless — the schema layer refuses one in a key for that
reason — and an array's order is the thing being offered. `array_ordering_regressions`
names the case, and the mutation that reintroduces the count is caught by four
tests.

**A two-byte terminator, as a byte string has.** A string needs two because its
payload is escaped bytes, so a lone `0x00` inside it is ambiguous with the end.
An array needs one, and the difference is worth stating precisely rather than
copying the safer-looking answer: the array decoder never *scans* for the
terminator. It reads elements one at a time, each consuming exactly its own
bytes, so `0x00` is only ever examined on an element boundary, where no tag can
be `0x00`. One byte is not a shortcut here; two would make `[]`'s encoding
longer for nothing. Prefix-freeness is not taken on that argument —
`element_encodings_are_prefix_free` now draws arrays too.

**`ValueType::Array(Box<ValueType>)`.** Costs `Copy`, `const fn name`, a finite
`ValueType::ALL` and `type_code`'s `const fn`, all to describe one column. The
precedent against it is exact and already in the codebase: a decimal's scale
lives on `ColumnDef`, not in its type. Worked through in `docs/arrays.md` §1.

**Bound the nesting depth instead of refusing it.** A ceiling is a number
somebody has to get right, and the property holds at every depth. It is also not
a real restriction yet: `ValueType::Array` cannot name an inner element type, so
a nested array is a value no column can describe — accepting one on decode would
mean recursing to a depth chosen by whoever wrote the bytes, which need not be
this encoder. The wire's expression converter carries an explicit ceiling for
the same hazard; this one can simply refuse.

**Share one array loop between `read_array` and `skip`.** They differ in exactly
the thing `skip` exists for — not materialising a `Value` per element — so
sharing would cost the saving. The duplication is a real cost and the `DECIMAL`
arm right above it avoids the same duplication by recursing, so this is a
departure: the nesting refusal is now written twice and could drift.
`a_deeply_nested_array_is_refused_rather_than_recursed` asserts it at both entry
points for that reason, and `skipping_agrees_with_decoding` holds the lengths
together over generated arrays.

**Hand-written ordering cases instead of the generators.** Kept both, which is
the repository's stated position: the oracle catches what nobody thought of, the
named cases say which mistake each one is about. The named cases exist so a
future change is told *what* it broke, not merely that something did.

## Evidence

Eleven mutations through `scripts/mutate.py`. Nine caught:

```
ok  an array is length-prefixed instead of terminated  -> array_ordering_regressions, decode_prefix_returns_the_suffix, byte_order_matches_value_order, round_trips
ok  the array terminator is not below every element tag -> array_ordering_regressions, cross_type_order_is_the_documented_rank, ...
ok  the decoder accepts a nested array                 -> a_deeply_nested_array_is_refused_rather_than_recursed
ok  skip accepts a nested array                        -> a_deeply_nested_array_is_refused_rather_than_recursed
ok  skip does not rewind before skipping an element    -> skipping_and_decoding_can_be_mixed
ok  read_array does not rewind before reading          -> round_trips, an_unterminated_array_stops_at_the_end_of_the_buffer, ...
ok  an array ranks with vectors instead of above them  -> cross_type_order_is_the_documented_rank, byte_order_matches_value_order
ok  arrays compare backwards                           -> array_ordering_regressions, byte_order_matches_value_order
ok  arrays compare by length first, as vectors do      -> array_ordering_regressions, byte_order_matches_value_order
```

**Two survived, and both were missing tests rather than redundant code.**

*`ValueType::Array => "vector"` in `name()`.* Nothing tested `name` at all, for
any type. A duplicate name makes `TypeMismatch` read "expected vector, found
vector", a message that cannot be acted on. `every_type_has_a_name_of_its_own`
now loops `ValueType::ALL`; both the duplicate and an empty name are caught.

*Deleting `ValueType::Array`'s arm from `type_code`,* so it falls through to
`_ => 0`. This survived `every_value_type_fingerprints_apart` — the integration
test written in this same change — and the reason is the interesting part: with
a *single* type falling through, code 0 is unique and no two fingerprints
collide. The test passes and the trap is armed; the harm arrives with the second
fallthrough, by which time the first is deployed. `type_code` is private, so
`every_type_has_its_own_non_zero_code` lives beside it and asserts the half the
public API cannot see. Both mutations are caught now.

**One mutation was invalid and the harness said so** rather than scoring it:
removing an entry from `ValueType::ALL` is a type error (`[Self; 10]`), and
fixing the length makes `all_lists_every_variant`'s wildcard-free `position`
match non-exhaustive. `NOTHING RAN — ['error[E0308]: mismatched types']`. That is
the right answer: the completeness of `ALL` is enforced by the compiler, not by
a test, so there is no behaviour mutation to make. The valid version — listing
`Vector` twice, keeping the length — is caught by `all_lists_every_variant`.

**One mutation of the new test itself** (`assert_ne!(code, 0)` → `assert_ne!(code,
999)`) survived, and is not a finding: it mutates the oracle rather than the
code. The assertion is not vacuous — the code mutation it exists for, deleting an
arm, is caught by that exact line.

**Five new `proptest-regressions` seeds are committed, and none of them is a
bug this repository had.** They were recorded while the mutated code was
failing, which is worth saying because the file reads as a history of real
defects and four of the existing entries are exactly that. They are kept
because they are the minimal counterexamples that separate this encoding from
the wrong ones — `Array([])` against `Array([Null])` is the terminator case,
`Array([])` against `Vector([])` is the rank case — so re-running them first is
free and catches the same mistakes again.

`cargo test -p slate-tuple -p slate-kernel --no-fail-fast`: 60 suites, no
failures. `-p slate-schema -p slate-orm -p slate-derive`: 21 suites, no failures.
`-p slate-server -p slate-serverd`: 32 suites, no failures.
`cargo clippy --workspace --all-targets`: clean, after fixing two `clippy::panic`
warnings and an `items after a test module` that `-D warnings` would have turned
red in CI. `cargo fmt --all -- --check`: clean. `scripts/check.sh`: 29 of 29.

**The increment is contained to `slate-tuple`, and that was checked rather than
assumed.** `Value` and `ValueType` are both `#[non_exhaustive]`, so every
downstream match already carries a wildcard; all four were read
(`kernel/scalar.rs`, `serverd/value.rs`, `serverd/lang/pred.rs`,
`server/convert.rs`) and each fails loudly — `value_to_proto` sends
`<unrepresentable array>` rather than a null, `pred.rs` refuses the literal. The
`type_code` wildcard is the one that fails *quietly*, which is why its arm is in
this change and why the test above exists.

## What this does not do

**No column can be declared an array.** `ColumnDef::element_type`,
`TableBuilder::array_column`, hashing the element type into the fingerprint, and
the refusal of an array in a primary key or an index — decisions 1 and 4 of the
design note — are all still to come. Until they are, `ValueType::Array` is
reachable only by constructing a `TableDef` by hand, which
`every_value_type_fingerprints_apart` does.

**Nothing outside the kernel knows about arrays.** The wire `Value` message, the
three clients' encoders and decoders, the generated row types and the SQL front
end's literal syntax all have no case. `value_to_proto` will send
`<unrepresentable array>` if one ever reaches it.

**Elements decode dynamically, not by the column's element type.** An array
element that is an integer comes back `I64` where it fits and `U64` otherwise —
the same answer `read_dynamic` gives a bare integer, and the same bytes either
way, so nothing about ordering or the round trip turns on it. It would still be
wrong for a `U64` column to read back `I64` once the schema can declare one, and
that is work for the increment that adds `element_type`.

**The design note's two open questions are still open.** Whether a non-nullable
array column may hold a null element, and what `SUM`/`COUNT` do with an array
column. Neither is decided here, and the aggregate path has a closed enum that
will need an arm or an explicit refusal.

**The nesting refusal is tested at two entry points, not proved at one.**
`read_array` and `skip` each carry their own copy. A third caller that walked an
array some other way would not inherit it. There is no check that enumerates
array-walking code the way `scripts/check_write_paths.py` enumerates write
paths, and I did not write one for two call sites.

**No measurement.** An array costs a tag, its elements and one byte; a
length-prefixed encoding would cost a tag, four bytes and its elements. I did
not time encode or decode, and nothing here claims a performance property.
