# The explorer's HTTP contract

Three adapters implement this, one per SDK, against the same head node. The
frontend picks which one serves a request with a flag.

**The contract is the point.** Three clients written from one protocol should
answer identically, and until now nothing checked that: each client's suite runs
against the same server, which catches a client that is wrong and not three that
are wrong the same way. `conformance/` sends every request below to all three
adapters and requires byte-identical JSON, which is the first test in this
repository that compares the clients to each other.

## Rules that make the comparison meaningful

- **Every response is deterministic.** No timestamps, no durations, no
  hostnames, no iteration-order-dependent arrays. Rows come back in an order the
  query pins. Where a server-side order is not defined, the adapter sorts.
- **64-bit integers are JSON strings.** `{"u64":"9007199254740993"}`, never a
  JSON number: `JSON.parse` would silently round it, and a primary key is where
  that hurts most. The TypeScript client already refuses `number` for this
  reason; the wire format follows.
- **Values are tagged.** `{"u64":"1"}`, `{"str":"ada"}`, `{"null":true}`. An
  `i64` and a `u64` of the same magnitude are different values to this database,
  so a bare `1` would be ambiguous and the three adapters would be free to
  disagree about what they sent.
- **Errors are `{"error":{"kind":"...","message":"..."}}`** with the kind drawn
  from each client's own taxonomy. The kinds must match across adapters; the
  messages come from the server and match for free.

## Endpoints

### `GET /api/meta`

```json
{"sdk": "go", "leader": true, "tables": ["authors", "books", "sales"]}
```

`sdk` is the only field that legitimately differs between adapters, and the
conformance runner excludes it.

### `POST /api/query`

```json
{
  "table": "books",
  "filter": {"op": "ge", "column": 3, "value": {"i64": "1970"}},
  "sort": [{"column": 3, "direction": "desc"}],
  "limit": 20,
  "offset": 0,
  "columns": [0, 1, 2, 3]
}
```

→ `{"rows": [[{"u64":"10"}, ...], ...], "servedBy": "writer"}`

`filter` is `null`, or one of:

- `{"op": "eq"|"ne"|"lt"|"le"|"gt"|"ge", "column": n, "value": tagged}`
- `{"op": "like"|"ilike", "column": n, "pattern": "a%"}`
- `{"op": "isNull"|"isNotNull", "column": n}`
- `{"op": "in", "column": n, "values": [tagged, ...]}`
- `{"op": "and"|"or", "parts": [filter, ...]}`
- `{"op": "not", "part": filter}`

### `POST /api/join`

```json
{"type": "inner"|"left"|"right"|"full", "limit": 50}
```

Authors ⋈ books on `authors.id = books.author_id`. Fixed shape: the demo is
about the join *type*, and a general join builder over HTTP would be a second
query language to keep three implementations of.

→ `{"rows": [{"authors": [tagged...] | null, "books": [tagged...] | null}, ...]}`

A `null` side is an outer join's unmatched row, kept distinct from a row of
nulls.

### `POST /api/aggregate`

```json
{"groupBy": "author"|"country"|"decade", "having": null | {"minCount": 2},
 "sort": "count"|"key", "direction": "asc"|"desc", "limit": 20}
```

A grouped join, which is what the chart draws.

`decade` is not a column. It is `books.year / 10 * 10`, declared on the join as
a computed value and named as the join's rather than as an input's — the
distinction is the point, since an input's computed value has no slot in a
joined row at all. All three adapters used to refuse it with "needs a computed
column, which this demo does not declare", which was true of the clients and
never of the database; it is the case that now makes the conformance runner
compare the three SDKs on a computed value, without the contract growing a
general expression language to keep three implementations of.

→ `{"groups": [{"key": [tagged...], "count": {"u64":"3"}}, ...]}`

### `POST /api/explain`

Same body as `/api/query`.

→ `{"table": "...", "access": "...", "indexOnly": false, "sorts": true,
    "estimatedRows": 4.0, "residual": "...", "display": "..."}`

`estimatedCost` is **excluded**: it is a float the three clients may format
differently, and the conformance runner compares text.

### `POST /api/explain-aggregate`

Same body as `/api/aggregate`.

→ `{"inputs": [{"table": "...", "access": "...", "indexOnly": false,
    "decodes": [0, 1], "algorithm": "hash"}, ...], "display": "Group by [...]"}`

The plan of the **grouped** read, which is not the plan of the join underneath
it. Grouping narrows each input's projection to the group keys and the
aggregates' columns — which is what lets an index answer a `COUNT(*)` without
reading a row — so `/api/explain` on the same join describes something else.

`decodes` is where the difference shows. Wherever no index applies, narrowing
changes what a plan decodes and nothing about how it reaches rows, so the two
plans have the same `access` and the same `display` shape.

`estimatedRows` and `estimatedCost` are **excluded**, as on `/api/explain`, for
the same reason: floats the three clients may format differently.

### `POST /api/transaction`

```json
{"commit": true}
```

Inserts a book inside a transaction, reads it back inside, then commits or rolls
back, and reports whether it is visible afterwards. Demonstrates the one thing a
single request cannot.

→ `{"visibleInside": true, "visibleAfter": true}`

## Identity

Every request may carry `X-Demo-Identity: app | reader | stranger`. The adapter
maps it to the head node's three identity headers. This is how the demo shows
RBAC and row-level security live: the same query, three answers, none of them
the adapter's doing.
