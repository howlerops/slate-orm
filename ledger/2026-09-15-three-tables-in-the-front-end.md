# `FROM a JOIN b JOIN c` in the SQL front end, and two headers that named the wrong column

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/rust-orm-record-layer-gswxlu`
- **Touches:** `crates/slate-wasm/src/sql.rs`, `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/sql.rs`
- **Kind:** feature

## What changed

The workbench's SQL parser reads any number of `JOIN <table> ON <col> = <col>`
clauses. Two tables lower onto `JoinSpec` as before; three or more lower onto a
new `Statement::Chain(ChainSpec)`, which reaches the kernel's `Chain` through
`Playground::chained_spec`. A `chain()` entry point is exposed beside `join()`.

The parser's joined-space helpers — `resolve_side`, the group-key and
ORDER-BY-slot resolution, the aggregate resolution — now take `&[TableDef]`
instead of a `left` and a `right`, so there is one implementation of each rather
than a two-table one and an n-table one to drift apart. `join_tail` is written
over that slice from end to end and only its last dozen lines care how many
tables there are.

Three defects surfaced on the way and are fixed here, all in the *two-table*
path that has been shipping for weeks.

## Why

The kernel has done chains since task #30, the wire since #69, and all three
SDKs since #86. This front end could not say one: `JoinSpec` was
`left`/`right`/`left_key`/`right_key` by construction and the parser was written
against that shape in five places. So the workbench — the thing a visitor
actually types into — was the only component of the system that could not
express a query the rest of it plans, executes, costs and explains. That was
recorded as a gap in the #141 entry and this closes it.

The fork between `Join` and `Chain` is deliberate and is not a length. `Join`
narrows each side's projection for a grouped read; `Chain` cannot. They are
different plans, `slate-server` splits them the same way, and a two-input
`ChainSpec` would answer a query the join path already answers through a second
kernel entry point. The parser therefore never emits one, and `chained_spec`
refuses one if anything else tries.

### The three defects

**1. A projection on an ungrouped join was silently ignored.** `SELECT title
FROM authors JOIN books ON authors.id = books.author_id` returned all eight
columns of both tables, with a header saying so, and no error. The guard meant
to catch it read `!star && list.is_empty()`, which `select` cannot produce: it
sets `star` from a leading `*` and otherwise parses at least one item, so the
two conditions are never both true and the check had never fired once. It is now
`group_by.is_none() && !star`, which is the condition it was always meant to be.

This is the worst shape of wrong answer available: the header is right about the
rows, the rows are right about the database, and only the query has been
ignored. Nothing was going to notice, because every test of an ungrouped join
wrote `SELECT *`.

**2. A computed value's header was named against the left table.** The join arm
did `left_table.column(c.column)` whatever `c.input` said, so
`year(books.author_id)` — ordinal 1 of `books` — came back labelled
`year(name)`, which is ordinal 1 of `authors`.

**3. An aggregate's header was named against the right table.** `labels(&spec
.aggregates, &right_table)`, from when an aggregate could only read the right
table. Since task #133 it can read either, so `max(born)` — ordinal 3 of
`authors` — came back labelled `max(year)`, ordinal 3 of `books`.

Both header defects produce correct values under a wrong name, which is why no
test asserting on cells ever caught them: it is the heading that lies. Both are
fixed by one function, `joined_names`, which the join and chain arms share and
which resolves each name against the table the value's own `input` names.

## Alternatives rejected

**Widen `JoinSpec` to a list of inputs and drop `ChainSpec`.** One spec, one
arm, one code path, and it is the shape the task was written as. Rejected
because it would make the *binding* choose between `Join` and `Chain` from a
length, one level further from the reason than the parser is — and because a
two-table `JoinSpec` is what the panel's join form sends and what three tests
and the site's examples are written against. Widening it means either a
migration of all of that, or a spec whose first two inputs are special, which is
the same fork with the reason removed.

**Keep two implementations of each helper, one for two tables and one for n.**
Cheaper to write and nothing existing changes. Rejected outright: the reason the
two-table versions had the joined-space bug fixed in #133 — computed columns
against the left, aggregates against the right — is that the ordinal arithmetic
was written out inline, three times, slightly differently. A second full set of
it is a guarantee of a fourth variant.

**Refuse a step that joins to anything but the previous table.** Simpler to
parse and to explain, and it covers the straight-line chain. Rejected because
`JoinKey` is in the joined space and always has been: a star schema, every
dimension keyed off one fact table, is the commonest chain there is, and
refusing it would be the front end inventing a restriction the kernel does not
have. `a JOIN b JOIN c ON a.x = c.y` parses, and there is a test for it.

**Aliases, so a table can appear twice.** This is what a *useful* three-table
chain over the taxi schema needs — `trips` reaches `zones` through both
`pickup_zone` and `dropoff_zone` — and not doing it is why there is no workbench
example in this entry (see below). Rejected for *this* change because it is its
own cascade: a name that is not a table name means `resolve_side`, `resolve`,
every error message and both specs need an alias alongside the `TableDef`, and
the headers and the spec pane need to show it. Bundling it would make one commit
that does two things, and the second one is the one with a UI in it.

**Qualify a grouped result's key header** (`authors.country` rather than
`country`), for consistency with the ungrouped header. Rejected: an ungrouped
header is a grid of every table's columns side by side, where three unqualified
`id`s name nothing; a group key is one column the reader named themselves, and
echoing their own spelling is what the single-table grouped path does. The two
callers want different answers and `joined_names` takes a flag saying which.

## Evidence

Ten new tests and two new refusal cases in `crates/slate-wasm/tests/sql.rs`
(27 → 37 in that file; every suite in the crate is green with `--no-fail-fast`,
and `cargo clippy --workspace --all-targets` is clean).

The two that are worth more than the rest are differentials against an
independent query rather than against a written-down answer:

- `a_grouped_chain_agrees_with_the_join_the_chain_narrows`. The zone ids are
  `1..=n` with no holes — which the test *checks* rather than assumes, by
  comparing the row count with the highest id — so `books.id = zones.id` keeps
  exactly the books with `id <= n`, and the chain's count per country must equal
  the two-table join's count per country with that as a `WHERE`. A fixture
  change moves both sides.
- `an_aggregate_is_labelled_with_the_column_it_reads` checks the label *and*
  compares the values per country against `SELECT max(born) FROM authors GROUP
  BY country`. It therefore fails either way round: a right label over the wrong
  column, or a wrong label over the right one.

`a_chain_through_sql_matches_the_chain_spec` compares the plan text as well as
the rows, because rows alone would pass for a front end that dropped a step and
got lucky — and over three tables joined on an id, that luck is available.

### Mutation testing

Ten mutations, each applied alone, with `cargo test -p slate-wasm --test sql`
(the first seven also against `--test examples --test datetime`):

| mutation | verdict |
| --- | --- |
| `joined_at` sums no earlier widths (`.take(input)` → `.take(0)`) | caught, 4 tests |
| the chain's `ON` index is not shifted (`at.checked_sub(1)` → `Some(at)`) | caught, 7 tests |
| a chain's `ON` may name a later table (resolve the far side against every table, not the earlier ones) | **survived** — see below |
| `shape()` never says "chain" | caught |
| the ungrouped select list is ignored again (the old unreachable guard) | caught |
| a bare name takes the last matching table, not the first | caught, 2 tests |
| a computed value is named from the first table (defect 2, restored) | caught |
| an aggregate is labelled from the last table (defect 3, restored) | caught |
| a grouped key's header is qualified | caught, 4 tests |
| the join/chain fork is never taken (two tables also become a chain) | caught, 6 tests |

The three that mattered most were the two restored defects and the restored
unreachable guard: each is a bug this change fixes, put back, and each is caught
by a named test.

**One survived, and it was a missing test rather than a harmless mutation.**
`join_key` resolves the far side of an `ON` against the tables read *before*
this one; the mutation resolved it against all of them, including the table
being added. For the chains under test that changes nothing — `ON books.id =
zones.id` resolves `books` to the same input either way — so no test noticed. But
what it lets through is `ON b.x = b.y`, both sides on the table being joined,
and over **two** tables that is not a refusal but a wrong answer: `left_key`
becomes an ordinal of `books` used as an ordinal of `authors`, so `authors.name`
is joined to `books.id` and nothing errs anywhere. That is the same class as
defect 1 above, reached from a different direction.

Two refusal cases were added for it, one two-table and one on a step, and the
mutation is now caught by both `refusals_name_what_was_wrong` and
`a_chain_refuses_what_it_cannot_answer`. It is the one thing in this change that
the mutation pass found rather than confirmed, which is the argument for running
one at all.

### What the plan looks like

The three-step chain plans and explains through the same path `slate-server`
uses, which was worth seeing rather than assuming:

```
Chain
  -> Table Scan on authors  (rows=406 cost=1.05 decodes=[0,2])
  -> Hash step 1: Table Scan on books  (rows=4824 cost=1.60 decodes=[0,1])
  -> Hash step 2: Table Scan on zones  (rows=265 cost=1.03 decodes=[0])
```

The `decodes` lists are the grouped read's narrowed projections — `authors`
decodes its key and its country, `zones` only the column the step joins on —
which is task #92's work showing up unasked in a front end that had never
reached it.

## What this does not do

**There is no alias, so there is no useful chain over the taxi schema and no
workbench example.** `trips` reaches `zones` twice, through `pickup_zone` and
`dropoff_zone`, and "the borough a trip started in and the borough it ended in"
is the three-table query this dataset is *for*. It needs `JOIN zones AS pickup
... JOIN zones AS dropoff`, and naming a table twice is refused here — correctly,
since without an alias every column reference to it would be ambiguous. The only
chain this schema can express is `authors JOIN books JOIN zones` on `books.id =
zones.id`, which is a well-formed chain and a meaningless question. The tests use
it and say so. Putting it on the site as an example would be documenting a query
nobody would write, so the site is unchanged and the example waits for aliases.

**`GROUP BY` on a join or chain still takes exactly one key.** The single-table
path takes a list; `JoinSpec::group_by` and `ChainSpec::group_by` are one
`Option<u32>` each. Widening them is not hard in the parser — the loop is already
there for the single-table case — but it changes both specs, the panel's form
and the spec pane, so it is left where it was rather than half-done. `GROUP BY a,
b` over a join is still a refusal.

**`ORDER BY` on an ungrouped chain is still a refusal**, for the same reason it
is on an ungrouped join: neither `Join` nor `Chain` has a sort, the kernel orders
groups, and there is nothing to lower an ordering of whole rows onto. The refusal
now says "chain" when the reader wrote three tables, which is all that changed.

**Nothing outside the wasm crate is touched.** The panel's join form is still
two dropdowns; the browser check (`site/check/workbench.py`) has no chain case,
because it would have to be the meaningless one. Both belong with the aliases.

**The chain path has no proptest round trip.** The single-table path has one —
generate a spec, render it as SQL, parse it back, require the same spec — and
that property is the most valuable test in the file. A chain renderer is a
strictly bigger independent implementation (n inputs, n-1 keys, each naming any
earlier input) and writing it is real work rather than a line. The chain tests
here are hand-written differentials, which test the cases somebody thought of;
this is the case where I know which kind I wrote.
