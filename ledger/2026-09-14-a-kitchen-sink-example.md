# A kitchen-sink example, and a test that runs every example the page offers

- **Date:** 2026-09-14
- **Author:** Claude (agent session), at the request of the repository owner
- **Touches:** `site/workbench.js`, `crates/slate-wasm/tests/examples.rs` (new),
  `site/check/workbench.py`
- **Kind:** feature

## What changed

A tenth example in the workbench sidebar, **Kitchen sink**: two statements
using every construct the grammar has.

The first is a grouped join with its conditions split across both sides. The
second is everything else at once — six conditions over three value types
including a `LIKE` pattern and a regular expression, two group keys, seven
aggregates (`count(*)`, `count(column)`, `count(distinct)`, `min`, `max`,
`sum`, `avg`), ordered by an aggregate descending and then by a key, with
`LIMIT` and `OFFSET` over the groups.

And a test that reads the example list **out of `site/workbench.js`** and runs
every query in it.

## Why

The owner asked for the most complex query I could think of. Writing one is
easy; writing one that stays true is not, which is where the second half comes
from.

The sidebar list is the first thing most readers click and it lives in
JavaScript, where nothing checks it against the grammar. Rename a column,
tighten the parser, change a fixture, and an example ships broken — greeting a
reader with a refusal in the one place the page is trying to look confident.
Nine examples had that exposure before this change; ten do not.

The kitchen sink also has a teaching job. It is **two statements rather than
one**, and its comments say why: `ORDER BY` and a second group key are not
available on the join path. The most complex thing the grammar can do is
bounded, and a reader is better served seeing the boundary than being shown a
query that hides it.

## Alternatives rejected

**One enormous statement.** Not possible, and the reason is the point above.
Forcing it would have meant either dropping `ORDER BY` (losing the ordered
groups, which is a real feature) or dropping the join (losing the hash join and
the side-split filters, which is a better one).

**Put the join second so it reads in order.** The results pane shows the last
statement that produced a grid, and the join here yields one row — `Manhattan`.
Ending on that is an anticlimax; ending on twenty ordered groups is not. The
Log tab shows both regardless.

**Assert the kitchen sink in a Rust test with the SQL copied into it.** A
second copy of a string is a second thing to update, and the copy that rots is
always the one nobody runs. The test reads the shipped file instead, with a
small unescaper for JavaScript string literals — ugly, and honest about which
artifact it is checking.

**Test only the kitchen sink.** It is the newest and most likely to break, and
it is also the one I would notice. The other nine are the ones that rot
quietly, which is why the test runs all of them.

**A `HAVING` clause.** The kernel has `Grouping::having`, so this is the one
construct the sink is missing. The parser does not expose it; adding it to
write a better example would have been the tail wagging the dog, and it is
recorded below as a gap rather than smuggled in.

## Evidence

The sink, run against the real data:

```
Table Scan on trips  (rows=2327 cost=13.50 decodes=[1,2,5,6,7,8,9,10])
["170","1","850","850","85","5.8","40","24065.77","2.738"]
["163","1","836","836","84","7.2","40","23173.05","2.676"]
```

Twenty groups, ordered by count descending, from 2,327 rows that survived six
conditions. The compiled spec carries six filters, two group keys and seven
aggregates, which is what `the_kitchen_sink_uses_everything_it_claims_to`
asserts — including that `like` and `matches` are both among the operators, so
the comment's claim about patterns is checked rather than believed.

Two Rust tests and one browser assertion. Mutations, each failing a *named*
test:

| mutation | check that failed |
| --- | --- |
| break one example's SQL (`author_id` → `nosuch`) | `every_example_the_page_offers_runs` |
| drop two conditions from the sink | `the_kitchen_sink_uses_everything_it_claims_to` |
| drop the `ORDER BY` from the sink | `the_kitchen_sink_uses_everything_it_claims_to` |

The browser check clicks the example the way a reader does, because the sink is
the only example containing a semicolon and the only one that is two
statements — the statement splitter is exercised by the click and by nothing
else on the page.

## What this does not do

**No `HAVING`.** The kernel supports it and the parser does not, so the sink
cannot filter groups after aggregating — the one construct missing from
"everything the grammar has".

> **Closed on 2026-09-15** by `a-having-and-the-type-underneath-it`. The sink
> now carries a `HAVING` over two of its aggregates, and there is a separate
> example for the clause on its own.

**No `OR`, no sub-queries, no window functions, no date arithmetic.** The first
is a deliberate refusal recorded elsewhere; the rest do not exist. A reader who
reads "kitchen sink" as "everything SQL has" will be disappointed, and the
comment in the example says *the grammar*, not SQL.

**The extractor is a regex and a hand-rolled unescaper.** It reads double-quoted
literals and `+` concatenation, which is what the file uses today. Rewrite the
examples list as a template literal or a `.json` file and the test stops finding
them — it asserts it found at least eight, so that fails loudly rather than
passing with zero, but it is still a coupling to a file format.

**It does not check the examples are *good*** — only that they parse, run, and
return something. An example that runs and demonstrates nothing would pass.
