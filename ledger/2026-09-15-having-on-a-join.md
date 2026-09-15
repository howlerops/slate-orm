# HAVING on a join and a chain, and a lowering that was never applied

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/rust-orm-record-layer-gswxlu`
- **Touches:** `crates/slate-wasm/src/{sql,lib}.rs`, `crates/slate-wasm/tests/taxi.rs`
- **Kind:** feature

## What changed

`SELECT borough, count(*) FROM trips JOIN zones ON ... GROUP BY borough HAVING
count(*) > 5000` runs. `JoinSpec` and `ChainSpec` gained `having:
Vec<FilterSpec>` whose `column` is a group-space ordinal, the parser reads the
clause where `select` reads its own, and `group_value_type` walks a slice of
tables so a stored key's type resolves through a *joined* ordinal.

`join_group_ordinal` now takes the clause name, so a refusal about an
uncomputed aggregate says HAVING when the reader wrote HAVING.

## Why

The single-table path has had HAVING since task #39. The joined path never did,
and the reason was one signature: `group_value_type(ordinal, keys, aggregates,
table)` took a single `TableDef`, so it could resolve `keys[i]` to a column type
only when the key was an ordinal of that one table. A group key on a join is an
ordinal of the *joined* row, and which table it lands in is the sum of the
widths before it.

Nothing else blocked it. `Join::having` and `Chain::having` are `Expr` fields
the kernel already evaluates over the group; `Grouping::having` is how the
single-table path reaches them. The whole feature is the walk, a clause in the
parser, and two call sites.

This also finishes what task #146 started. The workbench's "kitchen sink"
example was two statements because a second group key and an ORDER BY were not
available on a join; those landed in #141 and #146, and HAVING was the last
reason the note could have been true.

## Alternatives rejected

**Pass the joined `TableDef`s only where a join needs them**, leaving the
single-table call unchanged. Two signatures, and the single-table one is the
one-input case of the other — which is exactly the split that left the joined
path without HAVING in the first place. `&[table]` at the single-table call
site is one bracket pair and keeps one implementation.

**Resolve a HAVING term against the joined row rather than the group.** It would
let `HAVING borough = 'Queens'` mean the *column* rather than the key. Rejected
because that is a WHERE — it filters rows, not groups, and the kernel would
evaluate it after the fold where the column no longer exists. The refusal for
an ungrouped HAVING says so and suggests WHERE.

**An `#[allow(clippy::too_many_arguments)]` on `join_group_ordinal`**, which
the `clause` parameter pushed to eight. Rejected for `GroupSpace`: the keys, the
aggregates and the computed list travel together at every call because a group
is `[keys..., aggregates...]` and a key may be computed, so a signature that
names them separately is one that can be handed two of the three from one query
and the third from another. Bundling them is the fix the lint was pointing at.

**Skip the `clause` parameter and leave the message saying "ORDER BY".** It is
one `&str`. The single-table `group_ordinal` already takes one, and its comment
says why: an error naming a clause the reader did not write sends them looking
at the wrong line. Copying the function without copying the reason would have
been the third time this repository made that mistake.

## Evidence

Four tests in `crates/slate-wasm/tests/taxi.rs`, over the real 100,000 trips.
Two are differentials against the *unfiltered* query, filtered in the test —
not a written-down list of boroughs, so a different sample moves both sides.

- `having_keeps_exactly_the_groups_that_pass` asserts the threshold filtered
  something and not everything before comparing, so a threshold that stopped
  biting could not make the test vacuous.
- `having_ands_its_terms_over_different_aggregates` asserts that *each* term
  excludes a group the other keeps. That guard fired on the first attempt:
  `count(*) > 100 AND avg(total) > 20` left the count term doing all the work
  (5 of 5 by count, 7 by average), so the AND was not being tested at all. The
  thresholds are now `> 100` and `> 30`, where EWR has ten trips at a high
  average and Manhattan has ninety thousand at a low one, and each term drops
  one of them.
- `having_works_on_a_chain_and_on_a_group_key` runs it over three tables with
  two aliases.
- `having_is_refused_where_it_cannot_mean_anything` covers three refusals,
  including that the uncomputed-aggregate message contains "HAVING" and *not*
  "ORDER BY".

`cargo clippy --workspace --all-targets` is clean and every suite in the crate
is green.

### The lowering that was never applied

Worth writing down because the build was green throughout. The edit that adds
`grouping.having(...)` to `joined_spec` and `chained_spec` was written as a
two-edit script whose *second* anchor did not match. The script asserts before
writing, so neither edit landed — and the failure looked like the *next* error
in the queue (`sql.rs` missing a struct field), which I fixed and moved on from.

The result compiled, parsed the clause, stored it in the spec, serialised it to
the Spec tab, and filtered nothing. `HAVING count(*) > 5000` returned all eight
boroughs, including the one with ten trips.

No test would have caught it at that moment, because the tests were not written
yet — it was caught by running the query by hand and reading the answer, which
is the only reason to do that before writing the assertions. The lesson is
about the tooling: a patch script that aborts partway leaves a tree that
compiles and is wrong, so a multi-edit script has to be read as all-or-nothing
in *both* directions, and "it builds" is not evidence that an edit applied.

## What this does not do

**No mutation testing yet.** Three of the last four changes had a mutation pass
find a real missing test; this one has not had one. Given how the lowering
failure above went, the mutation worth running first is the obvious one —
delete the `having` call — and I would expect the two differentials to catch it.

**HAVING still takes no computed column.** `HAVING hour(t) > 3` on a grouped
join resolves through `join_group_ordinal`, so it works when `hour(t)` is the
group key and is refused otherwise, which matches the single-table path.

**`OR` is still refused in HAVING**, for the reason it is refused in WHERE: the
spec ANDs its conditions and the kernel's `Expr::Or` is unreachable from any
front end. That is a wider gap than this entry — the kernel has had `Expr::Or`
since expressions arrived, and no client, no spec and no parser can produce one.
