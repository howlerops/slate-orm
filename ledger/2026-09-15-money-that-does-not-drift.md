# An exact decimal, as an integer count of the column's unit

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-tuple/src/{value,codec}.rs`, `crates/slate-tuple/tests/ordering.rs`, `crates/slate-schema/src/{table,error}.rs`, `crates/slate-kernel/src/{aggregate,migrate}.rs`, `crates/slate-kernel/tests/decimals.rs` (new), `crates/slate-orm/src/{field,lib}.rs`, `crates/slate-derive/src/lib.rs`, `crates/slate-orm/tests/money.rs` (new), `crates/slate-server/tests/decimal_wire.rs` (new)
- **Kind:** feature

## What changed

`Value::Decimal(i64)`, `ValueType::Decimal`, and a per-column `scale`. A decimal
is a count of the column's smallest unit: `Decimal(1250)` in a scale-2 column is
`12.50`. It encodes as an integer, orders as an integer, and sums exactly. The
ORM has `Units` and `#[record(scale = 2)]`.

## Why

There was no decimal type at all, so money was an `f64` or an `i64` with a
comment. Measured, through the actual aggregate path, over a hundred rows of ten
cents:

```
SUM over a decimal column:  Decimal(1000)   exactly $10.00
SUM over the same as f64:   9.99999999999998
```

Both halves are asserted in `a_hundred_dimes_are_exactly_ten_dollars`, including
`assert_ne!(drifted, 10.0)` — if floating point ever sums to exactly ten the test
fails rather than quietly agreeing, because then this column would have no
reason to exist.

## Alternatives rejected

**A self-describing decimal** carrying its own scale, so `Decimal(1250, 2)`
prints itself. It is the obvious design and it was rejected for three reasons
that compound:

- *Ordering.* `1.5` and `1.50` are the same number with different
  representations, so an order-preserving encoding has to normalise before
  encoding — and getting that subtly wrong silently corrupts index order, which
  is the worst failure this system can have. With the scale on the column there
  is nothing to normalise.
- *Equality.* Index keys require that equal values encode identically. Two
  spellings of one number means one value with two encodings.
- *Arithmetic.* Summing across scales needs rescaling and therefore rounding.
  Summing counts of one unit is integer addition.

The cost is real and is stated everywhere it matters: **a value cannot print
itself**. Rendering needs the column. Every layer that renders one has the
schema, and `Units::to_string_with_scale` takes the scale rather than guessing.

**An `i128` or arbitrary-precision mantissa.** Rejected on the encoding: the
integer type codes run `0x0D..=0x1D`, eight magnitude bytes each way, and
widening past `i64` would need new codes adjacent to `f64` and `uuid`. `i64`
units at scale 2 covers ±9.2 × 10¹⁶ currency units, which is not the limit
anybody hits first.

**Sharing the integers' encoding outright**, with no distinguishing byte.
Tempting — a decimal *is* the integer encoding — and rejected because then the
integer `1250` and a scale-2 `12.50` compare equal. They are different numbers.
One byte separates them, and there is a test asserting exactly that.

**Letting `SUM` mix decimals with ordinary numbers.** Refused instead, in both
directions. A decimal's units mean what the column says; an integer's mean one
each. Adding them gives a number in no unit at all, which is the kind of wrong
answer that looks right. The check is per value rather than on the first one,
because an accumulator that latched on the first would accept every mixture that
*starts* with a decimal — which is the shape the order-dependence bug already
recorded in `add_integer` had.

**Falling back to a float when a decimal sum overflows.** An integer sum does
that. A decimal sum returns null instead: a float is an acceptable answer for a
count and not for money.

## Evidence

The decision that made this safe was to reuse the existing encoding rather than
invent one, and to put decimals through the **existing property suite** rather
than write a suite of their own. Adding one generator line to
`crates/slate-tuple/tests/ordering.rs` covers round-tripping, byte order equals
value order, prefix-freeness and descending order — against every other type,
not just against decimals.

It paid immediately. The first run failed:

```
byte_order_matches_value_order
  a=[Decimal(9223372036854775807)]  b=[Decimal(-9223372036854775808)]
  left: Greater, right: Less
```

`Value::Decimal` had a class rank and no arm in `Ord`, so it fell through to a
branch whose comment read *"Unreachable: equal class ranks are exhausted
above"* — a claim the new variant had just made false. **Every decimal compared
equal to every other.** The comment now records what happened instead of
claiming to be unreachable.

A second run failed in `skipping_and_decoding_can_be_mixed`: the codec's `skip`
walks elements without decoding them and did not know the new code. A skip that
disagreed with the decoder by one byte would read the next column's bytes as
this one's.

Seven tests in `crates/slate-kernel/tests/decimals.rs` (exact sums beside the
drifting float, ordering across zero and a magnitude boundary, decimal ≠ integer,
the scale on the column, the scale refusal, and a rescale caught as a migration
refusal), two unit tests on the accumulator, five in
`crates/slate-orm/tests/money.rs`, and two pinning the wire.

### The mutation pass

Six mutations, no survivors:

| mutation | result |
| --- | --- |
| a decimal shares the integers' class rank | caught (2 suites) |
| the decimal comparison falls through again | caught (2 suites) |
| `skip` forgets to read the inner integer code | caught |
| `SUM` drops the decimal flag | caught (2 tests) |
| `SUM` allows mixing | caught |
| the fingerprint ignores the scale | caught |

## What this does not do

**It does not cross the wire.** The gRPC protocol has no decimal field, so a
decimal reaching a client becomes the `<unrepresentable decimal>` marker that
`value_to_proto`'s fallback arm was written for — visible, rather than the
silent null it would otherwise be. Two tests in `slate-server` pin that, and say
in their own doc comment that they are **pins, not endorsements**: when the proto
grows the field they should fail and be rewritten.

This is now the third feature on this branch wanting the same protocol change —
the pagination cursor, the conditional update, and this. That is an argument for
doing them together rather than three times.

**`AVG` over a decimal returns a float**, as it does over an integer. The mean of
exact decimals is generally not representable at the same scale, so something has
to give; consistency with integers was chosen over refusing. For money, sum and
divide yourself. This is the one place in the feature where exactness stops and
it is not hidden.

**No arithmetic on decimals in `Scalar`.** `price * quantity` is not expressible,
because the product of two scale-2 values is scale-4 and nothing in the
expression layer tracks that. Adding it means scale arithmetic, which is the
part of decimal support with the most edge cases and none of them are here.

**The SQL front end has no decimal literal.** `WHERE total > 19.99` parses as a
float and will not match a decimal column. The parser resolves literals against
the column type already — there is a test named for it — so this is a small
change, and it is not in this commit.
