# Keyset pagination: pages that cost the same and do not shift

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-kernel/src/plan.rs`, `crates/slate-kernel/src/query.rs`, `crates/slate-kernel/src/read.rs`, `crates/slate-kernel/src/error.rs`, `crates/slate-kernel/tests/pagination.rs` (new), `crates/slate-orm/src/ext.rs`, `crates/slate-orm/src/lib.rs`, `crates/slate-orm/tests/paging.rs` (new)
- **Kind:** feature

## What changed

`Query::after(key)` resumes a read from the primary key the last page ended on.
It is applied by `Plan::resume_after`, which narrows the scan range rather than
counting rows. At the ORM layer `Records::page_records` returns a `Page<R>`
carrying the rows and the cursor for the next page, so a caller never names a
key column.

## Why

`OFFSET` was the only pagination there was, and the executor is honest about
what it costs:

```rust
// An offset still has to find the rows it discards; there is no
// cheaper way to know which ones they are.
```

That comment is true when a count is all you have. Given the **key** the last
page ended on there is a cheaper way, because the range itself can start after
it. Measured, over a 500-row table in pages of five:

```
page 99 of 100:  offset read 495 key-value pairs
                 cursor read   5
```

**But the cost is the smaller half.** `OFFSET` counts rows, so it is only
correct while nothing changes. There is a test that deletes one row between two
pages and shows the offset reader silently never seeing row 4 — page one
returned 1–3, the delete shifts everything up, and page two returns 5–7. No
error, no warning. The cursor reader gets 4–6, because a key does not move when
its neighbours change. That test asserts the wrong answer as well as the right
one, because showing only the right one would not establish that there had been
a problem.

## Alternatives rejected

**A tenth argument to `plan_hinted`.** The obvious place to put a cursor is next
to the limit and the offset, in planning. Rejected: it is not an input to
*choosing* an access path, it is a narrowing of the one chosen, and
`plan_hinted` already carries nine arguments and an `#[allow]` for it. As a
method on `Plan` it is also independently testable and says what it is.

**Letting the cost model choose the access path for a paged query.** Rejected,
and this is the decision the rest depends on. An index might be the cheaper plan
for some page, and an index yields rows in the index's order — where a primary
key says nothing about where the page ended. A query that paged correctly on a
small table would start returning wrong pages once it grew, which is the class
of bug the migration work in this branch was about. A cursor therefore pins the
access path to the table's own key range, exactly as `AccessHint::TableScan`
already did for callers who asked. Which plan runs follows from the request.

**Filtering a sorted query by the cursor anyway.** A query sorted into an order
the key does not give has its rows re-ordered *after* they are read, so "after
this key" names nothing in the output. Applying it to the input would quietly
drop rows the sort would have placed on this page. Refused, with a message
naming both ways out (order by the primary key, or page by offset).

**Silently dropping the cursor on a grouped read.** `narrowed` already drops
`limit` and `offset` for grouped reads, with a documented reason, so dropping
`after` beside them was the consistent thing to do and would have been the worst
outcome in the change: the caller asks to resume from a row, gets an aggregate
over the whole table, and nothing says so — a *plausible* wrong number. Refused
instead, at both grouped entry points.

**An opaque cursor token** encoding the plan as well as the key, so a cursor
could survive an index scan. It is the general form and it is what a
production-grade "seek" API eventually needs. Rejected for now because it means
validating on the way back in that the plan has not changed since, and inventing
a wire format for something with one caller. What ships instead is the case that
is exactly right, with a refusal on the rest — recorded below as a limit rather
than described as a design.

**Defaulting `page_records` to some page size** when `limit` is unset. Rejected:
a silent default is a number the caller did not choose deciding how much of
their table they read.

**Reading one row past the page** so `Page::next` is `None` on the last full
page. Rejected: it pays an extra read on *every* page to save one empty request
at the end of a sequence most callers never finish. The behaviour is documented
on `Page` and there is a test asserting it, so it is a decision rather than a
rough edge.

## Evidence

Fourteen tests in `crates/slate-kernel/tests/pagination.rs` and four in
`crates/slate-orm/tests/paging.rs`. Five of the fourteen exist only because a
mutation survived without them; see the table below.

The cost claim is measured, not asserted, through a store that counts the
key-value pairs read — *pairs*, not scans, because both styles open exactly one
cursor and counting scans would report them identical. The number printed above
comes from that test. Its bound is deliberately loose (`cursor * 10 < offset`)
and it has a floor (`cursor >= 5`): the claim is "the cursor does not pay for the
rows it skips", not a ratio, and pinning a ratio would pin the prefetch depth
and the row encoding, neither of which the test is about.

The correctness claim is shown both ways round, as described above.

The other cases: an exclusive boundary (an inclusive one repeats a row per page,
which is the bug people write reaching for this by hand), descending order (the
cursor becomes the *end* bound, and a test with one direction would never find
that), composition with a filter, a cursor past the end and a cursor on a
deleted key, and the three refusals.

The ORM tests cover the part that only exists at that layer: the caller never
names a key column. One uses a **composite, tenant-scoped** key, because that is
where a hand-built cursor goes wrong — it pages through one tenant's rows using
another tenant's boundary — and asserts the returned cursor is
`[tenant, id]`, in key order, with the caller having written neither.

### A mistake worth recording

The first run of the ORM tests failed, showing rows repeated at every page
boundary — `[1,2,3,3,4,5,5,6,7,7]` — while the kernel tests covering the same
property passed. Twenty minutes went into the difference between the two tables
before the actual cause: **a mutation-testing script was running in the
background against the same working tree**, and had the `Bound::Excluded` →
`Bound::Included` mutation applied while those tests ran. The test was correct,
the code was correct, and the source on disk was neither.

Recorded because the failure mode is general and this repository is explicitly a
place where several agents work at once: a background job that edits source is
not a background job. It was restarted in the foreground, and the mutation table
below is from a run where nothing else touched the files.

It then stalled twice more with no output, which looked like a build hanging on
a lock and was not. Two separate mistakes: the runs piped through `tail`, which
holds everything until the stream ends, so there was nothing to see; and the
thing actually hanging was the *test*, not the build — with an inclusive cursor,
`a_cursor_walks_the_table_a_page_at_a_time` repeats its last page for ever.
Reading `/proc/<pid>/task/*/children` and finding the test binary rather than a
compiler is what settled it. The loop now has an iteration bound, with the
reason in a comment: a hanging test reads as broken infrastructure rather than a
broken assertion, and the repository already makes this argument about the
`slate-serverd` harness's timeouts.

### The mutation pass

Nine mutations, **four survivors**, every one a real missing test:

| mutation | result |
| --- | --- |
| the cursor bound becomes inclusive | caught (6 tests) |
| descending uses the ascending bound | caught |
| the cursor width is not checked | caught |
| a sorted plan is not refused | caught |
| a grouped read's cursor is not refused | caught |
| a single-row read ignores the cursor | **survived** → `a_cursor_applies_to_a_single_row_read_too` |
| the cursor stops pinning the access path | **survived** → `a_cursor_pins_the_access_path_to_the_primary_key` |
| an index plan accepts a cursor silently | **survived** → `an_explicit_index_hint_with_a_cursor_is_refused_rather_than_overridden` |
| a ruled-out read keeps its estimates | **survived** → `a_read_the_cursor_rules_out_explains_as_reading_nothing` |

**The second survivor is the one worth reading about**, because the test it
broke was not missing — it was *wrong*. There was a test called
`a_cursor_beats_an_index_the_planner_would_otherwise_have_chosen`, and removing
the pinning did not fail it. The planner had never been choosing the index for
that query, so the test asserted rows that were correct for a reason unrelated
to its name. Rewriting it to assert the *plan* rather than the rows made it fail
honestly, and then took two more attempts to build a fixture where the index
genuinely wins: a selective filter was not enough on its own — on this cost
model a scan of two hundred rows beats an index lookup plus two hundred row
reads — and it needed the projection narrowed so the index answers without
reading a row at all. That is a fact about the cost model nobody had written
down, and it is now in a comment where the next person will trip over it.

**The fourth survivor corrected a claim in the source.** The test written for it
first asserted that a cursor *past the last row* plans as reading nothing, and
failed. It does not: the bounds are still a perfectly good slice of the
keyspace, it simply has nothing in it, and no planner can know that without
reading. The only case where a cursor makes a plan provably read nothing is a
single-row read whose row is behind it. The test now asserts that, and says why
the obvious version of it was wrong.

## What this does not do

**No cursor over an index.** A query whose rows come back in an index's order
cannot be resumed from a primary key, and is refused rather than approximated.
That is the main limitation and the main thing someone will want next; the shape
it needs is the opaque token rejected above.

**No cursor on the wire.** Python, Go and TypeScript cannot send one, so this is
reachable from the Rust ORM only. The field is a repeated value on a query
message and the clients each need a `Page` of their own, which is a protocol
change and belongs with the rest of them.

**`after` and `offset` compose, and nobody should.** Setting both skips `offset`
rows *after* the cursor. It is allowed because refusing it would be arbitrary —
they are independent — but there is no use for it and no test beyond the fact
that neither disables the other.

**The prefetch window is not tuned for a cursor.** `planning_limit()` is passed
unchanged, which caps the prefetch at `limit + offset`: right for an offset,
more generous than necessary for a cursor. Left alone rather than changed on a
guess, and named here so it is not mistaken for having been considered and
chosen.
