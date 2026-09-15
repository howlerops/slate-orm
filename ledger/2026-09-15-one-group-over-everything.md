# `SELECT count(*) FROM books`, which only the front end was refusing

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-wasm/src/sql.rs`, `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/playground.rs`, `crates/slate-wasm/tests/sql.rs`
- **Kind:** fix

## What changed

An aggregate no longer needs a `GROUP BY`. `SELECT count(*) FROM books` returns
one row over the whole table, as does any list of aggregates. A bare column
beside an aggregate is still refused.

## Why

The refusal was in the parser and nowhere else. Probed before changing
anything:

```
Grouping::by([], &[Aggregate::Count])  ->  Ok([Group { key: [], values: [U64(5)] }])
```

The kernel has always answered a grouping with no keys — one group, empty key,
the aggregate values — so the front end was refusing a query every layer
beneath it could serve. The guard mistook the usual shape for the only shape.

## Alternatives rejected

**Lowering it to the ungrouped aggregate path** (`RecordTransaction::aggregate`,
which returns a bare `Vec<Value>`). It exists and would work. Rejected because
it is a *second* way to compute the same answer, reached by a different
condition, and the repository's own rule for the SQL front end is that it is a
way of writing a spec rather than a second way to reach the kernel. One grouping
with no keys goes down the path that is already tested.

**Allowing a bare column beside the aggregate**, filling it from some row. It is
what MySQL does. Rejected: the query returns a single row over the whole table
and there is no one value for that column to take, so any answer is a value
picked at random and presented as a fact.

## Evidence

Three tests in `crates/slate-wasm/tests/playground.rs`: the count, several
aggregates at once, and the refusal that stays.

The count is asserted against **4,824** — the number
`the_fixture_is_seeded_and_readable` reaches by listing every row — so it is a
count rather than a shape check.

### A second bug, found by the test rather than reasoned about

The first run came back with the right values and the wrong headers:

```
"columns": ["id","author_id","title","year"]
"rows":    [["4824","1944","2019"]]
```

Four headers over a three-value row. `statement` decides the headers with
`let grouped = !spec.group_by.is_empty();` — the same mistake as the dispatch,
in a different function. Two places had to agree about what "grouped" means and
only one of them had been changed. They now share the test and a comment saying
they have to. Both mutations — reverting either one — are caught.

### The mutation pass

| mutation | result |
| --- | --- |
| the dispatch tests only the keys | caught (2 tests) |
| the headers test only the keys | caught (2 tests) |
| a bare column beside an aggregate is allowed | caught (3 tests) |

No survivors.

### Two stale tests

`refusals_name_what_was_wrong` and `grouped_refusals_name_what_was_wrong` both
asserted that `SELECT count(*) FROM books` is refused. That is the behaviour this
commit deliberately changed, so the cases were replaced rather than deleted:
they now assert the refusal that remains — a column beside the aggregate — with
a comment recording what used to be there and why it went.

## What this does not do

**`SELECT DISTINCT` is still not a keyword**, though the machinery is the same:
`SELECT zone FROM trips GROUP BY zone` already returns the distinct keys, so
`DISTINCT` is a spelling away. It is not in this commit because the grouped path
appends a `count(*)` when no aggregate is named, and a `DISTINCT` that returned
a column the caller did not ask for would be a different kind of wrong. That is
a small change and a separate one.

**Nothing about whole-table aggregates reached the clients**, which never had
this restriction: the wire protocol takes aggregates and a group list, and an
empty group list already worked. Only the SQL front end was refusing, and only
the SQL front end is fixed.
