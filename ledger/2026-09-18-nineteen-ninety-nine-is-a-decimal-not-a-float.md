# A number with a point in it is read at the column's scale, and rendered back the way it was written

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `slate-tuple` (`Value::decimal_from_str`, `Value::decimal_to_string`), `slate-wasm` (`literal`, `text`, `render`, `group_value_type`, the `books` fixture), `site/check/workbench.py`
- **Kind:** fix

## What changed

`slate-tuple` grew the two halves of a decimal's text form:
`Value::decimal_from_str(text, scale)` reads `"19.99"` at scale 2 as
`Decimal(1999)`, and `Value::decimal_to_string(units, scale)` writes it back.
Both split on the point and read integers; neither touches an `f64`.

The SQL front end uses them. `literal` gained a `Decimal` arm and a `scale`
parameter, threaded from `ColumnDef::scale` at every call site; `text` and
`render` take the scale so a decimal shows as `19.99` rather than
`Decimal(1999)`; and `group_value_type` — which decides what a `HAVING`
literal is typed as — now returns the scale beside the type, so
`HAVING sum(price) > 60.00` compares a decimal to a decimal.

The workbench's `books` fixture grew a `price` column at scale 2, because a
front end that can read a decimal literal and a fixture with no decimal column
is a feature nothing exercises.

## Why

Three silent wrong answers, all the same shape.

**`WHERE price > 19.99` returned nothing.** `literal` had no `Decimal` arm, so
a decimal column fell through to the catch-all and the literal became
`Value::Str("19.99")`. A string sorts below every decimal in the cross-type
order, so the comparison was well defined, fast, and wrong: no rows, no error,
no way to tell from the outside that anything had happened. This is the worst
failure a query engine has available.

**`HAVING sum(price) > 60.00` admitted nothing**, for the same reason one
level up. `Total::sum` over a decimal returns a `Decimal`; `group_value_type`
answered "integer or float" for every summed column, so the literal was parsed
as `I64(60)` and compared to a `Decimal` across a class-rank boundary. The
existing comment on that function explains this hazard for `avg` and floats —
the decimal case was the same hazard, unnoticed because no fixture had a
decimal column.

**`UPDATE books SET title = ... ` failed on any table with a decimal in it.**
The SQL update path reads the row, renders every cell to text, edits the named
ones and writes the strings back through the same parser an `INSERT` uses.
`text` had no decimal arm, so it rendered `Decimal(895)` — which is not a
number, so the write was refused whatever it set. This one was found by adding
the fixture column: every `UPDATE` test in `tests/sql.rs` went red at once.

## Alternatives rejected

**Parse to `f64` and multiply by a power of ten.** Two lines instead of forty,
and it is what the code did everywhere else. It is also wrong, and the test
`the_float_shortcut_loses_a_cent` is the counterexample kept in the tree:
`("8.20".parse::<f64>() * 100.0) as i64` is **819**, because 8.2 has no exact
double and the product lands just under 820. A cent, lost silently, on a price
somebody typed.

**Keep the scale out of `text` and render decimals as their units.** Simpler
signature, no threading. Rejected because `1999` in a price column is a lie
that reads as a number — worse than `Decimal(1999)`, which at least looks
wrong. The threading is the cost of showing the right thing.

**Leave the fixture alone and test the literal against a purpose-built
schema.** Much smaller diff: sixteen tests in `slate-wasm` asserted on `books`
having four columns and had to be updated. Rejected because the feature would
then exist with nothing in the shipped surface pointing at it, and this
repository has a written-down history of exactly that: a workflow that never
fired, a Python suite that skipped itself. The three defects above were all
found *by* adding the column. A cheaper change would have found none of them.

**Refuse a literal with more places than the column's scale, always.** That
was the first version, and it refused `19.830` at scale 2 — a number the
column holds exactly. Pedantry dressed as rigour, and the sort of rule that
teaches readers to distrust the rules. It now refuses only when a dropped
digit is non-zero, so `19.999` is still refused at scale 2 and `19.830` is
1983 units.

## Evidence

**Two properties**, in `crates/slate-tuple/tests/decimal_text.rs`:
`decimal_text_round_trips` samples the whole `i64` range against every scale
0–18 and asserts units → text → units is the identity;
`a_written_decimal_renders_back_to_itself` goes the other way, over written
forms, which is what says the rendering is canonical. Between them they cover
the cases the hand-written tests found one at a time — a magnitude shorter
than the scale, a negative number whose form has a leading zero, `i64::MIN`
(which has no positive counterpart, so rendering through a negation panics in
debug and wraps in release).

**Eight mutations**, each restored:

| # | mutation | outcome |
|---|---|---|
| N1 | the fraction is not padded to the scale | killed — `a_written_number_is_a_count_of_units` |
| N2 | the sign is dropped | killed — four tests, both properties among them |
| N3 | extra places are silently truncated | killed — `more_places_than_the_scale_is_refused`, `a_trailing_zero_past_the_scale_is_not_a_loss`, and the SQL refusal test |
| N4 | rendering negates instead of taking the magnitude | killed — `the_most_negative_value_renders` |
| N5 | a decimal literal is a string again | killed — four SQL tests |
| N6 | a decimal renders as its units again | killed — three SQL tests |
| N7 | a summed decimal is typed as an integer again | killed — `having_over_a_summed_decimal_compares_as_a_decimal` |
| N8 | the single-table grid renders with no scales | killed — three SQL tests |

**A defect the property suite found while it was being written.** `"."` parsed
as `Decimal(0)`: an empty whole part is how `".50"` arrives and is substituted
with `"0"`, and the emptiness check ran *after* the substitution.
`what_is_not_a_decimal` caught it on the first run.

**The browser.** `site/check/workbench.py` grew two checks — that
`price > 19.99` selects ids 18..=24 (the seeded prices run 8.95 up in 68-cent
steps, so 20.51 is the first over) and that the grid shows `20.51` rather than
`2051`. The whole check passes, 77 of them.

`cargo test -p slate-wasm --no-fail-fast`, `cargo test -p slate-tuple`,
`cargo clippy --workspace --all-targets`, `python3 site/check/docs.py`,
`site/check/quickstarts.py`, `site/check/workbench.py`: all green.

## What this does not do

- **The demo and the three clients are untouched.** The explorer's `books`
  already has a `price` column and the conformance corpus already compares the
  three SDKs on a decimal *value*; none of them can express a decimal
  *literal* in a filter, because none of them has a SQL front end. A client
  builds `Value::Decimal(1999)` directly, which is the scale-agnostic form and
  is exactly right — but it means each client's caller does the
  cents-to-units arithmetic themselves, with nothing checking they used the
  column's scale.
- **Nothing checks a client's idea of a column's scale against the server's.**
  Still true, still recorded, still not fixed. The schema fingerprint does not
  cover scale.
- **A chain's computed value renders with no scale**, so a computed decimal in
  a chain shows as its units. The kernel refuses a computed decimal whose
  scale is not one a column already declares, so this is narrow, but it is not
  nothing.
- **`group_value_type`'s computed-key branch still returns `I64`** with no
  scale. Unchanged, and the comment there already says no query can tell that
  branch from reading the source column's type. A decimal computed key would
  be the first that could — and `compute_scalar` refuses a non-integer source,
  so it cannot arise yet.
- **The `taxi` fixture has no decimal column.** Its fare columns are `f64`,
  which is the wrong type for money and is the one place in the repository
  that still demonstrates the problem rather than the fix. Left alone: the
  numbers are real NYC taxi data, `site/data/bucket.json` is a committed
  listing of a real load of them, and changing the column type would invalidate
  that listing for a point already made by `books`.
