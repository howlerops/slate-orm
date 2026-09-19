# A purge something can call

## What changed

`PurgeDeleted` is an RPC. The head node serves it, all three clients call it, a
conformance case exercises it, and seven Rust tests pin what it does.

And a finding about the conformance suite that matters more than the feature:
**it cannot catch a server-side bug at all.**

## Why an RPC, and not a CLI flag

The obvious shape was `slate-serverd --purge TABLE --older-than 30d`, next to
`--check`, `--print-schema`, `--seed` and `--plan`. It is wrong here, and the
reason is structural.

A purge is a **write**, and writes in this system go to the holder of an
object-store lease. An out-of-band process doing one would have to take that
lease — which **fences the node that is serving**. A maintenance tool whose
documented side effect is "your head node stops writing" is not a tool you put
on a cron.

Asking the node that already holds the lease has none of that. It also gets
row-level security for free: the purge runs as the caller, under the caller's
policy, reaching exactly the rows that caller could have deleted.

## Alternatives rejected

**The CLI flag, with a warning.** Rejected above. Worth adding later as an
*offline* tool for a node that is stopped, where the lease is nobody's.

**A dry run.** Attractive for a destructive operation, and the elegant version
is free: run the purge in a transaction and roll back, so the count is exact
and the predicate is not restated. Not built because the ceiling covers the
mistake that actually happens — a mistyped `before` — by refusing rather than
erasing, and a dry run whose answer is stale by the time you act on it is a
weaker guarantee than a refusal at the moment of the write.

**A duration rather than an instant.** `--older-than 30d` is what an operator
wants, and it would put a retention policy in the protocol. The caller
subtracts; the wire carries a timestamp.

**Refusing a purge inside a transaction.** It would have saved the session
plumbing. Rejected because "retire what is stale, then forget what is old" is
one unit of maintenance, and a caller should not have to leave the transaction
to do half of it.

## The conformance suite's blind spot

Three mutations of the RPC — reporting a count it never did, ignoring the
ceiling, authorising `Read` where `Delete` was meant — **survived a full
96-case conformance run**. 0/3 caught.

The reason is structural too, and is the same shape as the gap `MUST_DIFFER`
closed an hour ago. That runner compares the three SDKs *to each other*. A
mutation in the server changes all three answers identically, so all three
still agree, and the run is green. It holds no expected values, so a server
answering `affected: 0` for every purge satisfies it perfectly.

The suite is for client divergence. Server behaviour needs a test that knows
the answer, which is what `crates/slate-server/tests/purge_wire.rs` is, and its
module docstring says so, because the next person to add a server feature will
reach for the conformance suite first.

## Evidence

Seven tests over the wire, and the mutations re-run against them:

| mutation | conformance | `purge_wire.rs` |
| --- | --- | --- |
| reports a count it did not do | survived | caught |
| the ceiling is ignored | survived | caught |
| `before` is ignored | — | caught |
| authorises `Read` instead of `Delete` | survived | **survives, by design** |

The last one is unobservable and stays. The kernel authorizes `Delete` again,
so weakening the handler's check leaves the purge still refused one layer
lower — `a_caller_who_may_see_retired_rows_still_cannot_erase_them` keeps
passing. `delete_where` three handlers up records exactly this about exactly
this line; the new handler now says it too, rather than leaving a reader to
rediscover it.

Two roles in the fixture hold one half each of what a purge needs:
`purger_blind` has all four data actions and not `read_deleted`, `watcher` has
`read` and `read_deleted` and not `delete`. Both are refused, which is how the
two-grant requirement is pinned rather than asserted.

`./run.sh --conformance`: 96 cases, the three SDKs agree on all of them. That
case earned its place by finding something the Rust tests could not: the first
adapter to run purged three rows and the other two purged two, because a purge
is table-wide and the first also erased the row the demo seeder retired. Each
handler now normalises the table, runs its experiment, and puts the seeder's
row back — so the corpus has no ordering rule for someone to break later.

`cargo clippy --workspace --all-targets` clean (it caught an
`unnecessary closure used with bool::then` on the way, which is `-D warnings`
in CI), 86 Rust suites passing, `gofmt`/`go vet`/`tsc`/`ruff`/`ty` clean, and
the Python stub-freshness test passing on regenerated stubs.

## What this does not do

**No scheduler.** Nothing runs a purge periodically. A deployment wanting one
needs an external cron calling the RPC, and there is no example of that here.

**No offline tool**, so a stopped node's keyspace cannot be purged at all.

**No metric.** `affected` comes back to the caller and nothing counts purged
rows server-side, so a scheduled purge's effect is invisible to `/metrics`.

The ceiling is a count, not a byte budget or a deadline, and it refuses rather
than purging a prefix — a caller with ten million retired rows must loop with
its own advancing bound, and nothing here helps.

The conformance purge case runs as `app`, which holds everything. The refusals
are covered in Rust only, so the three clients are not compared on what a
refused purge looks like the way they are for `include_deleted`.
