# Relationships: declared on the type, loaded in one read

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-orm/src/relation.rs` (new), `crates/slate-orm/src/lib.rs`, `crates/slate-orm/tests/relations.rs` (new)
- **Kind:** feature

## What changed

A `Related<Other>` trait declares one direction of a relationship as two
ordinals — the local column and the column on the other table that matches it —
and `load_related` / `load_one_related` fetch every parent's related rows in
**one** read, returning a vector the same length as the parents. `related_filter`
returns the `Expr::In` they run, for a caller who wants to narrow something else
by a relationship rather than fetch it.

## Why

The crate is called `slate-orm` and had no relationships. Foreign keys existed
as constraints since task #61, but nothing navigated them: a caller wanting an
author's books wrote the join themselves, naming the tables and the ordinals.
That is a query builder. The thing an ORM is *for* is that you do not.

**The design decision, taken before any code: explicit and batched, with no lazy
accessor.** The feature an ORM is judged on is `author.books()`; the feature an
ORM is blamed for is that `author.books()` ran a query, once per author, inside
a loop nobody noticed writing. Those are the same feature. A lazy accessor
cannot tell whether it is being called once or ten thousand times, so it issues
one query either way and the cost lands somewhere the code does not mention. An
explicit `load_related(&txn, &ctx, &authors)` costs one read for the whole set
and says so at the line that pays for it.

The cost of that choice is real: the caller has to ask. The benefit is that
there is no way to write the N+1 by accident, which is the failure this whole
module exists to prevent.

## Alternatives rejected

**A lazy accessor**, as above. It is what people expect, and it is the single
most common performance complaint about every ORM that has one. Rejected
outright; if it is ever added it should be on top of this, not instead of it,
and it should be loud.

**Lowering to a join.** The kernel has good ones and a join answers this in one
read too. It is the wrong shape for *this* question: a join returns the parent's
columns once per child, so an author with forty books arrives forty times and
the caller reassembles them anyway, after paying to decode the author forty
times. `IN` over the child table reads each child once and each parent zero
times, because the parents are already in hand. It also lands on machinery that
already exists — task #32 taught the planner to turn `Expr::In` into point gets
and index ranges, so a relationship on an indexed column is a range read with no
work here at all.

**One bidirectional declaration** rather than an impl per direction.
`Author: Related<Book>` and `Book: Related<Author>` repeat two ordinals in
swapped roles, which looks like duplication. Rejected because a single
declaration would have to be read differently depending on which way it was
being used, and "which of these two columns is the local one" is exactly the
kind of thing that is right in the test and wrong at the call site.

**Returning only the parents that have children.** Shorter result, and every
entry non-empty. Rejected because it silently misaligns every index after the
first gap: the caller's `authors[i]` and `books[i]` stop being the same author.
The result is the same length as the input, and a parent with nothing gets an
empty vector.

**Keeping the filter construction private**, inside `load_related` where it
started. Rejected on the evidence: the deduplication it does is invisible from
outside — it changes neither the rows returned nor the number of reads, only the
size of the request — and a mutation deleting it survived the whole suite. The
fix is either an assertion on the request or no claim about deduplication at
all, and the request is only assertable if something returns it. It pays for
itself twice, because narrowing a second query by a relationship is a thing
callers want and had no way to express.

**Requiring `C: Clone` so shared children can be handed out cheaply.** Three
books by one author means one author row wanted by three books. Rejected
because `Record` does not imply `Clone` and adding the bound would narrow what
can be related for a copy that `from_row` already does — and re-decoding keeps
the shared case from being a special path.

## Evidence

Seven tests in `crates/slate-orm/tests/relations.rs`, five of them over an
intentionally
uneven fixture: one author with three books, one with one, one with none. A
balanced fixture is passed by an implementation that returns the same list for
every parent.

The one that matters is `loading_relations_costs_one_read_however_many_parents`,
because the claim is about *how many reads happen* and no assertion about the
returned rows can see it — a per-parent loop returns exactly the same books. So
the store is wrapped in a `KvStore` that counts scans, and the count is
measured:

```
batched = 1 scan
looped  = 3 scans   (3 authors)
```

Both halves are in the test: it loads the relation and counts, then runs the
per-parent loop it exists to replace and counts that, and requires the second to
be larger. If the counter ever stops measuring, the second assertion fails
rather than the first silently passing.

The bound on the batched side is `<= 1` rather than `== 1`, deliberately: a
future planner that chooses a point get over a scan opens no cursor, and a test
about costing *less* should not fail when the cost goes down.

The other four over the fixture cover parent order with a gap in it, the
belongs-to shape (three books sharing one author, and all three getting it — a
loader that moved rows out of the group instead of copying would give the first
book its author and the rest `None`), the empty-parent case that issues no read
at all, and a parent whose children were deleted. Two more assert the filter
itself: that it names a shared key once, and that with no parents it is an `IN`
over nothing.

`cargo clippy -p slate-orm --all-targets` is clean; the crate's suites are green.

### The mutation pass

Seven mutations, **two survivors**, both of which were real missing tests and
both of which are now caught by a named one:

| mutation | before | after |
| --- | --- | --- |
| delete the dedup before the `IN` | survived | `the_filter_carries_one_value_per_distinct_key_not_one_per_parent` |
| delete the empty-parents early return | survived | `loading_no_parents_reads_nothing_and_returns_nothing` |
| group children by the local ordinal, not the foreign one | caught (3 tests) | |
| put the `IN` on the local column | caught (3 tests) | |
| skip parents with no children instead of pushing an empty vector | caught (2 tests) | |
| `load_one_related` always `None` | caught | |
| `value_at` reads column 0 rather than the ordinal | caught (3 tests) | |

The second survivor is the more embarrassing one: the test was *named*
`loading_no_parents_reads_nothing_and_returns_nothing` and asserted only the
second half. Removing the early return leaves the result correct — an `IN` over
no values matches nothing — and pays for a read to discover it, which is exactly
what the name promises does not happen. It now reads the scan counter.

The first was predicted in an earlier draft of this entry as a gap that would be
left open. It is not left open: `related_filter` exists so the request can be
asserted, which is the "write the test rather than hide the survivor" rule
applied to a case where writing it meant changing the shape of the code.

Each mutation was re-applied after the new tests to confirm the failure is real
and lands on the named test, not on some unrelated assertion.

## What this does not do

**There is no `#[record(has_many(...))]` attribute yet.** The relationships in
the tests are hand-written impls. The trait is the contract and the attribute is
sugar over it, so the contract got the tests first; the derive is the next
commit and changes nothing about what is tested here.

**Nothing reaches the SDKs.** This is the Rust ORM layer. Python, Go and
TypeScript have no relationship surface, and giving them one means putting the
declaration somewhere they can see it — which is a schema question, not a client
one.

**No cascade, no constraint enforcement, no `load` inside a join.** A
relationship here is a way to fetch; it does not delete children, does not check
that the foreign key exists, and does not participate in query planning.
Constraints are task #61's foreign keys, which are separate and stay separate.
