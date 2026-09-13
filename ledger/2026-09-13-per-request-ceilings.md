# Ceilings on the three operators that held unbounded state

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `slate-kernel` — new `limits.rs`, `aggregate.rs`, `exec.rs`, `read.rs`, `record.rs`, `pool.rs`, `error.rs`; `slate-server` — `service.rs`; `slate-serverd` — `config.rs`, `main.rs`, `serve.rs`
- **Kind:** security

## What changed

`ExecutionLimits` carries three per-request ceilings — distinct `GROUP BY`
keys, distinct values per `COUNT(DISTINCT)`, and rows an unlimited `ORDER BY`
may materialise — from `RecordStore` down through `SecuredReads` to the
`Grouper`, the `Accumulators` and the cursor's sort. Each refuses with an error
naming the limit. The daemon exposes all three in `[limits]`, plus
`max_concurrent_requests` and `request_timeout`.

## Why

Security review finding 7, the remaining three of four. A join has had
`DEFAULT_BUILD_LIMIT` since it was written, for exactly this reason —
materialising one side unbounded turns a mistyped join key into an
out-of-memory kill. Nothing else that holds state proportional to the data had
an equivalent, so one authenticated caller could pin the node by grouping a
large table on a unique column, or sorting it without a `LIMIT`.

## Alternatives rejected

**Truncate instead of refusing.** Cheaper and much worse: a silently short
answer is a wrong answer, and the caller cannot tell. Every limit here reports.

**Kill the request from outside** (a watchdog on memory). Bounds the node
without telling the caller anything actionable, and picks its victim by timing
rather than by who caused the problem.

**Approximate `COUNT(DISTINCT)`** — HyperLogLog and friends. This would remove
the memory question rather than bound it, and it is a different feature: the
current one is documented as exact, and callers rely on that.

**A concurrency limit and timeout with real defaults.** Deliberately unset. A
concurrency limit low enough to protect a small node is low enough to break a
large one, and a request timeout shorter than a legitimate analytical query
turns a slow answer into no answer. The defect was having no way to say one,
not the absence of a particular number — and a default here would be a number
chosen without knowing the machine, which is exactly the class of mistake the
readahead and Nagle findings came from.

**Reading `0` as "unbounded" in the config.** The tempting shorthand, refused:
`max_groups = 0` reads as "no grouping allowed" at least as naturally, and a
setting that disables the feature it appears to configure is the worst
available outcome. Unbounded is spelled by omitting the key, and
`ExecutionLimits::unbounded()` names it in code.

**Per-`Query` limits, the way `Join::build_limit` works.** Rejected because
these are the node protecting itself, not the caller expressing intent: a
client-supplied ceiling on the caller's own memory use is not a protection.

## Evidence

921 tests pass across the workspace, fmt and clippy clean. Three new kernel
tests set each ceiling to 10 against 100 rows and assert the specific error
variant and that the message names the limit; the sort test also asserts that
the *same* query with a `LIMIT` still succeeds, which is the claim its error
message makes.

Four new daemon tests: zero refused for each of the three ceilings with a
message naming the setting and saying how to mean unbounded, zero refused for
the concurrency limit, all five accepted together with a timeout, and — the one
that matters for anybody upgrading — a configuration omitting them entirely
still starts.

One gap was found by a failing test rather than by reading: the ungrouped
aggregate path built its `Accumulators` separately from the grouped one, so
`COUNT(DISTINCT)` without a `GROUP BY` was still unbounded after the `Grouper`
was wired. The test for it failed, which is how it was found.

## What this does not do

The three defaults (1M groups, 1M distinct values, 5M sorted rows) are chosen,
not tuned: far above any reasonable query and far below anything that threatens
a node. Nothing measures where a real node actually falls over, so a deployment
that cares should set them.

`max_concurrent_requests` maps to tonic's per-connection limit, so a caller who
opens more connections gets more concurrency. A true node-wide bound needs a
semaphore in the service, which this does not add.

The join's own `DEFAULT_BUILD_LIMIT` is untouched and still separate from
`ExecutionLimits`; the two should probably merge, and did not here because
`build_limit` is per-`Join` and client-settable, which these deliberately are
not.

Nothing here bounds the *number* of values in an `IN` list, or the size of a
decoded request. tonic's 4 MB default still admits a few hundred thousand
values; that is now cheap to evaluate rather than quadratic, but it is not
capped.
