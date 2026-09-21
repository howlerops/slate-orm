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
  `{"decimal":"1250"}` is the same rule a third time and the sharpest case: a
  decimal is a count of the column's smallest unit, so `1250` is 12.50 at scale
  2 and 1250 at scale 0, and the scale is never on the wire. The tag carries
  the units; rendering is compared separately, at
  `/api/conditional-update`.
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

### `POST /api/chain`

```json
{"type": "inner", "limit": 10}
```

Three tables in one read: `authors`, their `books`, and those books' `sales`.
Same body as `/api/join`.

→ `{"rows": [{"authors": row|null, "books": row|null, "sales": row|null}, …]}`,
sorted, for the reason `/api/join` gives.

A chain is **not** a separate RPC: `JoinQuery` carries `repeated JoinInput` and
the kernel takes its chain path past two of them. What differs between the
three SDKs, and so what this compares, is how each spells the third input's
attachment — it joins back to the *second* input, and a client that attached it
to the first would produce a cross join with exactly the right number of
columns.

### `POST /api/window`

```json
{"function": "rowNumber"|"rank"|"denseRank"|"lag"|"lead"|"sum"|"count",
 "partition": true, "running": false, "limit": 20}
```

Over `books`, filtered to `author_id <= 6`, sorted by `id` ascending and
limited. Fixed shape, like `/api/join`: a general window builder over HTTP
would be a second query language to keep three implementations of.

- `partition` true partitions by `author_id`; false is one partition over the
  whole result, which is what SQL means by omitting the clause.
- The window's own `ORDER BY` is `year` ascending. It is always present for
  the ranking functions and for `lag`/`lead` — the server refuses those
  without one — and present for `sum`/`count` only when `running` is true.
  That is not a spelling: it is the standard's default frame changing from the
  whole partition to a running value through the current row's peers.
- `lag` and `lead` read `year`, one row away. `sum` sums `year`; `count` is
  `COUNT(*)`.

→ `{"rows": [{"row": [tagged, ...], "windowed": [tagged, ...]}, ...]}`

The two lists are separate in the answer because they are separate on the
wire. A window value is not a column and not a computed value, and an adapter
folding it into `row` would return something a caller reads as a different
thing — which is the failure the three lists exist to prevent and the one that
looks like working software until somebody adds a column.

The filter keeps out book 19, whose `author_id` is 99 so the outer joins have
an unmatched side: it would be a partition of one in every answer here. And
note what the demo's data does *not* have — two books by one author in the
same year — so `rank` and `denseRank` agree on every row of it. The tie case
is covered in each client's own suite and in the kernel's; this compares the
three SDKs to each other.

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
| `discounted` | `books.price - 0.50` | a decimal literal, which takes the column's scale |
| `doubled` | `books.price * 2` | money times a whole number, which is still money |
| `badPrice` | `books.price + books.year` | **refused** by the server: money plus a count of nothing |

The three decimal rows are the newest and are there for a reason the others
are not: a decimal literal has **no scale of its own** and takes the column's,
so `0.50` beside a scale-2 price has to be sent as `Decimal(50)` — the count of
cents — and not as the integer 50 that each of the three languages reaches for
first. An adapter that sent the integer is refused by the server rather than
answering differently, which is a failure a three-way *answer* comparison
cannot see. `badPrice` is that refusal made deliberate: all three must surface
the same one.

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

### `POST /api/related`

```json
{"way": "children", "keys": [{"u64": "10"}, {"u64": "11"}]}
```

Loads one relationship for many parents, in a single read. The relationship is
`sales.book_id -> books`, the demo's only foreign key — `books.author_id`
cannot be one, because `Author Unknown` names author 99 on purpose so that a
right or full join has an unmatched side to show.

`way` is `children` (a book's sales) or `parents` (a sale's book). `through`
names the foreign key and defaults to `sale_book`; it is settable only so the
corpus can name a key that does not exist and compare the three refusals.

→ `{"groups": [[row, …], …]}`

One group per key **the caller sent**, in the caller's order, including the
empty ones. That is not the shape the server replies in: it sends one group per
*distinct* key that matched something, and says nothing about the order the
caller asked in. Each client puts the answer back into the caller's order
itself, so this is where the three can disagree without the server noticing —
which is why a repeated key, an empty group and a key for a row that does not
exist are all cases here.

Reading it `parents` goes through `books`, which carries the row policy. As
`reader` the group for book 20 comes back empty, exactly as a direct read would
lose it. A client that resolved the relationship from rows it already held
would not lose it, which is the argument for the call existing at all.

### `POST /api/page`

```json
{"limit": 4, "after": [{"u64": "13"}]}
```

One page of `books` by keyset. `after` is the cursor a previous page returned,
absent for the first. `columns` and `sort` are there so the corpus can reach the
refusals — see below.

→ `{"rows": [row, …], "cursor": [value, …] | null, "isLast": bool}`

`cursor` is `null` when the page was short and there is provably nothing after
it. A page that comes back *full* gets a cursor even when it is the last one:
reading one row further to find out would be paid on every page to save one
empty request at the end of a sequence most callers never finish.

The cursor comes back as a row of tagged values rather than as bare numbers, so
the corpus compares its *type* as well as its value — a client that had lost
the `u64`/`i64` distinction would look right on its own.

Three requests here are expected to be refused, and the refusal is the answer
being compared: a page with no limit (a page with no size is the whole table),
a projection that drops a primary-key column (the cursor is that key, so there
would be nothing to build one from), and a sort into an order the key does not
give (the page boundary would not be where the cursor says).

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

### `POST /api/predicate-write`

```json
{ "kind": "delete" | "update", "returning": false, "noSet": false }
```

Seeds four `books` with ids 9100–9103 and years 2000–2003, then writes over the
two with `year >= 2002` by predicate. `kind` picks which write; `returning`
asks for the rows back; `noSet` sends an update with no assignments, which the
server refuses.

```json
{ "affected": 2, "rows": [ [ … ], [ … ] ], "left": 2 }
```

`rows` is empty unless `returning` asked — for a delete they are the rows as
they were before removal, for an update as written. `left` is how many of the
four seeded rows are still there, and it is in the answer on purpose:
`affected` is the server's report of what it did, and `left` is what the table
says afterwards. A delete that reported two and removed none would agree across
three clients on a number it had made up.

The handler seeds and cleans its own id range rather than touching the fixture.
The conformance runner drives all three adapters against one database, so a
case that deleted a fixture row would make every later case depend on which SDK
happened to run first — and would not be idempotent, which the demo needs.

### `POST /api/conditional-update`

```json
{ "stale": false }
```

Seeds one `books` row at 9300 priced 10.00, reads it back, and — when `stale`
is set — lets somebody else move the price to 11.00 first. Then tries a
conditional update to 12.50 guarded by the row as it was read.

```json
{ "refused": "", "price": {"decimal":"1250"}, "rendered": "12.50" }
```

With `stale: true` the server refuses it: `refused` is `"conflict"` and the
price is 11.00, the other writer's.

One endpoint rather than two because the *pair* is the point. An unconditional
update and a conditional one over an unchanged row do exactly the same thing,
so an adapter that dropped the `expected` rows would pass the happy case and
only the stale one tells them apart. `refused` is a field rather than an
adapter error for the same reason `failed` is in the batch endpoint.

`rendered` is the only place the three clients' *decimal renderers* are
compared. A `{"decimal": ...}` tag carries the units and says nothing about a
scale — the scale is the column's and never travels — so each adapter renders
against the 2 it declares locally, and three renderers that disagree about
`-0.75` or about where the point goes show up here.

### `POST /api/conditional-delete`

```json
{ "stale": false, "gone": false }
```

Seeds one `books` row at 9301, reads it back, and then — depending on the two
flags — lets somebody else edit it (`stale`) or remove it (`gone`) before
trying a conditional delete guarded by the row as it was read.

```json
{ "refused": "", "affected": 1, "left": false }
```

Three answers, and the third is why this endpoint is not the update's twin:

- unflagged, the delete lands: `refused` empty, `affected` 1, `left` false;
- `stale`, it is refused as a conflict and the row is still there;
- `gone`, it is refused as **not-found** — where a *plain* delete of an absent
  key reports `affected: 0` and no error. A caller that said what it expected
  to find wants to hear that somebody got there first, and `not-found` rather
  than `conflict` because a row that moved can be re-read and the decision
  remade, while a row that is gone cannot, so retrying a conflict would loop.

`left` is what the table says afterwards, beside `affected` and `refused`,
which are what the server said it did — for the reason the predicate-write
endpoint gives: a refusal that removed the row anyway would agree across three
clients on a claim none of them checked.

### `POST /api/batch`

```json
{ "atomicity": "independent" | "all-or-nothing" }
```

Seeds one `books` row at 9201, then sends three inserts — 9200, **9201 again**,
9202 — as one batch under the asked-for atomicity. The collision in the middle
is the whole point of the endpoint: it is the operation that makes the two
guarantees visibly different.

```json
{ "failed": "", "outcomes": [ {"ok": 1}, {"kind": "already-exists", "reason": "DUPLICATE_PRIMARY_KEY"}, {"ok": 1} ], "left": 3 }
```

Under `independent` the failure is an *outcome* and the operations beside it
land, so `left` is 3. Under `all-or-nothing` the call fails, `failed` carries
the error kind, `outcomes` is empty and `left` is 1 — only the seeded row.

`failed` is a field rather than an adapter error on purpose: it keeps both
atomicities as ordinary cases the corpus compares, instead of one being a
refusal case and one not. `reason` is the server's stable token, which reaches
the client through the message body rather than through trailers — a batched
failure is data, not an exception.
