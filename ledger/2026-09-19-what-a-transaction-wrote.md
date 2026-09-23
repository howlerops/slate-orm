# A transaction's writes are counted too, once it has committed and not before

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `crates/slate-server/src/session.rs`,
  `crates/slate-server/src/service.rs`,
  `crates/slate-server/tests/transaction_counts.rs`,
  `crates/slate-serverd/src/observe.rs`
- **Kind:** fix

## What changed

`WriteObserver` now hears about all three write paths instead of one. A
transaction's task keeps a `Tally` — a map from (statement, table) to a row
count — that every write arm adds to, and reports it to the observer on a
successful `Commit` and on nothing else. An atomic batch that opens its own
transaction returns its per-operation counts out of the retry closure and
reports them after that transaction commits. `Decoded::apply` answers a label,
a table and a count rather than `()`, which is where those come from.

## Why

`slate_rows_written_total` shipped counting only what went through
`autocommit`, which is standalone writes. A deployment that wrapped its writes
in `Begin`/`Commit` — which is most of the reason to run a database with
transactions — reported zero rows written, forever, under any load. That is
worse than having no metric: an operator reads a flat zero as "nothing is
happening" rather than as "this does not measure you", and the previous entry's
gap paragraph was the only place it was written down.

The batch path was invisible for a second reason worth separating: an atomic
batch with no caller transaction opens one, applies everything and commits, all
inside the one call. It never touches `autocommit` and it never touches the
session either, so it fell between the two.

## Alternatives rejected

**Count at the write, like every other counter in the daemon.** By far the
least code — one `observer.wrote(...)` in each arm of `session::apply` and
nothing else. It reports rows that do not exist. A transaction is *allowed* to
be rolled back, to time out, to be fenced, or to be abandoned by a client that
disconnects, and all four are ordinary. A counter cannot go down, so the first
rollback puts a permanent lie in the series, and the lie is in exactly the
direction that makes a broken deployment look busy.

**Count at the write and correct on rollback.** Impossible for a Prometheus
counter and a bad idea even where it is possible: the scrape between the write
and the rollback has already published the number.

**Report the transaction's total with no statement label.** One call per
commit, no map, no per-arm change. It loses the thing the labels are for: an
operator watching a retention sweep needs `delete_where` on its own, and a node
whose deletes were folded into its inserts reports a plausible total and
nothing usable. The map is nine lines.

**A `Command::Commit` that carries the counts back to `Sessions::commit`, and
report from the handler.** Keeps the observer out of `session.rs` entirely,
which is tidier. It also puts the reporting where an `Err` can skip it by
accident, and the `run` loop already has the one place the transaction becomes
real. Reporting next to `transaction.commit().await` is the version where the
invariant is visible.

**Make the observer's contract "once per statement" everywhere.** Then a
transaction of ten thousand small inserts is ten thousand observer calls, all
on one series. The contract is now "once per statement for a standalone write
or an atomic batch, once per (statement, table) for a transaction" and it is
written on the trait, because an implementation that *set* rather than added
would be wrong under it.

## Evidence

Nine tests in `crates/slate-server/tests/transaction_counts.rs`, all over the
wire against a real serving head node with a recording observer attached. Eight
mutations, run against those tests:

| mutation | result |
| --- | --- |
| report after every command, not on commit | caught — 6 tests |
| report on `Rollback` too | caught — `a_rolled_back_transactions_writes_are_not` |
| never report at all | caught — 5 tests |
| insert's count becomes zero | caught — 4 tests |
| `delete_where`'s count becomes zero | caught — `each_statement_is_its_own_series` |
| every statement labelled `insert` | caught — 2 tests |
| the atomic batch reports nothing | caught — `an_atomic_batch_in_its_own_transaction_is_counted` |
| report when a command *fails* | **survived** — see below |

The survivor is the useful one. `answer` hands the loop back a `Some(error)`
only for `WriterFenced`, so "report on a failed command" is unreachable except
when a write inside a transaction is fenced — and then the report publishes the
tally of a transaction that is about to be abandoned, which is the bug. It is
narrow and it is real.

Reaching it in a test needs a real object store, a lease and a successor node
opening the same path: the setup `handover.rs` exists for, and its own fence
lands at `commit` rather than at a write. Rather than move that harness into
this file to cover one branch, `Tally::report` was changed to take `self`. The
loop then cannot move out of a value its next iteration uses, so the mutation
is a compile error rather than a silent bug — confirmed by applying it and
reading `E0382: use of moved value: tally`. The compiler is a cheaper oracle
than a test here and it does not need the fence to happen.

One test exists only because of that survivor's first form:
`a_refused_statement_does_not_flush_what_came_before_it`, which writes three
rows, is refused on a duplicate key, and asserts the observer heard nothing.
The rollback test could not see it — it has no failing statement.

The rest of `slate-server` (24 test binaries) and `slate-serverd` (8) pass, and
`cargo clippy --workspace --all-targets` is clean.

## What this does not do

~~**Still nothing scrapes `/metrics` over a socket in a test.**~~ **Closed**,
and the claim was wrong in a way worth naming: `observing.rs` had scraped a real
socket from a real process since the endpoint shipped, for the *request*
counters. I read the previous entry's gap paragraph and repeated it rather than
looking. What was true underneath is that `slate_rows_written_total` was never
scraped and `serve.rs`'s attaching line never run — including the commit rule
this entry is about. See `2026-09-19-the-counter-nobody-scraped.md`.

**A fence mid-transaction is covered by the compiler, not by a test.** Stated
above rather than buried: there is no test that fences a write inside a
transaction and asserts the observer stays quiet. What there is instead is a
signature that makes the wrong code fail to build.

**The counts are per node.** A transaction is pinned to the writer, so the
number is the writer's; there is nothing here that aggregates across a
handover, and a node that steps down takes its counters with it. That is the
ordinary Prometheus answer (sum across instances) and is said here only
because "rows written" reads like a cluster-wide quantity.

**A rolled-back transaction reports nothing at all, not even that it happened.**
An operator cannot tell "no writes" from "writes that were thrown away" from
this metric. A separate counter for abandoned transactions would say it; there
is no evidence yet that anybody needs it, and a metric added on speculation is
a metric nobody debugs.
