# An IN list is arranged once per plan, not scanned once per row

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `slate-kernel` — `expr.rs`, `plan.rs`, `stats.rs`, `tests/security_probe_resources.rs`
- **Kind:** security

## What changed

`Expr::InSorted` holds an `IN` list sorted, deduplicated and with nulls lifted
into a flag. `Expr::prepared()` rewrites any `In` at or above
`IN_LOOKUP_THRESHOLD` (16) into it, and plan construction calls that once, so a
candidate row costs a binary search rather than a scan of the caller's list.

## Why

Security review finding 7, first of four. `MAX_POINT_GETS` and
`MAX_INDEX_RANGES` stop a long list *becoming an access path*; they leave every
value in the residual, where `Expr::In` scanned the list linearly per candidate
row. The list length is chosen by the caller and multiplied by the table's row
count, so one authenticated request could pin the node: measured at 8 values
8 ms, 50,000 values 2.98 s over 2,000 rows — 362x.

## Alternatives rejected

**Cap the number of values a predicate may carry**, refused at
`expr_from_proto`. The review's other suggestion, and it refuses legitimate
queries to avoid a cost that turns out to be avoidable. A cap would still be
worth having as defence in depth, but not *instead* of making the operation
cheap — capping first would have left the quadratic behaviour in place under
the cap.

**A `HashSet`.** The obvious lookup structure and the wrong one here: `Value`
holds `f64` and `Vec<f32>` variants, so it is not `Hash`, while it *is* `Ord`
with a total order byte-identical to its encoding. Sorting is the structure the
project already has.

**Rewrite at cursor open rather than plan construction.** Same result for a
single scan and worse for everything else: a plan's residual is evaluated by
the executor, join probes and chain steps, so preparing per plan pays the
arrangement once for all of them.

**Rewrite every `IN`, however short.** A linear scan of a handful of values
beats a binary search on cache behaviour, and rewriting unconditionally would
allocate for every trivial `IN` in exchange for nothing. Hence the threshold —
which is chosen, not tuned: well below where the amplification matters and well
above where the rewrite could cost anything.

## Evidence

Five runs after the change: `IN(8)` 4.20–4.49 ms, `IN(50000)` 17.25–19.87 ms,
ratio **4.03–4.68x** (was 362x). What remains is consistent with the one-time
sort of 50,000 values rather than per-row work; that attribution is reasoning,
not a separate measurement, and is not claimed as one.

Three mutations, all killed:

- Dropping the sort fails the agreement oracle and the estimate test.
- Dropping `dedup` fails `a_repeated_value_is_not_counted_twice_in_the_estimate`
  — which did not exist until this mutation survived. `stats.rs` multiplies
  per-value selectivity by list length, so `IN (3, 3, … )` estimated forty
  times too many rows before deduplication. The comment claiming the prepared
  form fixes that is now asserted rather than asserted-at.
- Forcing `any_null` false fails the oracle on the three-valued case.

The oracle itself (`the_prepared_in_agrees_with_the_plain_one_on_every_input`)
checks both forms against each other *and* against an independently computed
expected answer, over empty, duplicate-laden, null-laden, unsorted and
above-threshold lists — consistency alone would pass if both were wrong the
same way.

918 tests pass across the workspace, fmt and clippy clean.

## What this does not do

Three of finding 7's four items are untouched here: `GROUP BY` still holds one
entry per distinct key with no cap, `COUNT(DISTINCT)` every distinct value, and
an unlimited `ORDER BY` the whole result. The daemon still has no concurrency
limit and no request timeout.

It does not cap the list. A caller can still send a very large `IN` and pay for
the sort; that cost is now `O(n log n)` once rather than `O(n)` per row, which
changes the shape of the problem but does not bound it.

The two `Expr` forms deliberately disagree on estimated selectivity, because
the plain one was wrong. Anything comparing plans across the rewrite boundary
would see that difference.
