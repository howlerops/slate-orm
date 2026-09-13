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

`AggregateJoin` takes exactly two inputs. A third is refused by the server with
that as the reason, rather than counted here where the count could drift from
the kernel's.

## What is not here

No vector similarity search surface, no schema-check plumbing (`SchemaCheck`),
no computed values in a `Query` or a join input.

The Python client in `clients/python` is the fuller one; where the two
disagree about the protocol, that is a bug in one of them rather than a
dialect, and worth reporting as one.

## Tests

`go test ./slate/` builds `slate-serverd` from this repository with cargo and
runs each test against a real node on an ephemeral port. There is no mock: a
mock is a second statement of what the server does, written by whoever wrote
the client, so it agrees with the client's own misunderstandings.

Requires a working `cargo` on the path.
