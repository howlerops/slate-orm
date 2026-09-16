# Batch in all three clients, and a token that only survives when batched

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** all three clients, `examples/explorer/`, `CONTRACT.md`
- **Kind:** feature

## What changed

`Batch` in Python, Go and TypeScript, with the required `Atomicity` that the
RPC demands. Ten tests in Python, nine in Go, eight in TypeScript, and **three
conformance cases**, bringing the corpus to **80, with the three SDKs agreeing
on all of them**.

This closes the pattern the previous entry named: three features in a row —
`Related`, predicate writes, `Batch` — landed a commit ahead of their clients.
No server feature on this branch is now without one.

## Why

Each client takes the shape it already had: a mutable builder in Python, where
`Query` is a builder; a `*Batch` with appending methods in Go, where a builder
suits an ordered list even though `Query` is a struct; a plain `Batch`
interface in TypeScript, where `Query` is one. `atomicity` is a constructor
parameter or a required field in all three, so the choice cannot be forgotten
on the way to the wire — the client-side half of the server refusing an
unspecified one.

The conformance cases are one request under each atomicity, with the *same*
three operations, one of which collides. That collision is the comparison:
independent reports it and keeps going (`left: 3`), atomic fails the call and
undoes the rest (`left: 1`). Three clients agreeing on both answers is what
says the distinction survived three separate implementations of it.

## A finding: the reason token only reaches a client when it is batched

A lone RPC carries its stable reason token — `DUPLICATE_PRIMARY_KEY` — in
`grpc-status-details-bin`, a protobuf blob. **None of the three clients decodes
it.** All three deliberately drop binary trailers, each with a comment saying
so, so `reason` has never been visible to a caller of any of them.

A batch has to put the token in the message body, because an independent
batch's failures come back inside a *successful* response where there are no
trailers. So the batched path now exposes something the lone path does not, and
all three clients grew a `reason` field that is populated for exactly one kind
of failure.

That asymmetry is recorded on the field in each client rather than smoothed
over. The fix is to decode the details blob on the lone path, in three
languages; nobody has needed it enough, and pretending the field is general
would be worse than saying where it comes from.

## Alternatives rejected

**Raise on the first failed operation in an independent batch.** It would make
the Python and TypeScript APIs feel like the rest of the client, where failure
is an exception. It also destroys the feature: the point of independence is
that the operations beside the failure landed, and a caller who gets an
exception has no handle on the ones that succeeded.

**Return raw `{code, message}` per failure rather than building an exception.**
Simpler, and one less conversion. It makes a caller write two error paths — one
`except SlateError` for lone writes and one field check for batched ones — for
failures that are the same failure. The three `from_batch_error` helpers exist
so `except NotFound` works either way.

**A single `Atomicity` default of independent, with atomic opt-in.** Every
client would be shorter. It reintroduces exactly what the RPC refuses, one
layer up: a caller who never chose would silently get independence, which is
the failure mode the whole design is about.

**Put `rowToWire` in a third module rather than moving it.** It lived in
`client.ts` and `query.ts` needed it for a batch's rows, and `client.ts`
imports `query.ts` — so a copy would be a circular import or a second
implementation. Moved to `value.ts`, which is where value encoding already
lives.

## Evidence

Python 212 passed, Go passed, TypeScript 108 passed, **80 conformance cases
with the three SDKs agreeing**.

**The corpus caught a fourth adapter bug, again mine, again only visible across
three implementations.** The Go adapter serialised an error kind with
`string(e.Kind)`, which on an int-backed enum yields the *rune at that code
point* — so the three answers were identical in every field except one, where
Go sent a single unprintable control character and Node and Python both sent
`already-exists`. The fix was to use `kindName`, the helper that exists in all
three adapters precisely to spell kinds the same way, and which I had not used.
Go's own `Kind.String()` would also have been wrong here: it returns
`already exists` with a space, and the contract spelling is hyphenated.

Worth stating plainly: the semantics were right on the first run in all three —
`left: 3` against `left: 1`, outcomes in the same order with the same reason
token. Only the *spelling* was wrong, in one client, and only a three-way
comparison shows that.

A composite-key mistake in my own Python test was caught by the client rather
than the server: `users` is keyed on `(tenant_id, id)` and I passed one value.
The client checks key arity before sending, so the error named the key instead
of being a confusing refusal from the server.

## What this does not do

No measurement from a client. The 9.4x–10.0x in the previous entry was measured
through the Rust wire test; none of the three clients has a benchmark, so the
claim that batching helps *them* is inherited rather than shown. The clients
add per-request work the Rust test does not (schema claims, value encoding),
and that work is not saved by batching — it is per operation either way.

`max_batch_operations` is enforced only by the server. No client checks the
length before sending, so a caller who builds a batch of 5,000 pays the round
trip to be refused. Cheap to add and not added: the limit is configurable, so a
client-side copy would be a second number to keep in agreement with a server it
cannot see.

The demo frontend still has no batch button, like predicate writes before it.
The adapters serve `/api/batch` and the corpus compares it; a visitor cannot
see it.
