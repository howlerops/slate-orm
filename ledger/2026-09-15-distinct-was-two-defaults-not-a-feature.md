# SELECT DISTINCT turned out to be two defaults in the way, not a missing feature

- **Date:** 2026-09-15
- **Author:** Claude Code
- **Touches:** `slate-wasm` (`sql.rs`, `lib.rs`), `slate-server` (`convert.rs`),
  `slate-kernel/tests/aggregate.rs`, the three clients, `site/workbench.js`,
  `README.md`, `site/docs.html`
- **Kind:** feature

## What changed

`SELECT DISTINCT a, b` now parses and answers, on a single table and on a join
or chain, and a grouping with keys and no aggregates now crosses the wire — so
it is reachable from Python, Go and TypeScript too, each with a test.

Nothing was added to the kernel. A `Grouping` with keys and no aggregates
already yields exactly the distinct combinations in key order; two new tests in
`slate-kernel/tests/aggregate.rs` say so, which turns that from an argument into
a fact the front ends can lower onto. `DISTINCT` is therefore a parse and a
lowering: the selected items become the group keys, through the same
`value_ordinal` / `join_value_ordinal` find-or-add that `GROUP BY` uses, so a
computed column registers once and a column named twice is one key.

What actually had to be removed were two defaults that made an empty aggregate
list mean something else:

- the browser binding turned it into `count(*)` in three places, with matching
  headers in `labels` and `joined_labels`;
- the head node refused it outright — "an aggregate request with no aggregates
  is a query; use Query".

Three refusals are new, each saying why: `SELECT DISTINCT *`, `DISTINCT` beside
`GROUP BY`, and `DISTINCT` over an aggregate.

## Why

The request was for `DISTINCT`, which `GROUP BY` over the same columns already
answers. What made it interesting is that both defaults were already returning
wrong answers without anybody asking for `DISTINCT`.

The binding's default meant `SELECT author_id FROM books GROUP BY author_id`
came back **two columns wide**, the second a `count(*)` the query does not
mention. Not a formatting detail: the header grew a column and so did every row,
so a caller reading `row[1]` got a number that is not in their query. The
default dated from when this was a panel with a group-by picker and no aggregate
picker, where a grouping with nothing to compute had no meaning — a reasonable
guess then, made in the one place a caller could not override.

The server's refusal was right about the case it was written for and wrong about
this one. No keys *and* no aggregates asks for one group with nothing in it, and
should be refused. Keys and no aggregates is `SELECT DISTINCT`, and a client
that wanted it had to request a `count(*)` and throw it away.

`SELECT DISTINCT *` is refused rather than answered, which is the one place this
does less than SQL. Every table here has a primary key, so the rows are already
distinct: answering it would read every row, hash all of it, and return exactly
what it was given. The refusal says that, which is more useful than the slowest
possible way to do nothing.

## Alternatives rejected

**A `Distinct` node in the query spec, or a `distinct: bool` on it.** This is
the shape most engines have, and it would have been a second way to say what
`GROUP BY` already says — with its own planning, its own `EXPLAIN` rendering,
its own interaction with `HAVING` and `ORDER BY`, and its own bugs in each. The
lowering reuses every one of those paths, which is why `SELECT DISTINCT a
... ORDER BY a DESC LIMIT 3` and `... HAVING a > 5` worked with nothing written
for them. The cost is that the Spec tab shows a `groupBy` where the reader wrote
`DISTINCT`; that is visible rather than hidden, and the example on the site
points at it.

**Keeping the implicit `count(*)` and adding a flag to suppress it.** Smaller
diff, and it would have left `SELECT author_id ... GROUP BY author_id` returning
a column the query does not mention — a wrong answer kept for compatibility with
a UI that no longer exists. Removing it broke nothing: the whole `slate-wasm`
suite (161 tests) and the three client suites pass, because every test that
wants a count asks for one.

**Leaving the server's refusal alone and calling DISTINCT a browser feature.**
This is what the first draft did, and the README would have gained another "does
not cross the wire" entry beside the four already there. But unlike pagination,
conditional writes and decimals, this one needed *no protocol change at all* —
the `group_by` and `aggregates` fields already exist and `repeated` is already
allowed to be empty. A refusal was the only thing in the way, and narrowing a
refusal that is wrong is not a protocol change.

**Supporting `SELECT DISTINCT *`.** Expressible — group by every column — and
deliberately not done, per the reasoning above.

## Evidence

Kernel, first, because everything else rests on it:
`a_grouping_with_no_aggregates_is_the_distinct_keys` and
`distinct_over_two_columns_is_the_combination`. The second is built so the wrong
implementation is visible: `region` cycles every 3 rows and `amount` every 10,
so the combination is 30 and a per-column lowering would give 10.

`crates/slate-wasm/tests/distinct.rs`, 15 tests, oracles counted from the
fixture rows rather than written down — the fixture's generated tail can change,
and a hand-written number would then fail as though DISTINCT were broken.
Observed: 8 countries, 72 years, 4824 distinct `(author_id, year)` pairs over
4824 books, each agreeing with a set built from `fixture::book_rows()`.

**Mutation testing**, 8 mutations:

| mutation | outcome |
|---|---|
| `let distinct = self.eat("distinct")` → `false` | caught, 4 tests |
| single-table key dedup deleted | caught, `a_column_named_twice_is_one_key` |
| `DISTINCT *` refusal disabled | caught |
| `DISTINCT` + `GROUP BY` refusal disabled (both paths) | caught, 2 tests |
| `DISTINCT` over an aggregate refusal deleted, single table | caught |
| `DISTINCT` over an aggregate refusal deleted, join | caught |
| implicit `count(*)` restored in `aggregates` | caught |
| implicit `count(*)` header restored in `labels` | caught, 3 tests |
| **join key dedup deleted** | **SURVIVED** |

The survivor was real and is the entry worth keeping: every other join test
names each column once, so nothing exercised the join path's own copy of the
dedup. `a_column_named_twice_on_a_join_is_one_key` was written for it, and the
mutation is caught now. Reasoning that "the join path is the same code" would
have missed it — it is a separate copy.

On the wire: `keys_with_no_aggregates_are_the_distinct_combinations` in
`slate-server/tests/multi.rs` compares the streamed groups against the kernel
answering the same grouping directly, rather than a written-down count, and
asserts each group carries no values. Mutating the new guard back to `||`
(the old behaviour) fails it.

Clients, all run locally against a real `slate-serverd`, not only in CI:

- Python `165 passed` — including a **pre-existing test that had to be
  replaced**: `test_a_grouped_join_needs_at_least_one_aggregate` asserted the
  refusal this change removes. It is now
  `test_a_grouped_join_with_no_aggregates_is_the_distinct_keys` plus a test for
  the refusal that remains.
- Go, whole suite, with the two new tests confirmed by name under `-run -v`.
- TypeScript, `68 pass`, both new tests confirmed by name.

Also: `cargo test -p slate-kernel` (50 binaries), `-p slate-server` (15),
`-p slate-wasm` (161 tests), `cargo clippy --workspace --all-targets` clean, and
`site/check/workbench.py` against a wasm bundle rebuilt for this change.

## What this does not do

**No `DISTINCT ON`**, and no `count(distinct ...)` change — that aggregate
already existed and is a different thing, which the refusal message points at.

**`SELECT DISTINCT *` is refused**, per above. If a table without a primary key
ever becomes possible, that refusal becomes wrong and should be revisited rather
than kept.

**No client-side spelling of it.** Python, Go and TypeScript can send a grouping
with no aggregates, and that is all: there is no `.distinct()` builder, so a
caller writes `group_by(...)` and omits `aggregate(...)`. The three doc comments
now say that is legal, which is the whole of the client-side change.

**The workbench's Spec tab shows a `groupBy`** where the reader wrote
`DISTINCT`, because that is what it lowered to. Honest, and the example's own
comment says so, but it is the one place the abstraction is visible.

**Nothing was measured.** DISTINCT costs what the equivalent `GROUP BY` costs,
which is already characterised; no separate number was taken, and none is
claimed.
