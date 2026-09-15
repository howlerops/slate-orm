# HAVING reaches the parser, and its literals are typed from the aggregate rather than the column

- **Date:** 2026-09-15
- **Author:** Claude Opus 5, at Jacob's direction
- **Touches:** `crates/slate-wasm/src/sql.rs`, `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/having.rs`, `site/workbench.js`, `site/docs.html`, `site/check/workbench.py`
- **Kind:** feature

## What changed

`HAVING <group-cond> (AND <group-cond>)*` between `GROUP BY` and `ORDER BY`,
lowered onto `Grouping::having`, which the kernel has had all along and nothing
could reach. The left side is a select item resolved into *group* space —
`[keys..., aggregates...]` — through the same function `ORDER BY` uses, so both
clauses agree on what `count(*)` refers to and both refuse an aggregate the
select list does not compute. The operator half is `WHERE`'s, extracted into one
`comparison_tail` so the two clauses cannot drift into accepting different
operators.

The part that is not bookkeeping is the literal's **type**, which comes from the
aggregate and not from the column it reads. That is a five-line function and the
reason this entry is long.

## Why

It was the one construct missing from "everything the grammar has" — the
kitchen-sink example said so in a comment, and three ledger entries recorded it
as a gap. `HAVING count(*) > 300` is also the natural second question after
`GROUP BY`, and answering it previously meant reading all 226 groups and
squinting.

## Alternatives rejected

**Lower `HAVING` onto the query's filters when it only names group keys.** For
`HAVING pickup_zone < 150` this is not merely equivalent, it is *better* — as a
`WHERE` it can become a scan bound and skip rows instead of grouping them and
throwing the groups away. Rejected because the rewrite is only valid for the
subset that touches no aggregate, so the clause would sometimes be a scan bound
and sometimes a post-filter with no way for a reader to tell which; and because
`EXPLAIN` would then show a bound the query never asked for. A reader who wants
the bound can write `WHERE`, and the grammar makes that the obvious thing.

**Give `HAVING` its own condition parser.** Two functions, each simple, no
shared parameter threading. Rejected: it is exactly how `WHERE` comes to accept
`ILIKE` and `HAVING` does not, six months from now, for no reason anyone can
reconstruct. The shared `comparison_tail` costs one indirection.

**Type the literal from the column, as `WHERE` does.** The obvious
implementation, and wrong — see below. Rejected once it was clear that being
wrong here produces no error at all.

**Compute an aggregate a `HAVING` names but the select list does not.** SQL
proper allows it. Rejected: the group would then be filtered on a number the
reader cannot see in the answer, which is the same reason the select list
refuses a non-key column rather than dropping it.

## Evidence

The hazard first. `Value` orders by *class* before magnitude:
`class_rank` puts `I64` and `U64` together at 4 and `F64` at 5, so a float
against an integer is decided by the ranks and never looks at the numbers.
`F64(0.5) > I64(1000000)` is **true**. Meanwhile `avg` returns `F64` however
integral its column is, and `sum` returns `I64` for an integer column and `F64`
only for a real one.

So `HAVING avg(duration) > 1500` over an `I64` column, with the literal typed
from the column, admits **every group** — 226 of 226 — with nothing logged and
no error raised. Typed from the aggregate it admits 110. The same bug pointing
the other way, typing every `sum` as a double, makes `HAVING sum(duration) >
3000000` admit **none**, because `I64(x) > F64(3000000)` is false by rank.

Seven mutations, each restored and re-verified, plus a no-op control:

| mutation | caught by |
|---|---|
| `avg` typed from its column | `an_integer_literal_against_a_double_aggregate_is_not_a_free_pass` |
| every `sum` typed as a double | `a_sum_over_an_integer_column_stays_an_integer` |
| `count` typed as a double | three tests, `it_keeps_only_the_groups_that_pass` first |
| the parsed `HAVING` never applied | six of eight |
| the error names ORDER BY on a HAVING | `it_refuses_what_it_cannot_answer` |
| group-space off-by-one in the type lookup | six of eight |
| aggregate ordinals collide with the keys | six of eight |
| *(control)* a `let _ =` that changes nothing | nothing — all eight stayed green |

The first row is the one worth reading twice. **The test written to catch it
did not**, first time: it used `avg(total)`, and `total` is already an `F64`, so
the column's type and the aggregate's agree and the mutation sailed through.
The disagreement is the test, so it moved to `avg(duration)` over an `I64`
column. That is the second time in two days a test has had to be rewritten
because the mutation it existed for survived it, which is an argument for
mutating before believing a test rather than after.

Four browser assertions as well, because the clause crosses the wasm boundary
through a new `QuerySpec` field and the Rust tests on either side would both
pass with it dropped in the middle: 226 zones before, 12 after, every survivor
verified against both conjuncts, the integer-literal case still filtering to
110, and the no-`GROUP BY` refusal reaching the page. 47 checks in that file
now, all green; 80 in `slate-wasm`.

## What this does not do

**No `HAVING` on a join or a chain.** `Join` and `ChainStep` have a `having`
field of their own, and it means something else there — a condition over the
joined *row*, not over a group — so the name colliding is a trap rather than a
head start. The grouped-join path takes a `Grouping` and could carry this, but
the join grammar has no `ORDER BY` either and adding one clause without the
other would make the join path a strange subset.

**No `OR`, in `HAVING` as in `WHERE`,** and refused with a message that says so.

**No aggregate on both sides.** `HAVING count(*) > avg(passengers)` does not
parse; the right side is a literal, exactly as in `WHERE`. `Expr` has
`compare_columns` and the group is a row, so this is reachable — it was left out
because nothing asked for it and an untested path is worse than a refusal.

**The planner does not use `HAVING` for anything.** It is applied after the
groups are built, and the estimated row count in the plan is the count *before*
it — so a query returning 12 groups may show an estimate of 226. That is honest
about what the planner knows and will read as a bad estimate to anyone who does
not know why.
