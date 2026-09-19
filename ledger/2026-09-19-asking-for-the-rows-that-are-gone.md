# Asking for the rows that are gone

## What changed

All three clients can now ask for soft-deleted rows: `include_deleted()` in
Python, `IncludeDeleted` in Go, `includeDeleted` in TypeScript. Three
conformance cases exercise it against a running head node.

Two things found on the way: the demo listed its tables in four places, and the
conformance runner could not see a field that every client drops.

## Why

The wire field shipped with nothing able to send it. That entry said so —
"their stubs carry it, but none has a builder method, so a caller would have to
construct the protobuf by hand" — and the only evidence it worked was a
proptest over the conversion functions, which tests that Rust can serialise a
bool.

## The gap in the runner, which is the finding

The conformance runner's whole design is *three clients must agree*. That
cannot see a request field every client drops: all three then send the same
smaller request, get the same smaller answer, and agree perfectly.

`include_deleted` is exactly that shape. "The read returned four rows" proves
nothing on its own, because a server ignoring the flag returns four too if four
is what the plain read returns. The evidence has to be that *asking changes the
answer*, which needs two cases and a comparison between them — and the runner
had no way to express one.

So it has `MUST_DIFFER`: named pairs whose agreed answers must not be equal. It
also fails if either name is missing, because a renamed case would otherwise
switch the check off silently.

The mutation that justifies it: drop the flag in **all three clients at once**.
Every single-client mutation is caught by disagreement — the other two still
send it — but the unanimous one produces 95 agreeing cases and a clean bill of
health. With `MUST_DIFFER` it reports exactly what happened:

> `'a read that cannot see a retired row'` and `'a read that asks for retired
> rows too'` returned the same answer, so whatever separates them was dropped
> by all three clients or ignored by the server

## The four table lists

Adding `shipments` made `/api/meta` disagree between adapters, and the reason
was that each adapter named its tables twice: an allowlist for `/api/query` and
a literal in the meta handler. Go and Python had drifted the moment a table was
added — the query path served four tables and the meta handler described three.
The node adapter had been deriving both from one constant all along, which is
why it was the one that disagreed.

Both now derive from the list their query path enforces. Go's needs sorting,
because Go randomises map iteration and an unsorted list would disagree with
*itself* between two runs — a far more confusing failure than a missing table.

## Alternatives rejected

**A builder method on `Query` only, not `_QueryBase`.** Simpler, and wrong: the
wire carries the flag per `Query`, and a join has one per input. Reading live
parents against every child including retired ones is a real request, and the
server already accepts it.

**Have the clients refuse without the grant.** They cannot know what was
granted, and a client that guesses is a client that refuses something the
server would allow. The refusal belongs at the server and arrives identically
in all three SDKs, which the third case asserts.

**Add `editions` to the query allowlists while fixing them.** It is reachable
only through a relationship, and adding it would make all three serve a request
that nothing asks for. The asymmetry is deliberate and now stated where the
lists are defined.

**Assert the row counts in the conformance cases.** The runner compares
adapters to each other and holds no expected values; adding one kind of
expectation for one case would be a second mechanism. `MUST_DIFFER` is a
general statement about a *pair*, which is what the evidence actually needs.

## Evidence

The refusal, identical from all three SDKs:

```
access denied: no role grants read_deleted on table `shipments`
```

Five mutations, all caught:

| mutation | caught by |
| --- | --- |
| Python drops the flag | disagreement |
| Go drops the flag | disagreement |
| TypeScript drops the flag | disagreement |
| TypeScript sends `include_deleted` instead of `includeDeleted` | disagreement |
| **all three drop it at once** | **`MUST_DIFFER`** |

The fourth is worth its place: proto3 JSON is lowerCamelCase and the snake_case
spelling is the natural mistake, silently ignored by the server.

`./run.sh --conformance`: 95 cases, the three SDKs agree on all of them.
`ruff check`, `ty check` against a clean virtualenv, `gofmt`, `go vet`, and
`tsc --noEmit` on both the client and the adapter: clean.

## What this does not do

No client can *un*-delete a row or purge one; `include_deleted` is a read.

The demo UI has no control for it — the identity switcher shows the refusal
only if someone edits a request by hand. The conformance corpus is where the
behaviour is demonstrated, not the browser.

`MUST_DIFFER` holds one pair. Several other flags have the same weakness —
`paged`, and any hint — and nothing checks them; adding those pairs is cheap
and is not done here.

Nothing tests `include_deleted` on a *join* input, which is the case
`_QueryBase` was chosen to support. The Go and TypeScript join builders take a
different input type, and whether the flag reaches the wire from there is
untested.
