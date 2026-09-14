# A timing test's comment said its middle query used an index; it plans as a table scan

- **Date:** 2026-09-14
- **Author:** Claude Opus 5
- **Touches:** `crates/slate-wasm/tests/timing.rs` (comments only)
- **Kind:** docs

## What changed

Two comments in the test file added an hour earlier. `the_reported_time_grows_with_the_work`
described its middle query, `SELECT * FROM trips WHERE pickup_zone = 132`, as
reading "4,837 rows through an index". It does not: the planner picks a **table
scan** for it, which was measured in the same session that produced the file.
The module header also said 4,824 rows where every other line in the file says
4,837.

## Why

The wrong one is not a typo. It states the opposite of the single most
counter-intuitive thing this project has to say — that on object storage an
index which still has to fetch rows *loses* to a full scan, because a point read
costs about as much as scanning 24,000 rows. A reader who takes the comment at
face value learns the opposite of the lesson, from a file whose whole subject is
not reporting numbers you did not observe.

It also would have misled the next person to touch the test. The query is there
because it reads 4,837 rows, not because of how it reads them; someone
"improving" it to actually use an index would change what the test measures
while making the comment true.

## Alternatives rejected

**Amend the previous commit, which already carries a full entry.** Cheaper, and
avoids a second ledger file for two comments. Rejected because that commit is
pushed and CI is running against it; a force-push would discard a run in flight
and rewrite history other checkouts may already have.

**Change the query to one that really is an index scan, so the comment becomes
true.** Rejected: the test needs a middle term between one row and 100,000, and
what makes it a valid middle term is the row count. Choosing the query to suit a
comment is backwards, and it would quietly weaken the ordering assertion by
making the three cases differ in access path as well as size.

## Evidence

Measured in headless Chromium against the built bytes, reading the plan the page
itself displays: `SELECT * FROM trips WHERE pickup_zone = 132` reports access
`Table Scan`, while `SELECT pickup_zone FROM trips WHERE pickup_zone = 132`
reports `Index Only Scan using by_pickup_zone`. Narrowing the projection is what
flips it, which is the point the comment now makes.

`cargo test -p slate-wasm --test timing` — 6 passed, unchanged, as it must be
for a comment-only diff.

## What this does not do

Comments only; no assertion, threshold or query changed. It does not audit the
rest of the file's prose against the plans actually chosen — the two fixed here
were found while re-reading the diff, not by a sweep. The one structural guard
against this class is in `the_clock_stops_before_the_rows_are_rendered`, which
asserts its two queries share an access path rather than trusting a comment;
`the_reported_time_grows_with_the_work` deliberately does not, because its three
queries are *expected* to differ.
