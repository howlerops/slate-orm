# A fifth per-request ceiling, and the concurrency limit that bounded politeness rather than load

- **Date:** 2026-09-29
- **Author:** Claude Code, task K5
- **Touches:** `crates/slate-kernel/src/limits.rs`, `crates/slate-kernel/src/expr.rs`,
  `crates/slate-kernel/src/exec.rs`, `crates/slate-kernel/src/error.rs`,
  `crates/slate-kernel/src/lib.rs`,
  `crates/slate-kernel/tests/security_probe_resources.rs`,
  `crates/slate-server/src/status.rs`, `crates/slate-serverd/src/config.rs`,
  `crates/slate-serverd/src/main.rs`, `crates/slate-serverd/src/serve.rs`,
  `crates/slate-serverd/tests/ceilings.rs`, `docs/security-review.md`
- **Kind:** security

## What changed

Two ceilings that three caveats said were missing or misnamed.

**`max_in_values`, the fifth `ExecutionLimits` field.** An `IN` list is checked
against every row the scan reaches, so its cost is the list's length times the
rows scanned, and nothing bounded the length. It is 10,000 by default, checked
in `QueryCursor::open` — where every read and every predicate write meets — and
raises `KernelError::InListTooLarge { limit, actual }`, which the daemon
classifies as `IN_LIST_TOO_LARGE` / `ResourceExhausted`. `[limits]
max_in_values` sets it.

**`max_concurrent_requests` is node-wide now.** It was
`concurrency_limit_per_connection`, so a caller who opened a second socket got
a second allowance: the setting bounded one channel and its name claimed to
bound the node. It is `tower::limit::GlobalConcurrencyLimitLayer` — one
semaphore for the process — layered outside the per-connection stack.

## Why

Both were recorded as open, and both are the kind of gap that reads as closed.
`max_concurrent_requests` is the worse of the two: an operator setting it
believes the node is bounded, and the defence is one word inside a method name
nobody re-reads. The caveat named the fix — "a node-wide bound needs a
semaphore in the service" — and the semaphore turned out to be one tower layer.

The `IN` ceiling is the fifth of a set whose first four bound *state the node
accumulates while answering*. This one bounds an *input the caller sends*,
which makes it the only one that can refuse before a row is read, and the only
one whose number is chosen against what the caller is describing rather than
against memory: matching ten thousand keys is a join, and saying so gets a
build side the planner can cost, where an `IN` list of the same keys is a
filter it can only apply.

## Alternatives rejected

**Capping the list at the wire instead of in the kernel.** `convert.rs` already
refuses a predicate nesting past `MAX_EXPRESSION_DEPTH`, so the `InList` arm
was the obvious place. Rejected because the kernel is reachable without the
wire — `slate-orm`, the wasm binding, the SQL front end — and a ceiling that
only the gRPC path enforces is the shape of finding 1, where a refusal covered
one of two catalog constructors. `QueryCursor::open` is the one place all of
them meet.

**Summing the lists in a predicate rather than taking the longest.** Two lists
of 6,000 cost what the longer one costs, twice — they are checked one at a
time. A sum would refuse a predicate that is cheaper than one it accepts, which
is a ceiling that does not describe a cost.

**Counting only `Expr::In` and not `Expr::InSorted`.** They are the same list;
`prepared()` turns one into the other above `IN_LOOKUP_THRESHOLD`. A walker
that saw only the unoptimised form would fire on short lists and never on long
ones, which is exactly backwards. The doctest asserts the prepared form counts.

**Checking the caller's filter before the security compiler merges the policy
in.** That would leave a policy's own `IN` list uncapped, which is the
direction that matters — a policy is generated, and a generated predicate is
where a long list comes from without anybody choosing it. The cost is that a
policy's list counts against the caller's budget; the ceiling is three orders
of magnitude above any policy this repository generates, so that is a
theoretical unfairness. Written on the check.

**Keeping `concurrency_limit_per_connection` beside the global one.** Two
limits with one setting is a config nobody can reason about, and the
per-connection one adds nothing once the node-wide one exists: a single client
hammering one channel is bounded by the global limit too.

**Renaming the TOML key to match what it used to do.** Rejected before, and
still right: renaming a shipped key breaks every file that sets it. What was
wrong was the behaviour, not the name, and the behaviour is what changed.

## Evidence

Three new tests in `crates/slate-kernel/tests/security_probe_resources.rs`,
all passing, 16 of 16 in that file:

- `an_in_list_past_the_ceiling_is_refused_before_a_row_is_read` — 11 values
  against a ceiling of 10, refused with both numbers named.
- `an_in_list_at_the_ceiling_is_served` — 10 values against 10, served, ten
  rows back. The control, and it is not decorative: without it a ceiling of
  zero refuses every `IN` and passes the test above, which is the `>` against
  `>=` mistake and is one character.
- `a_predicate_write_is_bounded_by_the_same_ceiling` — `delete_where` with 11
  values, refused. Asserted rather than read off the call graph, because "they
  share a function" is exactly the kind of claim this repository has been wrong
  about.

**The ceiling caught an existing test, which is the finding inside the
finding.** `a_large_in_list_no_longer_costs_the_list_length_per_row` sends
50,000 values to measure the hash-lookup path, and a default store now refuses
it. It opts out with `max_in_values: usize::MAX`, and
`a_list_this_large_is_refused_by_default` is written beside it so the opt-out
cannot quietly become the shipped behaviour: it fails if 50,000 stops being
over the default.

**A second staleness, found the same way.** `the_default_limits_are_not_unbounded`
rostered three fields when there were four — a `max_window_rows` of
`usize::MAX` would have shipped green past it. It destructures
`ExecutionLimits` now, so a sixth field added without a row will not compile.
`slate-kernel`'s own re-export had the same gap: `DEFAULT_WINDOW_LIMIT` was
never public.

Two mutations against `crates/slate-kernel/src/exec.rs`, both caught, recorded
in `ledger/mutations/20260929T005056-crates-slate-kernel-src-exec-rs.json`.
Deleting the check fails all three of the new refusal tests; `>` to `>=` fails
`an_in_list_at_the_ceiling_is_served` and nothing else, which is the control
earning its place — without it that mutation survives. The
doctest on `widest_in_list` covers the walker: longest rather than sum, through
a `Not`, zero for a predicate with no list, and the prepared form.

## What this does not do

**Nothing has seen the concurrency limit bind.** The permit is released when
the response future resolves, and for a server-streaming RPC that is before any
row is read — so a held-open stream holds no permit and a client cannot keep
one long enough to watch a refusal. That property is unchanged by making the
limit node-wide; only its *scope* is fixed. `tests/ceilings.rs` says so where a
reader arrives, and asserts the whole of what a client can see.

**It bounds admission, not open streams.** A node-wide cap on concurrent
response streams is a different mechanism and does not exist. A caller who
opens ten thousand slow streams is bounded by nothing here.

**The `IN` ceiling is not measured against a real cost.** 10,000 is argued for
— a list that long is a join — and not derived from a measurement of what
10,000 values cost per row on this hardware. The number is documented as
untuned, like the other four.

**No client refuses a long list before sending it.** The refusal arrives from
the server after the request is encoded and decoded, so the bytes are spent. A
client-side check would save that, and none of the three has one.
