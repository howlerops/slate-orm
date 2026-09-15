# A computed value that belongs to the join, not to either side — in the kernel, over the wire, and in a chain

- **Date:** 2026-09-15
- **Author:** Claude (Opus 5), with jacob.beck.018@gmail.com
- **Touches:** `slate-kernel` (`join.rs`, `chain.rs`, `read.rs`), `slate-server`
  (`convert.rs`, `session.rs`), `records.proto` (both copies)
- **Kind:** feature, and three fixes for silently wrong or silently dropped answers

## What changed

`Chain::compute`, the n-table twin of `Join::compute`: values over the whole
accumulated row, appended after every table, readable by the grouping and the
aggregates. `JoinQuery.compute` and `ColumnRef.joined_computed` on the wire, so
a join's computed column is no longer reachable only from the browser binding.
`JoinedRow.computed` beside the inputs, so an *ungrouped* join or chain returns
what it computed.

Three defects found while building it, each of the class this repository keeps
finding: a plausible answer to a question nobody asked.

1. **`Join::compute` validated none of its own column references.** A scalar
   reading `Ordinal(15)` on a six-column joined row returned
   `[Group { key: [Null], values: [U64(24)] }]` — one group, every row in it,
   no error.
2. **An ungrouped join silently discarded `Join::compute` entirely.** The field
   was applied where the grouped path flattens and nowhere else, so
   `left=2 right=4` came back on a join that computed a value, with nothing to
   say it had not.
3. **A grouped chain never validated its grouping ordinals.**
   `validate_grouping` was written for the two-table join in the last change
   and the chain path did not call it.

## Why

The previous entry recorded the wire gap as "the largest thing left open here",
and its reason was precise: `ColumnRef` had no way to name a slot belonging to
the join rather than to an input. That is now a kind of its own,
`joined_computed`, and the distinction it draws is the point — a computed value
of an *input* genuinely has no slot in the joined space, because that space is
packed by declared table width, so the ordinal it would take is the next
table's first column. A value belonging to the *join* does have one, because
the join is what decides where the columns end. Two kinds, two different
refusals, both saying which.

`Chain::compute` had to come first. `JoinQuery` is one message for two and for
n inputs, and adding `compute` to it while chains had nowhere to put it would
have made the field mean different things at different widths.

The three defects were not the task. Each turned up by trying to break what had
just been written, and each is a wrong answer rather than a crash — which is
why they had survived: an out-of-range ordinal and a genuinely null column are
indistinguishable once the answer is a table of numbers.

## Alternatives rejected

**Refusing `Join::compute` on the ungrouped path instead of producing it.**
Much the smaller change, and defensible on the grounds that a `JoinedRow` is
per-input rows and a join-level value belongs to no input. Rejected because the
caller cannot always route around it: a side's own `Query::compute` covers a
value reading one table, and nothing covers one reading both. "Supported when
you group and refused when you do not" is a shape a client cannot predict from
the request it wrote.

**Appending the join's computed values to one side's row.** No new field, no
proto change. It would make the value's position depend on which side carried
it, which is the arithmetic `ColumnRef` exists to remove, and it would make a
row's width stop meaning its table's width — the invariant
`a_join_inputs_computed_values_come_back_beside_its_columns` was written to
hold.

**Letting a side's `Query::compute` work on the grouped path by widening
`JoinSchema` so each side carries its own computed slots.** Rejected last time
and still rejected: the right table's ordinals would shift by the left side's
computed count, silently re-pointing every existing caller.

**Keeping `flatten_computing(schema, compute)` and letting the cursor compute
separately.** Two evaluation sites for one rule, including the null rule for an
absent side. They would have drifted the first time one of them learned
something. Now `computed_values` is the only place either produces them, the
cursors call it, and `flatten_appending` appends what is already there. The cost
is one extra `flatten` per row on the grouped path when a computed column is
present — the cursor flattens to compute, the grouper flattens to group. That is
a real regression on that path and it is not measured; the alternative was two
copies of the null rule.

**Validating compute references only in the kernel.** The kernel does validate
them now, and that is the check that matters. The wire validates too, because
the kernel's message names a flat ordinal the client never wrote: a client sent
`joined_computed 3`, and "Ordinal(19) is outside the 16 columns" is a sentence
about a number it has never seen.

## Evidence

**The three defects, before the fix.** Defect 1: a probe grouping by a compute
reading `Ordinal(width + 9)` returned exactly one group, `key: [Null]`, 24 rows.
Defect 2: an ungrouped join with one computed value returned `left=2 right=4` —
the two tables' declared widths, nothing else. Defect 3: `GROUP BY` an ordinal
five past a chain's width was accepted.

**Nine mutations on the kernel, each caught by a named test.** Never firing the
compute validation; admitting a self-reference (`columns + index + 1`, which
still refuses a *later* reference — caught only by
`a_computed_value_naming_its_own_slot_is_refused`, which exists because this
mutation survived the first round); the ungrouped join dropping compute again;
computed values coming back reversed; a chain dropping compute; a chain
appending only the first value; a chain skipping compute validation;
`narrowed_chain` forgetting what compute reads; and the grouped chain dropping
its grouping validation.

**Ten on the wire, each caught.** The server dropping a join's or a chain's
compute; `joined_computed` resolving to a bare ordinal; its range check never
firing; a compute permitted to read itself; `unresolve` losing the kind (which
broke the round trip); the response dropping the values; `group_by` unable to
see them; and `MultiCursor` dropping either shape's.

**Two mutations survived the first pass and are worth recording.** One was the
self-reference above. The other found that the grouped-chain oracle was
comparing the code against itself: its fold flattened through
`flatten_appending`, the function under test, so truncating the computed values
truncated both sides identically and the property passed. The oracle now
flattens and computes the two values in the test file, independently of
`Scalar` down to the null rule.

**The oracles.** `a_grouped_chain_with_a_computed_column_agrees_with_folding_it`
runs the sixteen combinations of two steps' join types with the group key drawn
from the computed slots. Over gRPC, `a_joins_computed_values_come_back_over_the_wire`
and `a_chains_computed_values_come_back_over_the_wire` compare a socket against
the kernel in process from the same value, and
`a_grouped_join_can_group_by_the_joins_computed_value` does it for the grouped
path — the query the last entry recorded as not expressible.

**Suites:** `slate-kernel` (40-odd binaries), `slate-server` (12), `slate-orm`,
`slate-wasm`. Clippy clean on both changed crates with `--all-targets`.

## What this does not do

**The extra flatten on the grouped path is unmeasured.** A grouped join or chain
with a computed column now flattens each row twice. It only affects reads that
use a computed column, which is a feature two days old, and the benchmark
harness has no case for it.

**`JoinInput.having` still cannot read a computed value**, deliberately, and now
with a better reason available in the message: the pair is admitted before the
values are produced from it. `AggregateQuery.having` reads them through the
group.

**Go and TypeScript still cannot build any of this.** They have no `Scalar`
surface at all. The conformance runner therefore still cannot compare the three
SDKs on a computed column, joined or otherwise.

**The wasm binding's `JoinSpec` still pins compute to the left table and
aggregates to the right.** The kernel and the wire are now both wider than the
browser's own spec, which is the reverse of the position this started in.

**Nothing reads `JoinedRow.computed` yet except the tests.** The Python client
decodes rows through a path that does not look at it, so an ungrouped join's
computed values arrive and are dropped by every shipped client.
