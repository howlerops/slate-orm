# A chain's computed value is now tested from all three clients, and writing the first test found `concat` splicing Rust's `Debug` form into a label

- **Date:** 2026-09-15
- **Author:** Claude, working from "I don't want any gaps. Please address those"
- **Touches:** `crates/slate-kernel/src/scalar.rs`, `crates/slate-kernel/tests/scalars.rs`, and a chain test in each of the three client suites
- **Kind:** fix

## What changed

Two things, and the second was found by the first.

**`Chain::compute` is now exercised from every client.** It needed no new API:
a chain is a `JoinQuery` with more than two inputs and the same `compute`
field, and its values come back in `JoinedRow.computed` like a join's. That is
exactly why it was worth writing tests for — nothing in any client suite had
ever asked for one, and "the field is already there" is not evidence that the
path works. Each suite gains two: an expression reading all three inputs on the
row path, and a grouping keyed on the chain's computed value.

**`Scalar::Concat` renders a non-string value as its text, not its `Debug`
form.** It was `format!("{other:?}")`, so `concat(name, "/", book_id)` on a
`u64` id produced `ada/U64(10)`. Now integers are digits, floats are Rust's
`Display`, booleans are `true`/`false` and a UUID is its hyphenated form. Bytes
and vectors are null.

## Why

The chain tests were a recorded gap: `Chain::compute` and `Join::compute` are
one wire field and two code paths in the kernel — a join's values are filled in
by `JoinCursor::next` as it pairs rows, a chain's by a pass over the
accumulated rows after the last step in `chain::run` — and only the first had
ever been asked for anything by a client.

The `concat` bug is the quiet kind. No error, no null: a label built per row
with a Rust type tag in the middle of it, and a query that looks like it
worked. SQL's `||` renders a number as its digits, and nobody writing
`concat(name, '/', id)` would think to check whether the id came back as
`U64(10)`. It survived because no Rust test concatenated a non-string: the
kernel's `Concat` tests join strings, the wire's round-trip property never
evaluates, and the SQL front end has no `||`. The Python chain test asserted
`ada/a-one/10`, got `ada/a-one/U64(10)`, and that was the whole of it.

## Alternatives rejected

**Assert `U64(10)` in the test and move on.** The literal cheapest option, and
it would have written the bug into the test suite as the specification. The
assertion was written from what SQL does before the answer was known, which is
the only reason it caught anything.

**Render bytes as hex, or as base64.** Both are defensible and that is the
problem: a caller who concatenates a `bytes` column into a label has not chosen
between them, and picking one silently decides for them. SQL refuses the
concatenation outright; null is this layer's way of saying the same thing,
because a `Scalar` has nowhere to put an error. The same argument disposes of
vectors, where the alternative is a hundred floats in a label.

**Format floats as SQL does — `1.0` rather than `1`.** That means a float
formatter in the kernel, with its own rounding decisions, for a value that is
going into a label. Rust's `Display` is used and the choice is written down in
the code rather than left to be discovered.

**Give `Value` a `Display` impl in `slate-tuple` and use it here.** Tempting,
and wrong at that layer: `Display` would then be the one true rendering for
every consumer — error messages, the keyspace viewer, the workbench's cells —
and each of those wants something different (the viewer wants a type tag,
because showing the *encoding* is its whole point). A private helper in
`scalar.rs`, named for what it is for, keeps the decision local to
concatenation.

**Add a `ChainQuery` message to the wire so a chain is not a `JoinQuery`.** A
much larger change, and the protocol's existing position is defensible and
documented: a chain is a join with more inputs, and the response shape is
already common. The gap was in the tests, not in the protocol.

## Evidence

**Six new client tests, all passing**, and the grouped ones are the sharp half:
the four matched books are from 2001, 1990, 2003 and 2010, so grouping a chain
by `books.year / 10 * 10` must give exactly `{1990: 1, 2000: 2, 2010: 1}`. The
counts are written out from the fixture rather than folded from another query,
so a group key that landed on some table's column instead fails rather than
agreeing with an equally wrong fold. The Python version *does* fold, from a
second query that reads `books.year` directly and never uses a computed value,
so the two halves cannot be wrong together.

The row-path tests use an expression reading all three inputs. That choice is
load-bearing: evaluated against a prefix of the accumulated row it comes back
null, and against the last step alone it is missing the name.

**Mutation testing `concat_text`, two mutations:**

| mutation | caught by |
| --- | --- |
| back to `format!("{other:?}")` | `concat_renders_a_value_as_its_text_rather_than_its_debug_form` |
| bytes rendered rather than nulling | the same test |

**Suites, against a rebuilt `slate-serverd` / `slate-testserver`:** Python 161
passed (159 before), Go `./...` ok, TypeScript 66/66, `slate-kernel`,
`slate-server` and `slate-serverd` all green, `cargo clippy --workspace
--all-targets` clean.

The concat fix is demonstrated end to end rather than only in the kernel: the
Python assertion that failed against the old server passes against the rebuilt
one, through gRPC, with the value computed by a chain.

## What this does not do

**`ORDER BY` a chain's computed value is untested from a client.** The grouped
tests sort by nothing in particular; ordering *groups* by a computed key is
tested for a join and not for a chain.

**No test declares a per-input computed value on a chain** — only on a
two-input join. The mechanism is the same (it travels in that input's row), and
the three accessors added alongside this are exercised there.

**No outer-join step in a chain carries a computed value.** The chains here are
inner throughout, so nothing checks what a chain's computed value does when an
earlier step contributed no row. The kernel flattens such a row shorter and the
server pads it, which is tested; the *expression* over one is not.

**The `concat` fix changes an answer.** Any caller relying on the `Debug` form
— which nothing in this repository was — gets different text now. It is a
behaviour change rather than a pure fix, and it is the right one: the old
output was not a format anyone chose.

**Nothing checks the other value-rendering sites for the same mistake.** There
are several places that turn a `Value` into text — the keyspace viewer, the
workbench's cells, error messages — and each was written separately. Only
`Concat`'s was wrong here, and only because someone asked it.
