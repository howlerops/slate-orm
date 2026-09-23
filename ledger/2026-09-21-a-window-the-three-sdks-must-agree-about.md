# A window endpoint in the demo, and the pairs that stop three clients dropping a field together

- **Date:** 2026-09-21
- **Author:** Claude Code, closing F2b
- **Touches:** `examples/explorer/CONTRACT.md`, the Go, Node and Python adapters, `examples/explorer/conformance/conformance.py`
- **Kind:** feature

## What changed

`POST /api/window` joins the demo's HTTP contract, implemented by all three
adapters, and the conformance runner gained seventeen cases over it — six
window functions partitioned and unpartitioned, two running aggregates, and one
as the `reader` whose row policy hides a book. Four `MUST_DIFFER` pairs keep the
`partition` and `running` fields honest.

## Why

Both F2a entries named the same gap and did not close it: each client's window
suite runs against a real head node and agrees with that node, which catches
*one* client being wrong and cannot catch three being wrong the same way. The
conformance runner is the only thing in this repository that compares the three
clients to each other, and it had no window case.

The failure it is for is not hypothetical here. A window value comes back in a
list of its own, and "in its own list" is a placement rather than a value: an
adapter that folded it into the row returns something the caller reads as a
different thing, and every other conformance case compares columns only. Three
adapters written by one author in one sitting are exactly the population most
likely to share a misreading.

## Alternatives rejected

**A general window builder over HTTP.** The body could carry a partition list,
an order list and an arbitrary function, and the runner could then generate
cases. Rejected for the reason `/api/join` gives about join keys: three
adapters parsing one little expression language is three parsers to keep in
agreement, and the first divergence would look like a database bug rather than
a contract bug. The fixed shape names one table, one partition column and one
order column, and varies the part that is actually under test.

**Leaving `partition` and `running` to the ordinary comparison.** They are the
class of field the runner cannot see: a client that drops one sends a smaller
request, gets a smaller answer, and agrees with two other clients doing the
same. `MUST_DIFFER`'s own note lists the three things that cover a field, and
neither of these is covered by a refusal — nothing refuses a window without a
partition, and a whole-partition aggregate is a valid answer rather than an
error. So both need a pair, which is case 2 of that note exactly.

**Seeding a tie so `RANK` and `DENSE_RANK` differ here.** The demo has no two
books by one author in the same year, so the two functions agree on every row
of it. Adding one would change the answers of a dozen existing cases for a
property this runner does not test: whether the answer is *right* is the
kernel's suite and each client's own. This runner asks whether the three agree,
and two functions that happen to coincide still exercise two different wire
encodings and two different builder calls. The contract says so where a reader
will look.

## Evidence

**118 cases, the three SDKs agree on all of them** — up from 101. The runner
already refuses the vacuous reading: a case whose three answers are an
identical *error* fails unless it is listed in `EXPECTED_REFUSALS`, and none of
these is, so all three returned rows.

**The `MUST_DIFFER` pairs were demonstrated, not asserted.** `partition` was
dropped in all three adapters at once — the only mutation that tests what the
pairs are for, since one adapter alone is caught by the ordinary comparison —
and the run came back:

```
118 cases, disagreements:

'a rowNumber window per author' and 'a rowNumber window' returned the same answer,
so whatever separates them was dropped by all three clients or ignored by the server
'a sum window per author' and 'a sum window' returned the same answer, …
```

Two pairs, and nothing else: every ordinary comparison still passed, which is
the whole point. The `running` pairs stayed quiet because `running` was
untouched, and they are non-vacuous for the complementary reason — a
`MUST_DIFFER` pair fails when the two answers *match*, so the green run is the
evidence that a running aggregate and a whole-partition one differ.

Applied and restored by a script with the anchors asserted to occur exactly
once and the restore in a `finally`, for the reasons `scripts/mutate.py` gives
at length. `mutate.py` itself could not drive this: its dialects read a test
runner's output and `run.sh --conformance` is not one.

**`sh scripts/check.sh` 34 of 34**, which covers `gofmt`, `go vet`, the
TypeScript typecheck and both `ruff`/`ty` pairs over the three adapters.

## What this does not do

**It does not check the answers are right.** Three clients agreeing is
evidence about the clients and none about the kernel; correctness lives in
`crates/slate-kernel/tests/windows.rs` and in each client's own suite, which
have the ties this fixture lacks.

**No refusal case.** An unordered `RANK`, a running `COUNT(DISTINCT)` and an
offset of zero are refused by the server and each client's suite asserts it,
but the endpoint's fixed shape cannot ask for them — it always supplies the
order those functions need. A refusal case here would need a field whose only
purpose is to produce one, which is a shape nobody uses.

**The demo's UI does not call it.** The endpoint exists for the contract and
the runner; the SolidJS frontend has no window panel, and adding one is a
separate piece of work from proving the three clients agree.

**Windows on a join are not covered**, because the kernel refuses one on a join
input and the contract has no shape for a window over the joined row. That is
recorded in the SQL front end's entry and is the same boundary.
