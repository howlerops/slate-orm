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

## What is not here

`Join`, `Aggregate` and `ExplainJoin` are on the wire and not on this client
yet — the generated stubs are in `internal/pb`, so they are reachable, but
there is no typed surface and no test. `Explain` for a single table is here.

No `Vector` similarity search surface, no schema-check plumbing
(`SchemaCheck`), no computed values in a `Query`.

The Python client in `clients/python` is the fuller one; where the two
disagree about the protocol, that is a bug in one of them rather than a
dialect, and worth reporting as one.

## Tests

`go test ./slate/` builds `slate-serverd` from this repository with cargo and
runs each test against a real node on an ephemeral port. There is no mock: a
mock is a second statement of what the server does, written by whoever wrote
the client, so it agrees with the client's own misunderstandings.

Requires a working `cargo` on the path.
