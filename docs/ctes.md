# A CTE is three different questions, and only one of them is a gap

`orm-comparison.md` listed CTEs as Missing and said so about all of them:

> no plan node — **and the same tension as set operations above, unargued.**
> One statement compiles to one `QuerySpec` naming one table and one plan; a
> CTE is a second named query in the same statement. Whether that makes this
> Refused (like `UNION`) or Missing is a question nobody has answered in
> writing, and the row has been asserting the second for as long as it stood.

This note answers it. The answer is not one word, because `WITH` covers three
constructs with three different reasons, and lumping them cost the row its
meaning:

| shape | answer | why |
|---|---|---|
| `WITH RECURSIVE` | **Refused** | a fixpoint loop; one statement is one plan and nothing iterates |
| a CTE referenced more than once | **Refused** | materialising a named intermediate and reading it twice is a second plan |
| a non-recursive CTE referenced once | **Missing — and it is the same work as views** | it inlines into the outer spec, and the inlining is what `orm-comparison.md`'s views row says must be structural |

## What it does today, measured

Run against the workbench's parser rather than assumed:

```
WITH recent AS (SELECT id FROM books WHERE year > 2000) SELECT id FROM recent
  -> expected SELECT, INSERT, UPDATE or DELETE, found `WITH`
```

That is exactly the failure the `UNION` refusal was written against — its own
comment says a bare "unexpected `UNION`" *"reads as a parser that has not heard
of it, when the real answer is that there is nowhere for it to go"*. `WITH` had
the bare version. It has a named one now, and this note is what the message
points at.

For contrast, the thing that does work:

```
SELECT id FROM books WHERE id IN (SELECT id FROM books WHERE year > 2000)
  -> accepted, planned as an ordinary IN
```

An uncorrelated subquery runs once and its single column becomes the `values`
of an `Expr::In`. That is why a CTE *looks* close to buildable: half of what a
CTE does is already done, in a different position.

## 1. `WITH RECURSIVE` is Refused, and it is the clearest of the three

A recursive CTE is a fixpoint: evaluate the anchor, evaluate the recursive term
against what you have, repeat until nothing new appears. That is a loop whose
trip count depends on the data.

A `QuerySpec` holds one access path, one filter set, one projection. There is
no iteration anywhere in it, and there is nowhere to put one — the same
sentence the set-operator refusal uses, and here it is stronger, because
`UNION` at least has the "two statements separated by `;`" workaround and a
fixpoint has none. Nothing a caller can write with the pieces that exist
computes a transitive closure.

**It is also the one shape with an unbounded cost**, which matters more here
than the expressiveness. Every other refusal in this front end is about a
missing operator; this one is additionally about a query whose work a caller
chooses and the daemon's per-request ceilings — `max_groups`, `max_sort_rows` —
do not describe. A recursive CTE would need its own bound before it needed a
plan node.

## 2. A CTE referenced twice is Refused, for the `UNION` reason exactly

```sql
WITH recent AS (SELECT id FROM books WHERE year > 2000)
SELECT * FROM recent JOIN recent AS other ON …
```

The *point* of writing that rather than repeating the subquery is that `recent`
is computed once. Delivering the shape without that property would be a
performance trap dressed as a feature: the query reads as "compute this once"
and would silently compute it twice.

Computing it once means materialising a named intermediate result and planning
two reads against it — a second plan in the same statement, which is precisely
what "a statement compiles to one query spec" rules out. Refused, and the
message should say *referenced more than once* rather than "CTEs", because the
single-reference case is a different answer.

## 3. A single-reference CTE is Missing, and it is the views row

```sql
WITH recent AS (SELECT id, title FROM books WHERE year > 2000)
SELECT title FROM recent WHERE id > 5
```

is

```sql
SELECT title FROM books WHERE year > 2000 AND id > 5
```

The transformation is: substitute the CTE's body for the reference, merge the
filters, and resolve the projection through it. Nothing new reaches the kernel
— it is one `QuerySpec` over `books`, which is what the outer statement would
have compiled to anyway.

**This is the finding worth having.** `orm-comparison.md` lists CTEs and views
as separate rows, and they are the same mechanism:

> The safe shape is that a view expands to its underlying spec *before*
> planning, so the base table's id is what reaches the policy, and that has to
> be structural rather than a convention somebody remembers.

A single-reference CTE expands to its underlying spec before planning. That is
the same sentence. A view is that expansion with a name that persists and a
catalog entry; a CTE is that expansion with a name that lives for one
statement. Build the expansion and both rows move.

**The security hazard is the same one, and a CTE would meet it sooner.** Grants
and policies key on `TableId`. A CTE has no catalog entry, so
`Catalog::table_by_name("recent")` finds nothing — which is *why* the parser
fails where it does. The tempting fix is to register the CTE under a synthetic
`TableId` so the lookup succeeds, and that is the exact hazard the views row
names: every grant and policy check would then key on the synthetic id, and a
caller would read the base table's rows with the base table's policy never
consulted. Expansion before planning is not an optimisation here. It is the
only shape that is safe.

## What this note does not do

**It does not design the expansion.** Where it happens, what it does about
name collisions with real tables, whether an expanded spec keeps the CTE's name
anywhere for `EXPLAIN` to show, and what happens when the outer query's
`ORDER BY` names a column the CTE projected away — none of that is decided
here. That belongs with the views design, which is the point.

**It does not decide what a CTE over a join does.** The inlining above is
single-table on both sides. A CTE whose body is a join, referenced once from a
query that also joins, is a chain — and whether that composes or has its own
refusal is unexamined.

**It does not measure anything.** The `IN (SELECT …)` contrast above was run;
the inlining equivalence in §3 was reasoned from the grammar and not executed,
because there is nothing to execute it with.

**It does not count how much of the row this closes.** Of the three shapes,
two are now Refused with reasons and one is Missing with its work named. The
row can stop claiming all three are Missing; it cannot yet claim any of them is
built.
