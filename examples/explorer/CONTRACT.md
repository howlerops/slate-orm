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
{"groupBy": "<name>", "having": null | {"minCount": 2},
 "sort": "count"|"key", "direction": "asc"|"desc", "limit": 20}
```

A grouped join, which is what the chart draws.

Two of the groupings are columns (`author`, `country`). The rest are not
columns at all: each is an expression declared on the *join* and named as the
join's computed value rather than as an input's — the distinction is the point,
since an input's computed value has no slot in a joined row at all.

| `groupBy` | the expression | what it compares |
| --- | --- | --- |
| `author` | `authors.name` | a column |
| `country` | `authors.country` | a column |
| `decade` | `books.year / 10 * 10` | arithmetic |
| `shout` | `upper(authors.name)` | a string function |
| `era` | `CASE WHEN books.year < 1970 …` | a conditional |
| `tidy` | `regexp_replace(lower(books.title), '[^a-z]+', '-')` | a regular expression |
| `releasedYear` | `year(books.released)` | a calendar field |
| `releasedMonth` | `month_start(books.released)` | a calendar truncation |
| `releasedHourNY` | `hour(books.released` in `America/New_York)` | a named timezone |
| `label` | `authors.country ‖ '/' ‖ books.title ‖ '/' ‖ books.year` | concatenation, across both inputs and over an integer |

Only `decade` existed for a while, and that was weaker evidence than it looked:
integer division is the one operation every language spells identically, so
three clients agreeing about it says little about the ones where they do not.
Seven of the eleven books were released before 1970, so the calendar keys run
on *negative* epoch seconds; some are in daylight saving and some are not, so
`releasedHourNY` is not a constant shift.

An unknown name is refused, which is a case of its own below.

→ `{"groups": [{"key": [tagged...], "count": {"u64":"3"}}, ...]}`

### `POST /api/nearest`

```json
{"limit": 5}
```

Books ranked by cosine distance from a **fixed** query vector,
`[0.1, 0.2, 0.3, 0.4]`, against `books.embedding`.

Fixed rather than taken from the body, because the claim is that three SDKs
build the same `Distance` scalar and agree on the order it produces; a vector
from the body would let a caller ask a question the other two adapters were not
asked.

→ `{"titles": [{"str": "Solaris"}, ...]}`

The titles **in order**, and not the distances: a distance is an f64 and the
three clients format floats differently, which is why `/api/explain` excludes
`estimatedCost` for the same reason. The order is total — the sort breaks ties
on the primary key — so comparing it is comparing the whole answer.

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
