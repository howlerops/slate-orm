# `IN (SELECT …)` in the browser's SQL front end, and named refusals for EXISTS, UNION and everything correlated.

- **Date:** 2026-09-16
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-wasm` (`sql.rs`, `lib.rs`, `tests/subqueries.rs`,
  `tests/taxi.rs`, `tests/sql.rs`), `site/workbench.js`, `site/index.html`,
  `site/docs/features.html`, `site/docs/limits.html`, `README.md`
- **Kind:** feature

## What changed

`WHERE author_id IN (SELECT id FROM authors WHERE country = 'US')` compiles and
runs. The parser builds the inner `SELECT` into a `QuerySpec` and hangs it off
the `FilterSpec`; the binding runs it once, before the outer query, and turns
its single column into that filter's `values`. What reaches the planner is an
ordinary `Expr::In`, so the existing handling — point gets on a key, a range on
an index, a hash set on a scan — does all the work and nothing about subqueries
reached the kernel. The spec keeps both halves, so the Spec tab shows the
subquery as written beside the candidates it produced.

Three things are refused by name rather than left to fail as syntax errors:
`EXISTS` and `NOT EXISTS`, the three set operators, and a candidate type that
could never equal the outer column. A correlated subquery needs no refusal of
its own — the inner query is parsed against the inner table, so a column of the
outer one is already "no such column" there.

Two things outside the feature came with it. `tests/sql.rs` had not compiled
since `FilterSpec` grew its new fields, so the round-trip property suite had
been silently absent; it is fixed and now also generates literal `IN` lists.
And the Spec panel elides a candidate list longer than twenty entries, with the
real length in the marker.

## Why

`IN (SELECT …)` is the subquery people actually write, and it was the one shape
that needed no new kernel operator — `Expr::In` was already there, already
planned, already three-valued. The whole feature is *where the list comes
from*, which made it a parser and binding change rather than a database one.

The type check is the part that was not obvious. `title IN (SELECT id FROM
authors)` is an error nowhere downstream: the binding renders each candidate to
text and parses it back against the outer column's type, and `1` parses
perfectly well as the string `"1"`. So the query would have returned zero rows,
with no complaint, indistinguishable from a fact about the data. That is the
worst answer this front end can give, and it is exactly the class of failure
the rest of the parser is written to avoid.

## Alternatives rejected

**Implement `EXISTS` by decorrelating it.** `EXISTS (SELECT 1 FROM b WHERE b.fk
= a.pk)` is exactly `a.pk IN (SELECT fk FROM b)` — the rewrite is sound, nulls
included, because a null `fk` equals nothing and a null `a.pk` is excluded
either way. It was rejected on cost against benefit, and the benefit is the
weaker half: every decorrelatable `EXISTS` has an exact `IN` form, so refusing
it costs the reader a mechanical rewrite that the message spells out. The cost
is a correlation-aware column resolver (`condition` currently takes one table
and would need to try inner then outer), a rule for which side of the `=` is
which, and a scoping rule for an unqualified name that resolves on both — three
places to be subtly wrong in a feature whose payoff is syntactic. Written down
here because it is a real option and the next person should not have to
re-derive that it is sound.

**Cap the candidate list.** The obvious worry is an unbounded list: the inner
query's answer becomes the outer query's `IN`, so `id IN (SELECT id FROM
trips)` is a hundred thousand candidates. Measured instead of assumed, and the
measurement said no — see below. A cap chosen without that number would have
refused working queries to prevent a problem that is not there.

**Truncate the spec itself rather than its rendering.** The Spec panel's whole
claim is that it shows the structure the three clients send, so cutting the
value that goes on the wire to make the panel shorter would break the one thing
it is for. The elision is in the renderer, is labelled with the real length,
and the spec the binding returns is untouched — which is also what the tests
assert against.

**Refuse mixed integer widths along with genuinely incomparable types.** Strict
type equality is a simpler rule and would have refused `u64 IN (SELECT an_i64
…)`, an ordinary thing to write that works: the text round-trip handles it, and
a negative candidate against a `u64` column already fails loudly at run time
with the offending value in the message. The rule is "same type, or both
numeric" for that reason.

**Lower `UNION` by running both halves and concatenating.** The workbench could
do it — it already runs several statements from one buffer. The kernel could
not: deduplication across two independently planned statements has nowhere to
happen, so `UNION ALL` would work and `UNION` would quietly be `UNION ALL`.
Offering a SQL keyword that the database does not have, in the one front end
whose job is to compile to the wire spec, is the drift this parser exists to
prevent.

## Evidence

**Tests.** 23 in `crates/slate-wasm/tests/subqueries.rs`, one added to
`tests/taxi.rs`, and the `tests/sql.rs` round-trip property now covers literal
`IN` lists. The answer oracles recompute the expected set from
`slate_wasm::fixture` rather than naming a row count, because the fixture's
generated tail is a constant somebody may reasonably change.

**Mutation testing.** Eighteen mutations over `resolve_subqueries`,
`comparable`, `subquery`'s type reporting and its four inner refusals, the
`EXISTS` branch, the set-operator branch, and the `statement` call site. All
eighteen were caught by a named test; no survivors. Notable pairings: dropping
the null filter is caught only by `a_subquery_drops_its_null_candidates` (the
taxi fixture is the only one with nulls, and the SQL front end has no `NULL`
literal, so it is unreachable from the books fixture); resolving only
`filters[0]` is caught only by
`a_second_subquery_in_the_same_where_is_resolved_too`, which was written
because that mutation had nothing to catch it.

One of the eighteen was almost a false pass, and it is the failure mode of
mutation testing by string replacement. `if matches!(item,
SelectItem::Aggregate { .. }) {` occurs three times in `sql.rs`, and a
first-occurrence replace hit the `DISTINCT` one instead of the `IN` one —
which a `distinct.rs` test duly caught, so the run reported the mutation as
killed while the check under test was untouched. The tell was the name of the
catching test: it should have been `an_aggregate_inside_in_is_one_value_not_a_list`
and it was not. Re-applied by line number, that test fails as it should.
Worth writing down: "a mutation was caught" means nothing without checking
*which* test caught it.

**A test that was wrong before it was right.** Three assertions in the first
draft of `subqueries.rs` were wrong about the code rather than the code being
wrong about them: `plan` is an object and not a string; a projection does not
narrow the returned row, so `SELECT title` still arrives four wide and `row[0]`
is `id`; and an empty `values` is *absent* from the serialised spec rather than
`[]`. A fourth asserted that `IN` on `books.author_id` would plan as an index
scan — it plans as a table scan with an `InSorted` residual, which is the
correct costing on this fixture, so the assertion moved to the residual.

**Large candidate lists, measured rather than guessed.** Native release build,
100,000-row taxi fixture, one run each, so these are orders of magnitude and
not a spread:

| query | candidates | time |
| --- | ---: | ---: |
| `pickup_zone IN (SELECT id FROM zones)` | 265 | 40 ms |
| `passengers IN (SELECT passengers FROM trips)` | 95,337 | 166 ms |
| `id IN (SELECT id FROM trips)` | 100,000 | 141 ms |
| `pickup_zone IN (SELECT pickup_zone FROM trips)` | 100,000 | 160 ms |

Nothing degrades: a hundred thousand candidates against a hundred thousand rows
is a sixth of a second. The hypothesis that a large list would need a cap is
withdrawn.

What it *did* find is a rendering problem: the same specs serialise to 1.2–1.7
MB of pretty JSON, which is what the Spec panel puts in one `<pre>`. Hence the
elision, at twenty values.

**The check that had stopped running.** `cargo test -p slate-wasm --test sql`
failed to compile — `FilterSpec` grew `values` and `subquery`, and the
proptest generator still used a struct literal with three fields. It had been
in that state since the fields were added, which means the spec round-trip
property was not running at all and would have gone red in CI. Fixed with
`..FilterSpec::default()`, and the generator now emits `IN` lists too.

## What this does not do

**One level, and no depth check.** Nested subqueries are not refused by the
parser and would recurse in `resolve_subqueries`. One level is what the grammar
can produce today; the comment on that loop names it as the line that becomes a
depth limit if that changes.

**Nothing crosses the wire.** By the time a spec leaves the tab the subquery is
already a resolved list, so the head node and the three clients see an ordinary
`IN` and know nothing about any of this. The `subquery` field rides along for
display and is ignored everywhere but the browser.

**No `EXISTS`, no set operators, no correlation** — refused, with the reasoning
above and in the messages themselves.

**The type check skips computed items.** `IN (SELECT hour(t) FROM …)` reports
no candidate type, because the result type is the function's rather than the
column's and a second table mapping functions to types is not worth it for
this. A mismatch there is still caught at run time, with the offending value in
the message, which is the weaker form of the same check.

**The elision is untested by the Rust suite**, because it is in
`site/workbench.js`. The browser check exercises the panel; it does not
currently assert the marker, and a subquery large enough to trigger it is not
one of the workbench's examples.
