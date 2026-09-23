# `tags = ['a', 'b']` — the predicate parser learns an array literal

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `crates/slate-serverd/src/lang/{lex,mod,pred}.rs`, `docs/arrays.md`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`slate-serverd`'s predicate language takes an array literal opposite an array
column: `tags = ['a', 'b']`, `sizes = [1, -2]`, `tags = []`, and the ordering
comparisons too. The lexer gained `[` and `]`; `Scope` gained `element_type`;
`pred.rs` gained `array_literal` and `array_element`. `docs/arrays.md` and the
comparison table both recorded this as missing and no longer do.

## Why

`docs/arrays.md` listed two things left after the kernel, the wire and the
three clients had one. This is the second of them: an array column could be
declared in `slate-serverd`'s TOML, written, read, compared and sorted, but
could not appear in a `[[security.policies]]` predicate or a `CHECK`, which are
the two places that file expresses a condition. A column type that the
configuration language cannot mention is one an operator cannot use in the
place they are most likely to want it.

The element type comes from the *column*, which is the same rule every scalar
literal already follows here and for the same reason `literal_from_number`
gives: guessing from the text would make `tags = [7]` mean a different thing
from `tags = [-7]` in one table.

## Alternatives rejected

**Parsing an element by calling back into `operand`.** It is the obvious reuse
and it is wrong. `operand` resolves a bare word as a column and a `:principal`
as the caller, so `tags = [kind]` would mean "this row's `kind`, as one
element" and `tags = [:principal]` would make the *literal* vary by caller
rather than the comparison. Both are expressible, neither has a meaning anybody
asked for, and both are now refused by not being parsed rather than by special
cases that would have to be kept correct. `array_element` takes one token and
handles literals only.

**Allowing nesting, since the encoding recurses.** Rejected for the reason
decision 3 of `arrays.md` already gives one level down: an element type is a
single `ValueType` and cannot name an element type of its own, so there is no
array-of-arrays column for a nested literal to compare with. The refusal says
that rather than "not supported".

**Treating `[]` as null.** Rejected: `arrays.md` makes empty-versus-null
load-bearing in the kernel, and a surface syntax that cannot write `[]` leaves
a stored value nothing can name.

**Allowing a `NULL` element.** `Row::validate` refuses one on every write, so a
predicate naming one would select nothing, forever, with no error. Refusing at
parse time keeps the two ends of the system saying the same thing.

**A `contains` operator instead of a whole literal.** That is the operation
users actually want from an array column, and it is deliberately not here: it
needs an inverted index, which is a new index cardinality and the same
structural work full-text search needs. A literal is what the *existing*
equality and ordering support can be reached with.

**Putting the syntax in `slate-wasm`'s SQL front end too.** Out of scope for
this change and not obviously wanted: that parser serves the browser workbench,
whose tables have no array columns.

## Evidence

Twelve new tests in `crates/slate-serverd/src/lang/pred.rs`; the bin's suite is
28 passing in `lang::pred` and 188 overall, and `sh scripts/check.sh` is 32/32.

Nine mutations through `scripts/mutate.py`, **all nine caught**:

| mutation | caught by |
| --- | --- |
| a list opposite a scalar column is accepted | `an_array_literal_opposite_a_scalar_column_names_both_sides` |
| an element takes the column's type, not the element type | `an_element_of_the_wrong_type_is_refused` |
| a nested list is parsed instead of refused | `a_list_inside_a_list_is_refused_with_the_reason` |
| a `NULL` element is accepted | `a_null_element_is_refused_because_no_stored_row_could_match` |
| an empty list becomes null | `an_empty_array_literal_is_a_value_and_not_a_null` |
| a negative element loses its sign | `an_array_literal_takes_the_element_type_from_the_column` |
| a missing separator is ignored | `a_malformed_list_says_which_way_it_is_malformed` |
| a trailing comma is accepted | the same |

**Two findings came out of the tests rather than the code.**

A negative element did not parse at all. `sizes = [1, -2]` failed with
"expected a literal in a list, found `-`", because the lexer makes `-` its own
token and `array_element` takes one. `operand` does the same two-token dance
one level up; the list loop now does too. An ordinary thing to write, and it
would have reached an operator rather than a test.

The separator refusal was untested in a way that looked fine. The first version
of `an_unclosed_or_malformed_list_points_at_the_bracket` asserted only that
*an* error came back, and the mutation dropping the separator check **survived**
— because the stray element is eaten as a separator and the closing `]` then
looks like a trailing comma, so an error still arrives, with the wrong sentence,
and `!text.is_empty()` is satisfied. The test now pairs each malformed input
with the words its own refusal has to contain, and both separator mutations are
caught. That is the more useful finding: the assertion was not weak in a way
reading it would show.

## What this does not do

**It does not generate an array column.** `scripts/codegen.py` still refuses
one, with the message it already had. That is the remaining half of the gap row
and it is unchanged.

**No `contains`.** Equality and ordering are what the kernel supports on an
array and they are what the syntax reaches. Containment needs an inverted
index and is a separate item.

**`slate-wasm`'s SQL front end has no array literal**, so the browser workbench
cannot write one. Its tables have no array columns, so nothing there can
exercise it either way.

**An element cannot be a placeholder or a column**, by the reasoning above. If
a use for either appears, the refusal message is where to start.

**Nothing writes an array through the TOML `default` beside a predicate in the
same file.** That already worked — `value::from_toml` takes the element type —
and the two syntaxes are deliberately different, because a TOML default is TOML
and a predicate is this language. Nothing checks that they agree about what a
list of strings is; they do, and the agreement is untested.
