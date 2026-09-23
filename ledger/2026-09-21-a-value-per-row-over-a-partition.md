# Window functions, as a new operator — because an aggregate is the opposite cardinality

- **Date:** 2026-09-21
- **Author:** Claude Code, on `claude/rust-orm-record-layer-gswxlu`
- **Kind:** feature
- **Touches:** `slate-kernel` (new `window.rs`, `query.rs`, `exec.rs`, `read.rs`,
  `explain.rs`, `limits.rs`, `error.rs`), `slate-server/src/status.rs`,
  `slate-serverd` (`config.rs`, `main.rs`), `slate-wasm/src/sql.rs`,
  `docs/orm-comparison.md`, `site/docs/roadmap.html`

## What changed

`slate_kernel::window` computes a value per input row over a partition:
`ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, and any existing
[`Aggregate`] `OVER` a partition. `Query::window` carries them and they land
after the computed values, so a filter, a sort key or another window addresses
one by ordinal the ordinary way. `EXPLAIN` reports how many run and that the
plan no longer streams; `ExecutionLimits` gains `max_window_rows` and the
daemon a `[limits] max_window_rows` key; the SQL front end refuses `OVER` by
name, saying the operator exists and where the gap is.

## Why

`orm-comparison.md`'s row had already worked out the shape and it was right in
both halves:

> `Grouper` holds `HashMap<encoded_key, (Vec<Value>, Accumulators)>`: it folds
> each row into its group's accumulators and **discards the row**. A window
> function emits one output row per *input* row … the opposite cardinality. So
> it cannot be added to the aggregate list; it needs an operator that preserves
> rows, which also changes what `max_groups` and `max_sort_rows` are bounding.

Both predictions held. The operator preserves rows, and the ceilings did need a
fourth: an `ORDER BY` with a `LIMIT` sorts with a bounded heap and a window has
no such escape, because it runs *before* the limit applies. `ROW_NUMBER() OVER
(…) … LIMIT 10` has to number every row before it can know which ten.

What it shares with `GROUP BY` is the arithmetic, and only that:
`WindowFunction::Over` wraps an ordinary `Aggregate` and reuses `Accumulators`,
including the exact-integer, decimal-aware running total that took a bug to get
right. Nothing about how values are added is written twice — which is also what
makes the oracle possible.

## Alternatives rejected

**Add the variants to `Aggregate`.** The obvious move and the one the row
already rules out: `Grouper::push` drops the row after folding it, so there is
nowhere for a per-row answer to go. Widening the enum would have meant
`group_by` returning something other than groups for some of its members.

**Implement `ROWS` semantics instead of `RANGE`.** One character in the loop —
read the accumulator per row rather than per peer group — and it is what a
plausible implementation does by default. It is also wrong: SQL's default frame
under an `ORDER BY` is `RANGE`, so rows tied on the order column are *peers* and
all see the value that includes all of them. `ROWS` gives four different running
totals to four rows of the same year. Both are legal SQL and only one is the
default, and picking the non-default silently is a number that looks right.

**Offer explicit frames (`ROWS BETWEEN 3 PRECEDING AND CURRENT ROW`).** A real
feature and a much larger one: an arbitrary frame turns one forward pass into a
sliding window with its own eviction, and several aggregates cannot be
un-accumulated at all — `MIN` over a shrinking frame needs a monotonic deque,
not a subtraction. Left out and said so, rather than approximated.

**Reuse `max_sort_rows` for the buffer.** One less field, and it would have
reported `SortTooLarge` for a query with no `ORDER BY`. Worse, the two bound
different risks: a deployment that raised the sort ceiling did so knowing its
sorts carry a `LIMIT`, which says nothing about a window, since no `LIMIT`
bounds one. The error message now says that explicitly, so a caller does not
retry with a limit and get the same answer.

**Permit an unordered `ROW_NUMBER`/`RANK`, as Postgres does.** `RANK` and
`DENSE_RANK` would be 1 on every row; `ROW_NUMBER`, `LAG` and `LEAD` would step
through whichever order the access path happened to produce. Refusing costs a
line and the error names the fix; allowing it later stays compatible, and
allowing it now would be a number that changes when the planner changes its
mind.

**Move rows into partition order and compute in place.** Simpler to write, and
it leaves the result in the last window's order, so a query's own `ORDER BY`
would have to re-sort from scratch and two windows with different partitions
would fight. Instead each distinct `(partition, order)` specification sorts a
`Vec<usize>` of indices and writes results back by index; rows never move.

**Widen `plan_hinted` with a window parameter, to pin the columns it reads.**
The obvious place, and it would have been a tenth argument on a `pub` function
already at nine. Widening the *projection* in `SecuredReads::plan` says the same
thing — "these columns must come off the row" — in the vocabulary that already
exists, needs no signature change, and correctly stops a covering index that
lacks them being chosen.

**Answer a window on a keyset page.** `ROW_NUMBER` would restart at 1 on every
page and a running total at zero, both plausible numbers for a partition nobody
asked about. Refused, on `paging` as well as on a cursor already in hand, so the
first page fails rather than the second — the same argument `no_cursor_on_groups`
makes and the reason `Query::paging` exists.

## Evidence

**The oracle is `GROUP BY`.** `SUM(x) OVER (PARTITION BY p)` on every row must
equal the `GROUP BY p` aggregate for that row's `p`. The two share the
accumulator arithmetic and nothing else — one hashes the key and drops the row,
the other sorts indices and keeps it — so agreement is evidence rather than a
restatement. `a_partition_aggregate_agrees_with_the_same_group_by` checks all
seven aggregates the two share. The ranking functions have no sibling, so they
are held to properties instead: a partition's row numbers are exactly `1..=n`,
and ordering by the window's keys gives the same sequence as ordering by the
number it assigned.

15 kernel tests and 3 SQL-front-end tests, all passing; `slate-kernel`,
`slate-server`, `slate-serverd` and `slate-wasm` suites green unfiltered;
`sh scripts/check.sh` 32 of 32.

**Twenty mutations, eighteen caught, one recorded, one a real finding.**

The finding: nothing tested that a window *orders* by a column outside the
projection. There was a test for the *partition* column, and deleting the order
columns from `Window::collect_columns` left it passing — a partition column read
as null collapses the partitions, an order column read as null makes every row a
peer of every other, and they fail differently.
`a_window_orders_by_a_column_outside_the_projection` catches it.

**And one test that passed for the wrong reason.** `the query's sort runs after
the window` survived its mutation. The test ordered by a window partitioned by
`region`, whose three totals are all 90 — the seed's amounts cycle with period
30 and the regions with period 3 — so every row carried the same value and *any*
order satisfied "descending". Repartitioned by `year` (30, 42, 54, 66, 78), and
the distinctness is now asserted so the next change to the seed cannot quietly
restore the hole.

**The recorded survivor** is the shared-permutation optimisation: two windows
with the same `OVER` clause sort once rather than twice. Its only observable
effect is time, so no assertion about a result can see it, and asserting on
wall-clock is what this repository says not to do. Measured instead, 200k rows
and four windows, five runs each: **one shared specification 460–508 ms, four
distinct ones 875–898 ms**. The comparison is not perfectly clean — the distinct
case also partitions by different columns, so its boundary detection differs
slightly — but the dominant term is three extra sorts of 200k indices, which is
what sharing removes. Recorded with `expect_survivor` and that reason.

**A repository guard caught what I forgot.** `slate-server`'s
`every_error_is_classified` failed on all four new `KernelError` variants at
once: without a status code they fall to the wildcard and a caller reads them as
the server having broken. `WindowTooLarge` is `ResourceExhausted` with the other
five memory guards; the three specification refusals are `InvalidArgument`,
because the remedy is a different query rather than a smaller one.

## What this does not do

**Nothing outside Rust can ask for a window.** There is no protocol message, so
`convert.rs` pins the field empty on every request and the three clients have no
surface. That is the whole of `F2a`, and it is the split arrays took: kernel
first, wire second. The `wire.rs` round-trip generator pins `window` to empty
with a comment saying it becomes generated when there is a message — the way
`include_deleted` did.

**The SQL front end refuses `OVER` rather than parsing it**, and the workbench
is the awkward case: it runs the kernel *directly* in the browser, so it could
support windows with no wire at all. It does not, because `QuerySpec` has no
window field and the headers, labels and result rendering all key off it. The
refusal names both gaps, which means it goes stale if only one closes.

**No window over a join or a chain.** `Query::window` is on the single-table
read; the joined and chained readers have their own spec types and their own
ordinal spaces. Nothing refuses it there because nothing can express it there,
which is a weaker guarantee than a refusal and is worth knowing.

**`COUNT(DISTINCT … ) OVER (… ORDER BY …)` is refused, and the unordered form is
not.** The running version needs the distinct set as it stood at each peer
group, and the only way to keep those is a copy per group — quadratic in the
partition. Partial support is a trap, and this one is documented rather than
hidden; the alternative was refusing a form that works.

**The window pass is single-threaded and holds decoded rows**, not encoded ones.
A partition of a million wide rows is a million decoded rows in memory, which is
what `max_window_rows` is for and is not an optimisation anybody has attempted.
Nothing was measured about the per-row cost of the operator itself, only about
the sharing above.
