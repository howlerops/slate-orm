# slate — a Go client for a slate head node

```go
client, err := slate.Dial("127.0.0.1:8080", slate.Identity{
    Principal: "u64:1",
    Tenant:    "u64:1",
    Roles:     []string{"app"},
})
defer client.Close()

session := client.Session()
_, err = session.Insert(ctx, "docs",
    []slate.Value{slate.Uint(1), slate.String("note"), slate.Int(10)})

stream, err := session.Query(ctx, slate.Query{
    Table:  "docs",
    Filter: slate.Filter(slate.Ge(2, slate.Int(5))),
    Sort:   []slate.SortKey{{Column: 2, Direction: slate.Desc}},
    Limit:  slate.Limit(10),
})
rows, err := stream.Collect()
```

## The shape, and why

**`Identity` is not `Credentials`.** The head node does not authenticate; it
reads three headers a proxy is expected to have set. This type fills them in
and proves nothing, and it is named so that reading it does not suggest
otherwise.

**Values are a closed set, not `any`.** `Uint(7)` and `Int(7)` are *different
values* to this server — its ordering is type-first — so a client that guessed
from a Go `int` would produce rows that cannot be found again. The compiler
asks instead.

**Ordinals, not column names.** Positions are what the wire carries. Accepting
names would mean holding a second copy of the schema here, which can disagree
with the server's.

**A `Session` keeps a watermark.** Reads through one never go backwards: a read
after a write sees that write. Without it a replica read can be served by a
node that has not caught up — correct, and surprising. `Client.Session()` turns
this on; `SessionWithoutMonotonicReads` is spelled out at length so that
turning it off is deliberate.

**`Get` returns `(row, found, err)`.** A missing row is not an error, because
checking existence should not mean matching on an error type.

**`defer tx.Rollback(ctx)` is the shape.** Rollback after commit is a no-op.

## Errors

Every failure is a `*slate.Error` with a `Kind`. Use `slate.IsKind(err, …)`,
or `errors.As` for the detail.

`Retryable()` says whether trying again could work. Three points worth knowing:
a `KindNotLeader` is retryable **elsewhere** and carries `Leader`; a
`KindUnknownOutcome` is deliberately *not* retryable, because the write may
have landed and retrying is how one write becomes two; and a
`KindDeadlineExceeded` has the same ambiguity.

## Transactions, with a retry

`Begin`/`Commit`/`Rollback` are there for a body that must not be retried.
For everything else, `Transact` runs the body in a transaction and retries a
conflict:

```go
written, err := slate.Transact(ctx, session, slate.DefaultRetry,
    func(ctx context.Context, tx *slate.Transaction) (uint64, error) {
        if _, err := tx.Insert(ctx, "books", row); err != nil {
            return 0, err
        }
        return 1, nil
    })
```

A **function** rather than a method on `*Session`, because Go has no generic
methods and a method would have to return `any` — which is the version every
caller then wraps to get their value back.

It retries `KindConflict` and nothing else. A unique violation, an access
denial or a fenced writer fails identically forever, and retrying those turns a
clear error into a hang. `KindUnavailable` and `KindNotLeader` are *not*
retried here even though `Retryable()` reports them as retryable: both are
retryable against a **different node**, and this only has the one it was given.

## Joins and aggregates

```go
b := slate.NewJoin()
authors := b.Add(slate.JoinInput{Table: "authors"})
b.Add(slate.JoinInput{
    Table: "books",
    Type:  slate.Left,
    On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
})

groups, err := session.AggregateJoin(ctx, b.Query(), slate.Grouping{
    GroupBy:    []slate.Column{slate.At(0, 0)},
    Aggregates: []slate.Aggregate{slate.Count()},
    Sort:       []slate.GroupSortKey{{Column: slate.Agg(0), Direction: slate.Desc}},
    Limit:      slate.Limit(10),
})
```

A `Column` is *(which input, that input's own ordinal)* — never a cumulative
offset into a flattened row. `At(1, 2)` is the third column of the second
table, not "first table's width plus two". This is why the client needs no
catalog, and it is the thing about joins that is easiest to get wrong.

`Grouping`'s `Sort`, `Limit` and `Offset` are over **groups**, not the rows
going into them. A `HAVING` and a group ordering name keys and aggregates —
`slate.Key(0)`, `slate.Agg(0)` — and the comparisons for them are the
`Group*` family (`GroupGt`, …), separate from the row-level `Gt` so that using
the wrong one does not compile.

A `JoinStream` yields one slice **per input**, `nil` where an outer join found
no match — kept separate rather than concatenated, because a flat row cannot
tell "no match" from "matched, and the columns are null".

`AggregateJoin` groups a join or a chain of any length. It took exactly two
inputs while the kernel grouped only a two-table join and the server refused a
third; the kernel groups a chain now and the refusal went with it. Nothing
counts inputs here, where the count could drift from the kernel's.

### Explaining a grouped read

```go
plan, err := session.ExplainAggregateJoin(ctx, b.Query(), grouping)
```

Not `ExplainJoin` on the same join. Grouping narrows each input's projection to
the group keys and the aggregates' columns — which is what lets an index answer
a `COUNT(*)` without reading a row — so the two describe different plans.
`Explanation.Decodes` is where the difference shows when the access path does
not change. Exactly one of `Input` and `Join` comes back, matching the request.

## Deadlines

The context, and nothing else:

```go
ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
defer cancel()
rows, err := session.Query(ctx, slate.Query{Table: "trips"})
```

Every method takes a `context.Context` and grpc-go honours its deadline, so
this client needed nothing added where the Python and TypeScript ones each grew
a `with_timeout`. That was a claim until `deadline_test.go` was written: it ends
a call against a listener that accepts and never speaks, and checks the error is
`KindDeadlineExceeded` rather than something else — beside a test showing the
same call not returning when the context has no deadline.

## Schema checks

Optional, and worth turning on. Declare a table and every request naming it
carries a fingerprint the server checks:

```go
client := must(slate.Dial(addr, identity)).Declaring(slate.Schemas{
    "books": {
        Name: "books",
        Columns: []slate.ColumnDef{
            {Name: "id", Type: slate.TypeUint},
            {Name: "author_id", Type: slate.TypeUint},
            {Name: "title", Type: slate.TypeString},
            {Name: "year", Type: slate.TypeInt},
        },
        PrimaryKey: []string{"id"},
    },
})
```

This client resolves nothing from a declaration — the wire carries ordinals and
always did. What it buys is the check. Declaring `{id, title, year}` for a table
that is really `{id, year, title}` otherwise produces a client that reads titles
as years, silently and forever; declared, the first request is refused.

Every request naming the table carries it: writes, `Get`, and reads alike —
`Query`, `Explain`, `Join` and the aggregates put the claim on the query, so a
misdeclared table is refused before it can answer a transposed row.

Per-table and opt-in: a table with no declaration sends no claim and behaves
exactly as before.

## What is not here

No vector similarity search surface, and no computed values in a `Query` or a
join input.

The Python client in `clients/python` is the fuller one; where the two
disagree about the protocol, that is a bug in one of them rather than a
dialect, and worth reporting as one.

## Tests

`go test ./slate/` builds `slate-serverd` from this repository with cargo and
runs each test against a real node on an ephemeral port. There is no mock: a
mock is a second statement of what the server does, written by whoever wrote
the client, so it agrees with the client's own misunderstandings.

Requires a working `cargo` on the path.
