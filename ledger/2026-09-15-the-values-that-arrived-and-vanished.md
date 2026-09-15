# Every client can now read back every kind of computed value; two of the four kinds arrived on the wire and were dropped

- **Date:** 2026-09-15
- **Author:** Claude, working from "I don't want any gaps. Please address those"
- **Touches:** `clients/python/src/slate/rows.py`, `clients/go/slate/client.go`, `clients/typescript/src/client.ts`, and one test in each suite
- **Kind:** fix

## What changed

Three shapes come back from a read, and each keeps its parts separate: a `Row`
has stored columns and its own computed values, a joined row has one row per
input plus the values the *join* computed, and a group has keys and aggregates.
Two of those slots were being filled by the server and thrown away by a client:

- **Python** read a joined row's `inputs` and ignored its `computed`, so a
  caller who declared `JoinQuery.compute` and then iterated rows (rather than
  grouping by the value) got a joined row that looked complete and was not.
  `JoinedRow` now carries them, with `computed_values` and `computed(i)`.
- **Go and TypeScript** decoded each join input with the row decoder, which
  takes a row's `values` and drops its `computed` — right for a stored column,
  and it meant an *input's* own computed value arrived and vanished. Go gains
  `JoinStream.InputComputed(input)`; TypeScript gains `inputComputed` on
  `ComputedJoinedRow`.

Neither was a wire change: both values were on the wire already, in
`JoinedRow.computed` and in each input's `Row.computed`.

## Why

Sending is not reading back, and every existing test for these values took the
*other* path. All three clients had tests that declared a join-level computed
value and then **grouped by it** — so the value came back as a group key, and
the row path was never exercised. An input-level computed value had tests too,
on a single-table query, where it is the only input and the row decoder is a
different one. The gap was exactly the intersection: a computed value on a
joined *row*.

The failure mode is the quiet kind. No error anywhere: the request is valid,
the server evaluates the expression, the value is serialised, and the client
returns a row without it. A caller would conclude the feature does not work and
have nothing to look at.

## Alternatives rejected

**Append each input's computed values to that input's value array**, so
`inputs[1]` is columns-then-computed and no new accessor is needed. Rejected
for the reason the wire separates them, which is written up in `rows.py`: a
caller indexing past the table's own columns would silently get a computed
value and read it as a column, and adding a column to the table re-points every
one of them. That is protocol finding 4, and putting it back in the client
would be reintroducing it one layer up.

**Return an input's computed values inside `Computed()` / `computed`,
concatenated after the join's.** One accessor, and it conflates two things that
are not the same kind: a join's computed value is evaluated over the whole
accumulated row and may read every input, while an input's reads only that
input's table. Their *positions* would then depend on how many inputs there
are and what each computed — which is the arithmetic `ColumnRef` exists to
remove, on the response side.

**Leave Python's `JoinedRow` alone and tell callers to group by the value.**
That is what the existing tests happened to do, and it is a real workaround for
some queries and not for others: a join's computed value on an ungrouped row
stream has no other way out.

**Make the Go accessor take an `int`.** It took one in the first draft and the
test would not compile: every other join accessor takes the `uint32` handle
`Add` returns, so an `int` parameter would be the only place a caller converts.
Changed to `uint32`, which also removes the negative-index branch.

## Evidence

**One new test per suite**, each declaring all three kinds at once on the same
join — an input-level computed value on each side and a join-level one reading
both — and checking each against the stored columns it was computed from. That
last part is what distinguishes a value that survived from one that happened to
be the right shape: the upper-cased name is compared to `name.upper()`, the
decade to `year // 10 * 10`, and the join's to `f"{name}/{title}"`.

**Mutation testing, one per client, each restoring the bug:**

| mutation | caught by |
| --- | --- |
| Python: `JoinedRow.from_proto` drops `wire.computed` again | `test_both_kinds_of_computed_value_come_back_on_a_join` |
| Go: `perInput` filled with `nil` instead of `computedFromProto` | `TestBothKindsOfComputedValueComeBackOnAJoin` |
| TypeScript: `inputComputed: inputs.map(() => undefined)` | `both kinds of computed value come back on a join` |

Each failed with a message naming what was missing rather than a generic
mismatch — Python's was `where () = JoinedRow(...).computed_values`.

**Suites, against a prebuilt `slate-serverd` / `slate-testserver`:** Python 159
passed (153 before these), Go `./...` ok, TypeScript 64/64.

**A real API wart found by writing the tests.** An input's own `compute` has to
*name* that input: `Col(3)` means input 0, so on input 1 the server refuses it
— "computed value 0 names input 0, and is evaluated over input 1" — rather
than quietly reading the wrong table's fourth column. That refusal is good and
the ergonomics are not: the handle `Add` returns is not available inside the
literal that needs it, so both tests write the index down as a constant and
then assert that `Add` returned it. The awkwardness is in the tests on purpose,
where it is visible.

## What this does not do

**A chain's computed value is untested from any client.** `Chain::compute`
exists in the kernel and is reachable over the wire, and no client suite
declares one — that is its own task, not this one.

**No test declares a computed value on an outer join's unmatched side.** Both
new accessors return nil/`undefined` there, which is the documented behaviour
and is reasoned about rather than demonstrated: the input produced no row, so
it computed nothing. The fixtures here are inner joins.

**Python has no per-input accessor, because it needs none** — each input is a
`Row` and carries its own `computed_values`. Go and TypeScript return arrays of
values rather than row objects, which is why they needed a new method. The
asymmetry is in the clients' shapes, not in what they can express.

**Nothing prevents the next shape from being dropped the same way.** The guard
that would have caught this is a test per client that reads back every kind of
value a request can produce, generated from a list of the kinds; what exists
now is one hand-written test per client covering the three kinds that exist
today.
