# @slate-orm/client — a TypeScript client for a slate head node

```ts
import { Client, uint, str, int, ge } from "@slate-orm/client";

const client = Client.connect("127.0.0.1:8080", {
  principal: "u64:1",
  tenant: "u64:1",
  roles: ["app"],
});

const session = client.session();
await session.insert("docs", [uint(1), str("note"), int(10)]);

for await (const row of session.query({
  table: "docs",
  filter: ge(2, int(5)),
  sort: [{ column: 2, direction: "desc" }],
  limit: 10,
})) {
  console.log(row);
}

client.close();
```

## The shape, and why

**64-bit integers are `bigint`, never `number`.** A `number` loses precision
above 2^53, and a primary key is exactly where that shows up latest and hurts
most. A test writes `2^53 + 1` as a key and reads it back; decoding through
`Number` instead of `BigInt` fails it.

**Values are a tagged union, not plain JavaScript values.** `7` as an `i64` and
`7` as a `u64` are *different values* to this server — its ordering is
type-first — and JavaScript cannot tell them apart. Guessing would write rows
that cannot be found again.

**`Identity` is not `Credentials`.** The head node does not authenticate; it
reads three headers a proxy is expected to have set. This fills them in and
proves nothing.

**Ordinals, not column names.** Positions are what the wire carries. Accepting
names would mean holding a second copy of the schema here, which can disagree
with the server's.

**A session keeps a watermark.** Reads through one never go backwards: a read
after a write sees that write. `client.session()` turns this on;
`sessionWithoutMonotonicReads()` is spelled out at length so turning it off is
deliberate.

**`get` returns `undefined` for a missing row**, not a throw — checking
existence should not mean catching an exception.

**`await tx.rollback()` after a commit is quiet**, which is what makes a
`finally` block the right shape for every transaction.

## Errors

Every failure is a `SlateError` with a `kind`. Use `isKind(error, "…")`.

`error.retryable` says whether trying again could work. Three worth knowing: a
`"not-leader"` is retryable **elsewhere** and carries `leader`; an
`"unknown-outcome"` is deliberately *not* retryable, because the write may have
landed and retrying is how one write becomes two; and `"deadline-exceeded"` has
the same ambiguity.

## What is not here

`Join`, `Aggregate` and `ExplainJoin` are on the wire and have no typed surface
here. `Explain` for a single table is. No vector similarity search, no
`SchemaCheck` plumbing, no computed values in a query.

The proto is loaded at runtime by `@grpc/proto-loader` rather than compiled
ahead of time, so there is no codegen step and no `protoc` needed to build
this package. The cost is that message shapes are `unknown` at the boundary and
this client narrows them by hand — which is why the value decoder refuses a
kind it does not recognise instead of returning something plausible.

The Python client in `clients/python` is the fuller one. Where two clients
disagree about the protocol, one of them is wrong; that is worth reporting
rather than working around.

## Tests

`npm test` compiles and runs `node --test` against a real `slate-serverd`
built from this repository with cargo — no mock, because a mock is a second
statement of what the server does written by whoever wrote the client, so it
agrees with the client's own misunderstandings.

Requires a working `cargo` on the path.
