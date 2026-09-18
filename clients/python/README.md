# slate-client

A typed Python client for the `slate-orm` gRPC head node.

```python
from slate import Client, Column, Identity, Query, Table, ValueType, desc, i64, u64

docs = Table(
    "docs",
    [
        Column("id", ValueType.U64),
        Column("kind", ValueType.STR),
        Column("size", ValueType.I64),
        Column("note", ValueType.STR),
    ],
    primary_key=["id"],
)

with Client("127.0.0.1:50051", Identity("u64:1", roles=["app"])) as client:
    client.insert(docs, [(u64(1), "kind-a", i64(5), "first")])

    # Reads the write back — from a replica, without naming a token. The
    # session threaded the sequence the commit returned.
    q = Query(docs)
    for row in client.query(q.where(q.c.size >= 5).sort(desc(q.c.size)).limit(10)):
        print(row.get("kind"), row.get("size"))
```

The most useful thing in this directory is **[`PROTOCOL-FINDINGS.md`](PROTOCOL-FINDINGS.md)**:
fifteen places where the wire is awkward, underspecified or wrong, written from
the position of the first consumer of it outside Rust. The client is the
evidence for that document as much as it is a deliverable.

---

## The four decisions

### A column reference always comes from a builder, never from a table

`ColumnRef` names a producer and an index inside it, and the server does the
ordinal arithmetic — so that a client never adds a table width to anything, and
a column added to an early table cannot silently re-point a predicate over a
later one. A Python SDK can throw that away in one line, by letting a `Table`
produce references: they would have to carry `input=0`, which is right for the
first input of a join and silently wrong for every other.

So a `Table` produces no references at all. Every reference comes from a
`Columns` accessor that was handed its position by the builder that assigned
it:

```python
j = JoinQuery()
authors = j.add(AUTHORS)                                   # position 0
books = j.add(BOOKS, on=[(authors.c.id, "author_id")])     # position 1
books.having(books.c.year > authors.c.born)
```

`authors.c.id` is input 0's column 1; `books.c.year` is input 1's column 4.
There is no `input=` argument anywhere in the API, and no way to get a
reference that is not bound to a position. Adding the same `Table` twice is a
self-join and needs nothing else — an input is a position, which a self-join
distinguishes and a name cannot.

The `own` side of a join condition is a **column name of the table being
added**, not a free reference, so it cannot be pointed at another input. A
reference is accepted there too and is checked.

`tests/test_refs.py` asserts the built protobuf rather than the returned rows,
because the failure this guards against is a request the server *accepts*.

### The session threads the read token; the caller can still see it

Write and then read, and the read sees the write — from a replica, with no
token in the caller's code. `Session` holds a watermark: the highest sequence
it has observed from its own commits and (by default) from the views that
served its reads. Every read carries it as `Freshness.at_least`.

Hiding it completely would have been worse than exposing it. So:

| you want | you write |
|---|---|
| read your own writes | nothing; it is the default |
| a read that may be stale | `freshness=Freshness.any()` |
| the writer's view | `freshness=Freshness.latest()` |
| to know how stale an answer was | `stream.served_by` |
| to hand freshness to another process | `session.watermark` → `other.observe(token)` |
| a scope per end user | `client.session()` |
| to stop paying for monotonic reads | `Client(..., monotonic_reads=False)` |

A `Client` *is* a `Session`, because the common case is one process acting as
one caller. A process serving many users should call `session()` per user, so
that one user's write does not pin another's reads to the writer.

`tests/test_freshness.py` runs against a head node whose pool holds a replica
that is empty and will never catch up, and every test that asserts a token
worked also asserts the control — that the same read without one really does
miss the write. Without the control the whole file would pass against a client
that ignored freshness entirely.

### Errors are a hierarchy about retryability, and there is no string matching

```
SlateError
    Retryable                 ← the same call again may succeed
        Conflict              ABORTED — retry the whole transaction
        Unavailable           UNAVAILABLE — retry, possibly elsewhere
            NotLeader         ... and `.leader` says where
        ResourceLimit         RESOURCE_EXHAUSTED
    InvalidRequest  NotFound  AlreadyExists  PermissionDenied  Unauthenticated
    UnknownOutcome            UNKNOWN — the write may have landed. NOT Retryable.
    DataLoss  DeadlineExceeded  Cancelled  InternalError
```

`UnknownOutcome` not being `Retryable` is the load-bearing line: the server maps
a commit timeout to `UNKNOWN` precisely so a client does not retry an insert
that may already have succeeded, and a hierarchy that put it under `Retryable`
would undo that decision everywhere.

`Session.transact` retries `Conflict` and nothing else — not `Unavailable`,
even though it is `Retryable`, because a fenced writer is permanent for that
node and the retry that helps is against a different one. That is
`RecordStore::transact`'s own rule.

The mapping stops at the status code. The finer distinctions behind
`UNAVAILABLE` and `ALREADY_EXISTS` exist only in the message text, and matching
on prose is not something this client does; finding 6 explains why and what
would fix it.

### The generated stubs are committed

Rather than generated at install time. The argument is `build.rs`'s, one layer
out: a build that depends on an external code generator fails differently on
every machine, and generating at install time makes a wheel's contents depend
on which `grpcio-tools` the installing machine resolved. A committed generated
file also appears in a diff, so a protocol change is reviewed rather than
absorbed.

The cost of committing a generated artifact is drift, so the freshness is
asserted rather than trusted: `tests/test_generated.py` regenerates into a
temporary directory and requires byte equality. Regenerate with
`python scripts/generate_proto.py`.

Which is why the generators are pinned to exact versions in the `dev` extra.
The paragraph above objects to a wheel whose contents depend on which
`grpcio-tools` the installing machine resolved — and with a `>=` pin the
*freshness test* had that same dependency. It went red in CI with nothing
changed in this repository, on a `grpcio-tools` release that emitted a
different `_pb2_grpc.py`. Raising a pin is a deliberate change: bump it,
regenerate, and commit the stubs together, so the diff shows what the new
version did.

Types come from `mypy-protobuf` rather than protoc's own `--pyi_out`, because
protoc types the messages and leaves the *service stub* untyped — and an
untyped stub is exactly the boundary where a wrong field name survives type
checking.

---

## Deadlines

Every call takes the session's deadline, and there is none by default:

```python
client = Client("127.0.0.1:50051", timeout=5.0)   # seconds
slow = client.with_timeout(60.0)                  # a view, for a big scan
rows = list(slow.query(Query(TRIPS)))
```

`with_timeout` returns a *new* session over the same connection and the same
freshness scope — so a write through one is visible to a read through the
other, and a short deadline cannot be left switched on by a caller who forgot
to restore it. A transaction inherits the deadline of the session that began
it.

The deadline covers a whole streaming call rather than each message, which is
what a gRPC deadline means and why this is per call rather than one number for
the connection: a point get and a hundred-thousand-row scan do not want the
same value.

**Seconds here, milliseconds in the TypeScript client** — each follows its own
language, and each names the unit in the parameter.

There is no default, deliberately. Adding one would turn a slow query into a
failure in every caller that upgraded without asking for it. The cost of that
choice is that a head node which accepts a connection and then stops answering
blocks a caller for ever, which `tests/test_deadlines.py` demonstrates.

## Layout

```
src/slate/
    expr.py        ColumnRef, Columns, Expr — the reference model
    scalar.py      computed values
    query.py       Query, JoinQuery, AggregateQuery, Agg
    schema.py      Table — the local copy of the catalog, and its limits
    values.py      Python ↔ Value, and the integer-width problem
    freshness.py   Freshness, ReadToken, ServedBy, Watermark
    rows.py        Row, JoinedRow, Group
    client.py      Client, Session, Transaction, the streams
    errors.py      the exception hierarchy
    _proto/        generated, committed
testserver/        a Rust head node with a `main`, because the crate has none
tests/             pytest, against that head node — no mocks
scripts/
    generate_proto.py
    mutate.py      break each thing; check a named test notices
```

## Running the tests

```
uv pip install -e '.[dev]'  # or pip; uv is what CI uses
pytest                      # builds the head node with cargo and drives it
ty check                    # over src/slate and tests/
ruff check .                # a finding here is a failure, not a suggestion
python scripts/mutate.py    # the mutation table
```

Every test runs against a real head node started as a subprocess. There are no
mocks: a mock is a second statement of what the server does, written by whoever
wrote the client, so it agrees with the client's misunderstandings — and the
whole value of a first client is finding out where those are.

The strongest test is `tests/test_oracle.py`. The test server runs a fixed list
of reads **in process** against `slate-kernel` and dumps the answers; the Python
tests build what should be the same reads with this SDK, ask them over the
socket, and require the two to agree. The two statements are deliberately not
shared — the Rust side is written against the kernel's flat ordinals and the
Python side against the wire's `ColumnRef` — because a helper that built both
would make them agree by construction.

## Not covered

`PROTOCOL-FINDINGS.md` ends with the list. In short: `UNKNOWN` and `DATA_LOSS`
(no way to provoke them with an in-memory store), a real fence, a replica that
genuinely catches up, and every `Value` type the fixture schema has no column
of — vectors, UUIDs, bytes, and the time and distance scalars are built and
type-checked and never executed.
