# A batch is a round trip; a transaction is a guarantee

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-server/{proto,src/{service,session,status}.rs}`, `crates/slate-serverd/src/{config,main}.rs`, `crates/slate-server/tests/batch.rs`, all three clients' generated stubs, `README.md`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

A `Batch` RPC carrying several writes in one request, with a **required**
`atomicity`. Twelve tests, nine mutations, no survivors. `max_batch_operations`
is a new server limit, defaulting to 1,000 and settable in `[limits]`.

## Why

Fifty single-row inserts over loopback against the in-memory store take 46–49
ms. The same fifty as one batch take 4.9–5.1 ms. Five runs, **9.4× to 10.0×**,
and the spread is tight because the thing being saved is structural rather than
statistical: fifty round trips become one.

The design question this item posed was not performance, though. It was that a
batch and a transaction are different guarantees, and that conflating them is
how somebody ends up believing they have one when they have the other. So
`atomicity` is required and `ATOMICITY_UNSPECIFIED` is refused with a message
naming both options. The two differ *only when something fails*, which is
exactly when a caller who never chose finds out — and a zeroed request must
therefore mean neither.

The consequences follow from that one decision:

- `INDEPENDENT` reports one result per operation, and a failure is one of those
  results rather than the end of the batch. `an_independent_batch_reports_a_failure_and_carries_on`
  inserts a duplicate between two good rows and finds both good rows landed.
- `ALL_OR_NOTHING` reports *no* per-operation results, because they all
  happened or the request failed and none did. A list of successes would invite
  a caller to check it and could only ever say "yes".
- An atomic batch may join a caller's open transaction and does not commit it.
  An independent one inside a transaction is refused: "independent operations,
  all of which roll back together" is two contradictory requests.
- An operation's own `transaction` field is refused rather than ignored,
  because a caller who set it is asking for something else.

## Alternatives rejected

**`bool atomic`.** One field instead of an enum. Its false value is what a
zeroed struct sends, so a client that had never heard of atomicity would be
asking for independence without knowing — which is the failure this whole item
is about. The enum with a refused zero is the same argument the `Unit` idiom in
this proto already makes, applied to a choice rather than a flag.

**Default to `ALL_OR_NOTHING`, as the safer guarantee.** Tempting, and wrong
for the same reason: a caller who wanted a network optimisation and got a
transaction has their batch fail whole because one row was already there. Safe
defaults are for choices where one answer is always acceptable. Neither is,
here.

**New request messages for the operations, without `transaction`.** Cleaner
than reusing `InsertRequest` and friends, whose `transaction` field means
nothing inside a batch. It also means a batched insert and a lone insert are
different messages that must be kept in agreement forever. Reused, with the
field refused rather than ignored, so the wart is loud.

**Decode lazily, as each operation is applied.** Less memory for a large batch.
Under `INDEPENDENT` it would apply the first eight operations and then fail on
the ninth's malformed schema claim, handing the caller a "bad request" error
beside eight rows that had already landed.
`a_malformed_operation_stops_the_batch_before_any_of_it_runs` is that case, and
the mutation making the decode lazy is killed by it.

**A `Write` enum reused directly instead of `Decoded`.** `Write` borrows its
rows from the request, and a batch must own them so the request can be consumed
before the first write lands. `Decoded` owns, and `as_write` borrows back — so
both paths run the *identical* apply a lone RPC runs, which is what stops a
batched upsert from quietly diverging from a lone one.

## Evidence

Twelve tests in `batch.rs`. Nine mutations, no survivors:

| mutation | caught by |
| --- | --- |
| unspecified atomicity defaults to INDEPENDENT | `an_unspecified_atomicity_is_refused` |
| an empty batch is allowed | `an_empty_batch_is_refused` |
| decoding is lazy | `a_malformed_operation_stops_the_batch_before_any_of_it_runs` |
| an operation's own `transaction` is ignored | `an_operation_may_not_name_its_own_transaction` |
| INDEPENDENT inside a transaction is allowed | `an_independent_batch_may_not_run_inside_a_transaction` |
| an atomic batch reports per-operation results | `an_atomic_batch_reports_no_per_operation_results` |
| the operation index is dropped from a refusal | `a_malformed_operation_stops_the_batch_before_any_of_it_runs` |
| `returning` is ignored inside a batch | `a_batch_carries_every_kind_of_write` |
| an independent batch stops at the first failure | not expressible — the early return could not be written without changing the function's type |

The measurement is `a_batch_is_one_round_trip`. Round trips are **counted**,
not timed: the loop that sends them singly *is* the count, 50 against 1. Wall
clock is printed and only weakly asserted (batching must not be ten times
*slower*), because a threshold on a shared runner is a flake waiting for a slow
morning. The 9.4–10.0× above is from five runs recorded by hand.

Python 202, Go green, TypeScript 100, **77 conformance cases with the three
SDKs agreeing**. `cargo fmt --all` and `clippy --workspace --all-targets` with
`-D warnings` clean — after four findings, two of them lints that the narrower
`-p` command does not reach.

All three clients' generated stubs were regenerated in this commit rather than
the next one, which is the lesson from CI run 104: the same proto change went
red there because the Python stubs and the TypeScript proto copy were stale.

## What this does not do

**No client can call it.** The RPC exists and is tested; Python, Go and
TypeScript have no `batch` method, and the conformance corpus does not compare
one. This is the third time in this branch a server feature has landed a commit
ahead of its clients, and the honest reading is that it is a habit rather than
an accident.

The measurement is loopback against an in-memory store, which is the *least*
favourable case for batching: a round trip is microseconds there, and the 9.4×
is mostly per-request framing and authorization rather than network. On a real
network the ratio should be higher, and nothing here shows that — the deployed
harness in `examples/deployed` would be the place, and it does not exercise
`Batch`.

`max_batch_operations = 1000` is a guess, not a measurement. It is a guard
against a request nobody should send rather than a tuning knob, and it is
documented as such — but if somebody finds 1,000 too low, no evidence here
argues with them.

Reads are not batchable. A batch carries writes only, because a batch of reads
is already expressible as one query with an `IN`, and mixing them raises a
question this design does not answer: what a read inside an `INDEPENDENT` batch
should see of the writes beside it.
