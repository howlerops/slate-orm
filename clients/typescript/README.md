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

## Joins and aggregates

```ts
import { newJoin, at, count, agg, groupGt, uint } from "@slate-orm/client";

const b = newJoin();
const authors = b.add({ table: "authors" });
b.add({
  table: "books",
  type: "left",
  on: [{ earlier: at(authors, 0), own: 1 }],
});

const groups = await session
  .aggregateJoin(b.query(), {
    groupBy: [at(0, 0)],
    aggregates: [count()],
    having: groupGt(agg(0), uint(1)),
    sort: [{ column: agg(0), direction: "desc" }],
    limit: 10,
  })
  .collect();
```

A `Column` is *(which input, that input's own ordinal)* — never a cumulative
offset into a flattened row. `at(1, 2)` is the third column of the second
table, not "first table's width plus two". This is why the client needs no
catalog, and it is the thing about joins that is easiest to get wrong.

A grouping's `sort`, `limit` and `offset` are over **groups**, not the rows
going into them. A `having` and a group ordering name keys and aggregates —
`groupKey(0)`, `agg(0)` — and their comparisons are the `group*` family
(`groupGt`, …), separate from the row-level `gt` so the wrong one does not
typecheck.

A join stream yields one array **per input**, `undefined` where an outer join
found no match — kept separate rather than concatenated, because a flat row
cannot tell "no match" from "matched, and the columns are null".

`aggregateJoin` groups a join or a chain of any length. It took exactly two
inputs while the kernel grouped only a two-table join and the server refused a
third; the kernel groups a chain now and the refusal went with it. Nothing
counts inputs here, where the count could drift from the kernel's.

### Explaining a grouped read

```ts
const plan = await session.explainAggregateJoin(b.query(), grouping);
```

Not `explainJoin` on the same join. Grouping narrows each input's projection to
the group keys and the aggregates' columns — which is what lets an index answer
a `count(*)` without reading a row — so the two describe different plans.
`Explanation.decodes` is where the difference shows when the access path does
not change. Exactly one of `input` and `join` comes back, matching the request.

## Schema checks

Optional, and worth turning on. Declare a table and every request naming it
carries a fingerprint the server checks:

```ts
const client = Client.connect(addr, identity).declaring({
  books: {
    name: "books",
    columns: [
      { name: "id", type: "u64" },
      { name: "author_id", type: "u64" },
      { name: "title", type: "string" },
      { name: "year", type: "i64" },
    ],
    primaryKey: ["id"],
  },
});
```

This client resolves nothing from a declaration — the wire carries ordinals and
always did. What it buys is the check. Declaring `{id, title, year}` for a table
that is really `{id, year, title}` otherwise produces a client that reads titles
as years, silently and forever; declared, the first request is refused.

Every request naming the table carries it: writes, `get`, and reads alike —
`query`, `explain`, `join` and the aggregates put the claim on the query, so a
misdeclared table is refused before it can answer a transposed row.

Per-table and opt-in: a table with no declaration sends no claim and behaves
exactly as before.

## What is not here

No vector similarity search, and no computed values in a query or a join
input.

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
