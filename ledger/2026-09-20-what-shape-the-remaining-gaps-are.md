# Reading the other six gap rows before anybody builds one

- **Date:** 2026-09-20
- **Author:** Claude, doing what the previous entry said was owed
- **Touches:** `docs/orm-comparison.md`
- **Kind:** docs

## What changed

Four more gap rows annotated with what kind of work they are. No code. Two are
**refused with reasoning that was written somewhere else**, one is **larger
than it reads by a structural assumption**, one is **half refused and half
open**.

## Why

The previous entry found that the first gap row anyone would pick up —
generated migrations — cannot be built as written, because the store keeps a
one-way fingerprint rather than a schema. It closed with:

> The same reading is owed to the other six rows before any of them is picked
> up: I verified they are absent, not that the work is the shape the row
> implies.

An hour earlier I had verified all nine rows were *accurate* — the features
really are absent. That is a different question from *what kind of item this
is*, and the taxonomy at the top of this very file draws the line: **Missing**
is work, **Refused** is a decision with the reasoning written down, and
**Forbidden** needs the architecture to change. A row in the wrong category
sends somebody to do work that was already decided against.

Four of the six are in the wrong place or understate the work.

**Set operations are Refused, and the reasoning was in an error message.** The
SQL front end refuses `UNION`, `INTERSECT` and `EXCEPT` by name, and says why:
a statement compiles to one `QuerySpec`, and for `UNION` specifically,
"combining the two answers is the easy half; it is the deduplication across
them that nothing here can do, because each statement is planned and executed
on its own." That is precisely this file's definition of Refused. It read as
Missing because the reasoning lived where only somebody who hit the error would
see it.

**CTEs have the same tension and nobody has argued it.** A CTE is a second
named query inside one statement, against a spec that names one table and one
plan. Whether that makes it Refused like `UNION` or genuinely Missing is a
question with no written answer, and the row has been asserting the second by
default.

**Full-text search is a new index *cardinality*, not a new index expression.**
Every index here writes one entry per row: `entry_for` returns a single
`IndexEntry` from `index.key_values(row)`, and the write path, the
unique-slot check and the scan all assume it. An inverted index is one entry
per term per row. The row says "none; `LIKE`/`ILIKE`/regex only", which is true
and reads like "add an index kind".

**Seed factories are half a decision already made.** `seed.rs` argues, at
length and convincingly, that seeding is a command-line act precisely because
it writes as `SecurityContext::superuser` — the rows must land before the
grants they will be read under exist — and that this is "the only reason the
word `superuser` appears in this crate… neither reachable from the wire." A
client-reachable seed is a superuser write path from the wire, which is the
thing that argument exists to design out. The row's complaint that "no client…
can seed at all" is answered. What it also says — nothing *generates* rows, and
the Rust library has no entry point — is untouched by that argument and
genuinely open.

## Alternatives rejected

**Move the rows into the Refused section.** Cleaner taxonomy, and it loses the
comparison: the point of a row is that seven other ORMs have the feature, and a
reader wants to see that next to why this one does not. Annotating in place
keeps the comparison and adds the category.

**Decide the CTE question here.** Tempting — the `UNION` reasoning looks like
it transfers. It does not transfer cleanly: `UNION`'s hard part is
deduplication *across* independently planned statements, and a non-recursive
CTE is closer to a subquery, which this codebase does support. Deciding it
properly means working out whether a CTE lowers to a chain or needs a spec
node, and that is a design conversation, not an annotation. The row now says
the question is open instead of implying the answer.

**Build the cheapest one instead of reading all six.** What I would have done
without the previous entry's finding. Two of the four I looked at would have
been work already decided against, and a third would have been started on the
belief that it was an index expression.

**Leave the seed row alone because half of it is real.** A row that is half
wrong sends a reader to argue with `seed.rs`, which has already made the case.
Splitting it says which half to pick up.

## Evidence

Read and quoted: the SQL front end's set-operator refusal
(`crates/slate-wasm/src/sql.rs:511–537`), `entry_for`
(`crates/slate-kernel/src/record.rs:2774`, one `IndexEntry` per row) and the
two maintenance loops that iterate `table.indexes()` one entry at a time,
`seed.rs`'s module docs (lines 1–20).

`python3 site/check/docs.py`: every relative link resolves.

No behaviour changed, so there is nothing to mutation-test.

## What this does not do

**Two rows were not read this way: window functions and views.** For window
functions I checked only that `Aggregate` is a closed enum of seven with no
frame or partition, which is the row's own evidence and says nothing about
whether the sort and grouping machinery would take one. For views I checked
nothing beyond their absence — the interesting question, whether a view's own
predicate composes with the caller's row policy, I have not looked at. Both
are recorded as unread rather than implied to be ordinary.

**The array-type row was left alone**, and it is the one I am most confident is
ordinary work — with one design decision the row does not mention: an
order-preserving codec has to decide how an array sorts, and `ValueType::ALL`'s
length is now part of its type, so adding a variant fails to compile in the
right place. I did not annotate it because "ordinary work plus a design
decision" is what a Missing row already means.

**No category was actually changed.** Every row is still under "Missing", with
prose saying it may not belong there. Moving them is a judgement about a
document's structure that a second reader should make, and three of the four
turn on questions — is a CTE a subquery or a statement? — that this entry
deliberately leaves open.

**This is the third pass over the same file today** and it found something each
time: stale claims, then a blocked row, now four miscategorised ones. That rate
is itself a finding about the file, and the honest reading is that a fourth
pass would probably find more.
