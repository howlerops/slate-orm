# The adversarial decoder suite had never seen a decimal

- **Date:** 2026-09-20
- **Author:** Claude, on the third of the review's four unexamined areas
- **Touches:** `crates/slate-tuple/src/value.rs`, `crates/slate-tuple/tests/untrusted.rs`, `docs/security-review.md`
- **Kind:** correctness

## What changed

`ValueType::ALL`, and the fuzzer's type list derived from it rather than
hand-written. `any_scalar` gains `Decimal` too. No decoder change: **the
decoder was right, and adding the missing type found nothing.**

## Why

`docs/security-review.md` lists four areas it did not examine, one of which is
*"the tuple codec's behaviour on adversarial encoded input beyond the existing
`slate-tuple/tests/untrusted.rs`"*.

That file is good: 4,000 cases per property, bytes biased towards the tags and
the NUL and the 0xFF escape that mean something to the codec, every truncation
of a valid encoding, every single-byte corruption, and a vector claiming four
billion elements out of five bytes. It is not where I expected to find
anything.

What it had was a hand-written list:

```rust
fn any_type() -> impl Strategy<Value = ValueType> {
    prop_oneof![
        Just(ValueType::Bool),  Just(ValueType::Bytes), Just(ValueType::Str),
        Just(ValueType::I64),   Just(ValueType::U64),   Just(ValueType::F64),
        Just(ValueType::Uuid),  Just(ValueType::Vector),
    ]
}
```

Eight. `ValueType` has nine. `Decimal` arrived with the decimal work and
nothing connected the two, so the suite whose whole job is to hand the decoder
bytes it did not write had **never once told it to expect a decimal** — not in
the arbitrary-bytes case, not in a truncation, not in a corrupted byte.
`any_scalar` was short the same variant, so the truncation and corruption cases
never produced a valid decimal to damage either.

This is the fifth roster to go stale here today, and the first one that is a
test fixture rather than a check.

## Alternatives rejected

**Add `Decimal` and move on.** Two words. It is what four earlier versions of
four other lists got, and all four went stale again. The list is the defect;
`Decimal` is just the variant that happened to expose it.

**Write the exhaustive match in the test.** The obvious fix and it does not
compile. `ValueType` is `#[non_exhaustive]`, so from outside `slate-tuple` the
compiler *demands* a wildcard arm — and the wildcard is exactly the hole, since
a tenth variant would fall into it silently. This is worth knowing: a
`#[non_exhaustive]` enum cannot be exhaustively enumerated by its users, so the
enumeration has to be published by the crate or it cannot be trusted at all.

**A Python script over the source**, in the style of the four checks written
today. Rejected because a better instrument was available: `ALL: [Self; 9]`
puts the count in the *type*, so removing a variant from the list is a compile
error rather than a test failure, and the exhaustive match inside the crate
makes adding one a compile error too. A grep-based check is what you write when
the compiler cannot be made to care. Here it can.

**Put `ALL` in the test crate as a `const`.** Then nothing ties it to the enum,
which is where this started.

**Add `Uuid` and `Vector` to `any_scalar` while I was there.** They are
deliberately excluded — a vector's length prefix is covered by the hostile-byte
cases, and a uuid is sixteen fixed bytes with no structure for a corruption to
interact with. That reasoning was in the comment for the vector and not for the
uuid; it is now written for both, because an exclusion with no reason is
indistinguishable from an omission, which is how `Decimal` survived.

## Evidence

**The gap, measured**: `ValueType` has nine variants (`value.rs:83–100`),
`any_type` listed eight, and the missing one is `Decimal`.

**Adding it found nothing.** All eight properties pass with `Decimal` in the
rotation at 4,000 cases each: `decoding_arbitrary_bytes_never_panics`,
`dynamic_decoding_never_panics`, `skipping_arbitrary_bytes_never_panics`,
`mixed_reads_and_skips_never_panic`,
`every_truncation_of_a_valid_encoding_is_handled`,
`a_single_corrupted_byte_is_handled`. **That is a null result and it is the
result.** The decoder handles a hostile decimal exactly as it handles the rest;
what was wrong was that nobody could have known.

**`all_lists_every_variant`** pins `ALL` with a wildcard-free match, inside the
crate where that is possible. Three mutations:

```
ok  ALL lists a variant twice and another not at all -> all_lists_every_variant
ok  two entries of ALL are swapped                   -> all_lists_every_variant
```

and a third that **could not be scored, in the best way**: dropping a variant
from `ALL` is a type error, because the array's length is part of its type, so
the harness reported `NOTHING RAN — error[E0308]: mismatched types`. The guard
is stronger than the test that guards it.

Positions rather than `contains`, because a list with a duplicate *and* a gap
passes a membership loop and fails this one — which is the first mutation
above.

**I did not mutate the test's own assertion.** Deleting the only assertion in
the only test of a property survives by construction; that is true of every
test and is not a finding. Mutating a test with itself is circular, and the
thing I added and needed to break is `ALL`.

`cargo test -p slate-tuple`: 8 property tests plus the unit test, all passing.
`cargo test -p slate-kernel`: no failures. `cargo clippy --workspace
--all-targets`: zero diagnostics. `cargo fmt --all -- --check`: clean.
`scripts/check.sh`: 27/27.

## What this does not do

**It does not widen the contract.** The property is still "for any input,
decoding returns `Ok` or `Err`" — no panic, no runaway, no out-of-bounds.
Nothing asserts *what* a hostile decimal decodes to, and nothing should: the
weak contract is the one that can hold.

**`any_scalar` is still a hand-written list**, now of eight of `Value`'s ten
variants, with the two exclusions reasoned. ~~`Value` has no `ALL` and I did
not add one: its variants carry data, so an array of them is not a natural
constant, and the strategies differ per variant in ways a list cannot express.
A new `Value` variant will be missing from `any_scalar` and nothing will say
so. That is the same defect as the one this entry fixes, left open one level
down, and I am recording it rather than implying otherwise.~~

> **Closed, same day.** The list is still hand-written and now something says
> so. `Value::value_type` is the bridge `Value` lacking an `ALL` seemed to
> rule out: it is an exhaustive wildcard-free match inside the crate, so
> `any_scalar_generates_every_type_it_does_not_exclude` can sample the
> strategy, ask each value its type, and compare the set against
> `ValueType::ALL` minus a `NOT_GENERATED` list that carries a reason per
> entry. A new variant fails to compile in `value_type`, grows `ALL`, and
> fails this — which is the chain I said was not available. I concluded it
> from "an array of `Value` is not natural" without asking what else could
> stand in for one.

**`ValueType::ALL` is new public API.** Additive, and I did not check whether
any downstream generated code enumerates types in a way that should now use it
— `slate-derive` and the three client generators all map types individually.

**The codec's *ordering* properties were not attacked.** `ordering.rs` tests
that encoded bytes sort like the values, against inputs the encoder produced.
Whether a hostile encoding can be made to sort wrongly relative to an honest
one is a different question, and the review did not ask it and neither did I.

**Two of the review's four areas are done and one is closed with a null
result**; the Python client remains.
