# A decimal column can now be computed over, and the expressions whose answer would be at no scale are refused before the scan starts

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `slate-kernel` — `scalar.rs`, `read.rs`, `join.rs`, `chain.rs`, `error.rs`; `tests/decimal_arithmetic.rs`
- **Kind:** feature

## What changed

`Scalar` arithmetic understands `Value::Decimal`. The evaluator gained a
`Number::Units` variant and a `decimal_op` that implements the four shapes
whose answer is still a count of the operands' unit — `units ± units`,
`units × int`, `int × units`, `units ÷ int` — and the guard that routes to it
sits *before* the float-widening arm, which is the whole point: a decimal that
reaches `f64` has already lost.

Everything else is refused at plan time by a new `Scalar::decimal_scale`,
which walks an expression and answers "what scale is this a count of", or
`KernelError::DecimalScale` naming the operation and what to write instead. It
is reached from exactly two places: `SecuredReads::plan`, for every
single-table `Query::compute` — which is also every join side and every chain
step, because each of those is planned through it — and
`join::validate_compute`, for a join's or a chain's own `compute` in the
joined ordinal space, beside the ordinal check it was already doing. Five
places a `Scalar` can be attached, two call sites, and none of the five can be
reached without passing one.

## Why

`Value::Decimal(i64)` holds a count of the column's smallest unit and the
scale lives on `ColumnDef`, never on the value and never on the wire. That
makes storage, comparison and `SUM` exact, and it makes arithmetic
conditional: `price * quantity` is cents, and `price * discount` is
hundredths of a cent at a scale no column has and nothing on the wire could
carry. Before this, every one of those expressions widened to `f64` and came
back a plausible-looking float — the silent loss the type exists to prevent,
reintroduced one layer up.

A refusal is the right answer rather than a rescale because the kernel has
nowhere to put the result: the answer's scale would have to travel with the
value, and it does not.

## Alternatives rejected

**Let the result carry a scale.** `Value::Decimal { units, scale }` makes
every expression expressible and every refusal unnecessary. It costs a wire
change on every decimal value, a second place for a scale to live (and so a
second place for it to disagree with the schema), and a comparison that has to
rescale before it can order — which is how an order-preserving key encoding
stops being order-preserving. The scale-on-the-column design is load-bearing
for the tuple codec, so this was never really on the table; writing it down is
what makes the refusals defensible rather than arbitrary.

**Refuse in the evaluator only, per row.** Much less code: `decimal_op`
already returns `Null` for the shapes it has no arm for. It was rejected
because a column of nulls at the end of a scan is indistinguishable from a
column that is legitimately null, which is precisely the failure
`validate_compute` was written for one level down. The evaluator keeps its
`Null` as belt and braces, with a comment saying the real refusal happened
earlier.

**Put the plan-time check in `Reads::execute`.** That was the first version.
It covers every read and is one line. It was moved into `Reads::plan` because
`explain` does not go through `execute`: the query planned cleanly, `EXPLAIN`
returned a plan, and the read then failed — a plan for a query that cannot run
is worse than an error, because it looks like an answer. Moving it also made
the join-side and chain-step cases fall out for free, which two explicit calls
in `Join::validate` and `Chain::validate` had been added to handle; see the
evidence below for how they were found to be dead.

**Round a decimal division instead of truncating.** Rejected because rounding
needs a target scale and the answer is in the same unit the numerator was —
there is nothing to round *to*. Truncation toward zero is what the kernel can
do without inventing a policy, and it is documented at the operator rather
than left to be discovered.

## Evidence

26 tests in `crates/slate-kernel/tests/decimal_arithmetic.rs`, and 15
mutations, each restored after:

| # | mutation | outcome |
|---|---|---|
| M1 | `check_scales` forgets earlier computed scales | killed — `compute_reading_an_earlier_computed_decimal`, `a_later_compute_mixing_scales_is_refused` |
| M2 | joined/chained `compute` unchecked | killed — `a_joined_compute_is_checked`, `a_chained_compute_is_checked` |
| M3 | explicit side checks in `Join::validate` removed | **survived** |
| M4 | explicit step checks in `Chain::validate` removed | **survived** |
| M3′ | `plan` does not check a per-table compute | killed — five tests |
| M5 | `int × units` arm removed | killed — `a_count_times_price_is_the_same_money` |
| M6 | division returns `I64` not `Decimal` | killed — `dividing_by_a_count_truncates_toward_zero` |
| M7 | the decimal arm in `arithmetic` removed, so decimals widen | killed — five tests |
| M8 | the `Literal(Value::Decimal(_))` arm of `decimal_scale` removed | **survived, equivalent** |
| M8′ | a decimal literal does not count as mentioning a decimal | killed — `two_decimal_literals_with_no_column_are_refused` |
| M9 | `is_agnostic_decimal`'s recursive `&&` flipped to `||` | **survived, dead code** |
| M10 | `agree` always agrees | killed — the `case` and `coalesce` tests |
| M11 | `round` no longer sees a decimal | killed (did not compile) |
| M12 | `Mul` ignores a decimal literal again | killed — `money_times_a_decimal_literal_is_refused_not_nulled` |
| M13 | `Div` ignores a decimal literal again | killed — `money_divided_by_a_decimal_literal_is_refused_not_nulled` |
| M14 | nothing is scale-agnostic | killed — two tests |
| M15 | `other_side_is_agnostic` requires both sides | killed — two tests |

Four of those are worth more than the eleven that died on the first try.

**M3 and M4 survived, and the comment explaining them was false.** Two
explicit `check_decimal_scales` calls had been added to `Join::validate` and
`Chain::validate`, with a comment saying a side's own `compute` "never reaches
`Reads::execute` — `plan_join` plans each side rather than executing it". It
does execute them: `join.rs:1482` and `chain.rs:784` both call
`reads.execute`. The lines were dead, the comment was wrong, and only removing
them and finding the tests still green said so. The fix was to move the single
check into `plan`, which is what all five paths genuinely share, and to keep
the two tests — plus two more against `explain` and `explain_join`, which are
the paths that *only* the new placement covers.

**M8 survived and is an honest equivalent mutation.** A
`Literal(Value::Decimal(_))` arm returning `Ok(None)` sat above a catch-all
`Literal(_)` returning `Ok(None)`. Deleting it changes nothing, because the
catch-all answers identically; it existed only to hang a comment on. The two
were merged, and the comment now says what actually distinguishes a decimal
literal — `mentions_decimal` — with M8′ demonstrating it.

**M9 survived because the code was unreachable.** `is_agnostic_decimal`
recursed through `Add`/`Sub`, so that `price + (Decimal(1) + Decimal(2))`
could adopt `price`'s scale. It cannot: `decimal_scale` works bottom-up, so
the inner sum is asked first, has no column to take a scale from, and is
refused there. The recursion is gone and the reason is in the doc comment.

**A defect found by reading, demonstrated before it was fixed.**
`price * Decimal(3)` planned cleanly and evaluated to `Null` on every row.
`Mul` and `Div` asked only whether each side *had* a scale; a decimal literal
has none, so `(Some(2), None)` read as "money times a plain number" — the
expressible case — while the evaluator saw `Units × Units`, which `decimal_op`
has no arm for. `Add`/`Sub` never had the bug because they consult
`mentions_decimal`. Three failing tests were written first
(`money_times_a_decimal_literal_is_refused_not_nulled`,
`money_divided_by_a_decimal_literal_is_refused_not_nulled`,
`two_decimal_literals_multiplied_are_refused`), confirmed red, then made
green.

**A claim withdrawn.** `summing_a_computed_decimal_is_exact` was written
asserting that `38.85 + 69.93 + 14.50 + 30.00` added as `f64` is *not*
153.28, on the reasoning that cents are not representable in binary. It is
153.28 — four addends is not enough for the error to escape the rounding.
(`decimals.rs`'s hundred dimes gives 9.99999999999998, which is what a
drifting sum looks like.) The test now asserts the narrower true thing, which
is also the one that regresses: the aggregate returns `Decimal`, not `F64`.

Full `cargo test -p slate-kernel --no-fail-fast`: green.
`cargo clippy --workspace --all-targets`: clean locally.

## What this does not do

- **The SQL front end still parses `19.99` as a float.** `WHERE total > 19.99`
  against a decimal column compares a float to a decimal and matches nothing.
  That is the other half of this item and is not in this commit.
- **No client surface.** The Go, TypeScript and Python `Scalar` builders can
  express arithmetic but nothing in them knows about a decimal's scale, so a
  client can build an expression the server will refuse. The refusal is clear
  and arrives before any rows, but it arrives from the server.
- **Nothing checks a client's declared scale against the server's.** A client
  that thinks `price` is scale 2 where the schema says 4 gets numbers a
  hundred times wrong with no error anywhere. The schema fingerprint does not
  cover scale either. Recorded, not fixed.
- **`AVG` over a decimal is still a float**, which is the deliberate non-goal
  recorded against the README item: an average of cents is not cents, and the
  alternative is the scale-carrying value this note rejects above.
- **Aggregates other than `SUM` were not audited** for what they return over a
  computed decimal. `SUM` is tested; `MIN`/`MAX` pass values through and are
  very likely fine; `AVG` is the known float.
