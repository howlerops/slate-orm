# The three SDKs are compared on restoring a row, and on getting it wrong

- **Date:** 2026-09-20
- **Author:** Claude, closing the "what this does not do" of `the-helper-that-can-be-used-now`
- **Touches:** the three explorer adapters, `examples/explorer/conformance/conformance.py`
- **Kind:** feature

## What changed

Two conformance cases and the handlers behind them, in Go, TypeScript and
Python. `/api/restore` retires a row, reads it back with `include_deleted`,
restores it through the generated helper, and reports the row's state at three
points. `/api/restore-unchanged` writes the row back with the stamp still in it
and lets the refusal through, so the three clients are compared on the reason
token too.

99 cases became 101. All three agree on all of them.

## Why

The entry beside this one shipped `restored()` in three languages and admitted:

> **Go and TypeScript never restore against a real server.** … The
> three-SDK conformance runner has no case for it, so "all three agree about
> restoring" is untested and I am not claiming it.

It is claimed now, and measured. A generated helper is the exact shape this
runner exists for: one generator emits three implementations, nobody reads all
three, and "it compiles" is the only thing the unit tests could have been
checking if they did not run the encoder.

**The answer carries the row at three points, not one.** "It is live now" is
also what an adapter that quietly inserted a fresh row at the same key would
report — and that is not a hypothetical failure, it is the *likely* one, because
upsert-with-a-fresh-row is the obvious way to write this handler if you have not
read what `restored()` is for. `status_after` and `book_id_after` are what tell
the two apart, and the mutation below is exactly that adapter.

**The refusal case is separate because it is the path people actually take.**
Read the row, edit a field, write it back. The stamp comes along and the server
refuses. Comparing three clients on the happy path alone would leave the error
they will all actually see uncompared.

## Alternatives rejected

**One case that does both, catching the refusal inside the handler.** Rejected:
the runner compares refusals through the adapter's error wrapper, which is where
the reason token and the message live. A handler that caught its own refusal
would compare a string it chose rather than the one the client produced —
`bad_status` and `bad_batch` already establish the pattern of letting it
through.

**Reuse `purgeIDs` (8401–8403).** Rejected, and the reason is in the purge
case's own comment: it lists what survives at `id >= 8401`, so a row left there
would make that case's answer depend on which adapter ran first — the ordering
bug it records having been bitten by. 8301 is below the range and invisible to
it.

**Skip the cleanup and let the purge case's opening sweep collect the row.**
It would work, today, because that sweep is table-wide and this handler's row is
retired when it finishes. Rejected because it makes one case's correctness
depend on another case running afterwards, which is the coupling every handler
in this file goes out of its way to avoid.

**Show it in the demo UI instead.** A restore button is the natural demo and it
is not a *comparison*: one adapter serves the UI at a time. The runner is the
thing that catches two clients disagreeing, which is the risk a code generator
introduces.

## Evidence

**101 cases, three SDKs, no disagreements**, over a real head node with the Go,
TypeScript and Python adapters each answering every case:

```
101 cases: the three SDKs agree on all of them
```

**And the new case catches what it was written for.** The Go adapter's restore
replaced with a fresh row at the same key — the plausible wrong implementation,
not an arbitrary break:

```
a retired row can be restored (app): the adapters disagree
    go      {"book_id_after": [11], ..., "status_after": ["shipped"], "visible_after": [8301]}
    node    {"book_id_after": [10], ..., "status_after": ["pending"], "visible_after": [8301]}
    python  {"book_id_after": [10], ..., "status_after": ["pending"], "visible_after": [8301]}
```

`visible_after` and `retired_after` agree across all three under that mutation —
the row *is* live at that key either way. Only the columns the restore had to
carry through disagree, which is the whole reason they are in the answer.

**`hidden_while_retired` is `[]` in all three**, which is the other half: an
ordinary read does not see the row between the delete and the restore. A client
that leaked `include_deleted` into the plain read would report `[8301]`.

**The refusal case is registered in `EXPECTED_REFUSALS`**, so a server that
stopped refusing a caller-supplied `deleted_at` fails the run as a *stale list*
rather than passing quietly — the mechanism that list exists for.

**`scripts/check.sh`**: 20/20. `gofmt`, `go vet`, `go build`, `npx tsc
--noEmit`, `ruff` over both trees, and the adapter's own import check all clean.

## What this does not do

**No `MUST_DIFFER` pair.** `include_deleted` has one because a client that
dropped the flag would make all three agree about the smaller answer. The
restore cases do not need one — dropping the restore turns the happy case's
`retired_after` from `[false]` to `[true]`, and dropping the stamp from the
refusal case turns a refusal into an answer, which `EXPECTED_REFUSALS` catches.
That reasoning is the runner's own three-way test for whether a pair is needed,
applied; it is not a measurement, and I did not try deleting the calls in all
three clients to confirm it the way the `paged` note describes.

**It does not compare the *message* of the refusal in any detail.** The runner
diffs whole answers, so the message is compared because it is in the blob, not
because anything asserts its content. A server that changed the prose would show
as a disagreement only if one client cached the old one.

**The demo UI still has no restore.** A visitor cannot see this happen; only
the conformance runner can.

**The deployed harness does not exercise it.** `examples/deployed` runs the
stack against object storage and has its own case list, which this does not
touch. Restoring is an ordinary update, so there is no reason to expect the
storage backend to matter — an expectation, not a measurement.
