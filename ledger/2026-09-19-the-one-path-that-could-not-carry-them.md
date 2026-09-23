# A batched refusal carries its check violations, which was the one path that could not

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `crates/slate-server/proto/slate/v1/records.proto`,
  `crates/slate-server/src/service.rs`, the three clients and their generated
  stubs, the three demo adapters, `examples/explorer/conformance/`
- **Kind:** feature

## What changed

`BatchError` grew `bytes details = 4`, filled with the same `google.rpc.Status`
a lone failure carries in `grpc-status-details-bin`. The three clients decode
it in `fromBatchError` / `from_batch_error` with the *same* function they
already use for a lone refusal, so `Violations` / `violations` is populated
whether or not the write was batched. Then `/api/bad-batch` in the three demo
adapters and a conformance case over it.

## Why

The gap three entries have now recorded: an independent batch reports each
failure as *data* inside a successful response, so there are no trailers, no
details blob, and nothing to decode. A caller submitting a form as a batch got
the reason token and the prose — which is where every client was before the
check decoders were written, and a batch is exactly how a form with several
rows is submitted.

`V1`, `V2`, the three decoders and the live conformance case all exist to make
one round trip enough for a refused form. The one path a multi-row form
actually takes could not carry the answer.

## Alternatives rejected

**A repeated `CheckViolation` message on `BatchError`.** The obvious shape, and
it types the thing rather than handing over bytes. It also makes a fourth
encoding of one fact: the server would serialise violations twice, in two
shapes, and each client would decode two shapes and could come to disagree
between them. Opaque bytes means one encoder and one decoder per language, and
a batched refusal that *cannot* differ from an unbatched one — which the Rust
test asserts directly, by comparing the two blobs rather than by checking the
batched one is non-empty.

**Only the `check.N` keys, as a map.** Smaller on the wire and it discards the
rest of the `ErrorInfo` — the domain, the variant payloads a future decoder
might want. It also means the batch path publishes a *different, narrower*
thing than the lone path, so "what does a refusal carry" would have two
answers.

**Re-raise the operation as a real status with trailers.** Then the existing
decoder works untouched. It is also the one thing an independent batch must not
do: the request succeeded, and turning an operation's failure into the call's
failure is the guarantee `ALL_OR_NOTHING` exists to provide separately.

**Leave it, and tell callers to submit forms one row at a time.** Honest, and
it gives up the round-trip saving that is the whole reason the batch RPC
exists. Worse, it is advice nobody would find: nothing in the client says a
batched refusal is less informative.

## Evidence

Four mutations, one per layer, all caught:

| mutation | result |
| --- | --- |
| the server sends `Vec::new()` for `details` | caught — `an_independent_batch_reports_a_failure_and_carries_on` |
| Go's `fromBatchError` stops decoding | caught — `TestABatchedRefusalCarriesTheSameViolations` |
| Python's `from_batch_error` stops decoding | caught — `test_a_batched_refusal_carries_the_same_violations` |
| TypeScript's `fromBatchError` stops decoding | caught — "a batched refusal carries the same violations" |

Nine client tests, three per language, against the captured `CHECKS_BLOB`: the
three-violation blob decoded, an absent blob yielding nothing, and rubbish
yielding nothing rather than raising.

The Rust test asserts the batched blob is **byte-identical to the one the same
row's lone insert carries**, which is the invariant rather than a proxy for it.

99 conformance cases, the three SDKs agreeing on all of them, with the new case
among them. Its answer, printed out of the runner rather than assumed:

    {"outcomes": [
      {"kind": "invalid-request", "reason": "CHECK_VIOLATION",
       "violations": [{"check": "status_known", "column": "status"},
                      {"check": "id_is_seeded", "column": "id"}]},
      {"kind": "invalid-request", "reason": "CHECK_VIOLATION",
       "violations": [{"check": "id_is_seeded", "column": "id"}]}]}

Two operations, refused for *different* reasons — two violations and one. An
adapter reporting the same list for every failed operation would look correct
against a batch where both failed alike, and is caught here.

`sh scripts/check.sh` is 19 passed, which includes regenerating the Go and
Python protobuf stubs' freshness checks by way of the suites that diff them.

## What this does not do

**An atomic batch still reports nothing per operation.** `ALL_OR_NOTHING`
fails the whole call, so the refusal arrives as a real status with real
trailers and the *lone* decoder handles it — with one difference that matters
to a form: the status names the first operation to fail and not which one, so a
caller cannot map violations onto the row that caused them. That is a property
of the atomicity rather than of this change, and it is the reason a form should
use `INDEPENDENT`.

**Nothing says which operation a batched failure belongs to except its
position.** `results` is positional, which is the existing contract and is
enough. Said because a reader looking for an operation id will not find one.

**`details` is unbounded in principle.** A row breaking fifty checks carries
fifty messages in the body, once per failed operation. The server's own
`violations` list is bounded by the number of checks a table declares, which is
schema-controlled rather than caller-controlled, so this is not a way to make a
node emit more than it was configured to. Worth knowing, not worth a limit
until a schema has fifty checks on one table.

**A batch's failures are still not counted by `slate_rows_written_total`.** A
refused operation writes nothing, so there is nothing to count; the successful
ones in an independent batch go through `autocommit` and are counted as always.
Mentioned because "batch" and "observability" have appeared in the same gap
list before and these are different things.
