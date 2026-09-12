# slate-orm

A **record layer** for [SlateDB](https://slatedb.io) — typed schemas, secondary
indexes, query planning and row-level security over an ordered key-value store
on object storage.

The closest blueprint is FoundationDB's Record Layer rather than an ORM. The
derive macro is a surface; the value is in the kernel underneath it, which owns
the keyspace, index maintenance, and the point where access policy is enforced.

> **Status: early.** The Rust record layer works end to end and is tested,
> including a cost-based planner, index-only scans, aggregates, read replicas
> and S3-compatible storage. The gRPC head node and the Python, Go and
> TypeScript SDKs are not built yet. See [Status](#status).

```rust
use slate_orm::{Aggregate, Expr, Query, Record, Records, SortKey, Value};
use uuid::Uuid;

#[derive(Record)]
#[record(table = "users", id = 1, tenant = "tenant_id")]
struct User {
    #[record(pk)] tenant_id: Uuid,
    #[record(pk)] id: u64,
    #[record(index(name = "by_email", id = 10, unique))]
    email: String,
    nickname: Option<String>,
    age: i64,
}

let txn = store.begin().await?;
txn.insert_record(&ctx, &user).await?;
txn.commit().await?;

// Column ordinals are generated as constants, so a predicate needs no
// fallible name lookup.
let recent: Vec<User> = txn
    .query_records(&ctx, &Query::all()
        .filter(Expr::eq(User::COLUMNS.email, Value::Str("a@example.com".into())))
        .sort_by([SortKey::desc(User::COLUMNS.age)])
        .limit(20))
    .await?;

// Counting and aggregating decode no records, and read no rows when an index
// holds the columns involved.
let total = txn.count_records::<User>(&ctx, &Query::all()).await?;
let oldest = txn
    .aggregate_records::<User>(&ctx, &Query::all(), &[Aggregate::Max(User::COLUMNS.age)])
    .await?;

println!("{}", txn.explain_records::<User>(&ctx, &Query::all())?);
```

## Layout

| crate | what it owns |
|---|---|
| `slate-tuple` | Order-preserving tuple encoding, the closed value model |
| `slate-schema` | Table, column and index definitions; the row body codec |
| `slate-kernel` | Keyspace, record store, planner, executor, RLS/RBAC, statistics |
| `slate-slatedb` | SlateDB backend, S3-compatible storage, read replicas |
| `slate-derive` | `#[derive(Record)]` and generated column constants |
| `slate-orm` | Typed surface; re-exports the rest |

No `unsafe` anywhere (`#![forbid(unsafe_code)]` in every crate).

## The decisions that matter

### Key encoding is the linchpin

Everything is one ordered keyspace, so a key's encoding *is* its index:

```text
row entry     0x01 <table id : u32 BE> <primary key tuple>
index entry   0x02 <index id : u32 BE> <tenant?> <indexed tuple> <primary key tuple>
```

`slate-tuple` guarantees byte order equals value order. Two properties do the
work, and both were found by property testing before either had callers:

- **Every element's encoding is prefix-free.** With a one-byte string
  terminator, `enc("\xff")` is a proper prefix of `enc("\xff\0")`. Ascending
  order survives that, but a descending column then fails to reverse them, and
  in a mixed-direction key the *following* element's bytes can decide the
  comparison outright. The terminator is two bytes so no element can prefix
  another, which makes "complement the bytes" an exact order reversal.
- **Integer encodings are canonical.** A non-minimal length is rejected on
  decode; accepting it would give one value two encodings and silently break
  the uniqueness of a unique index.

### Unique indexes collide by construction

A unique index with no null in its indexed values leaves the primary key out of
the key and puts it in the value. Two writers racing for the same slot then
write the *same key*, so the store's own write-write conflict detection picks a
winner — no read-check, and no dependence on the isolation level. A null puts
the primary key back into the key, which gives the SQL behaviour that nulls do
not collide with each other.

The read the record store does before a unique write exists only to produce a
useful error instead of a bare retry.

### Index maintenance is atomic, not eventual

Index entries are written in the same transaction as the row. There is no
background index builder to fall behind and no repair path to get wrong. Tests
assert that the number of index entries stays exactly `rows × indexes` across
mixed sequences of inserts, updates, upserts and deletes.

### Security is in the kernel, not in the ORM

This is the part most KV-layer projects get wrong, and it is structural rather
than a matter of care. Policies are not checked by a wrapper a caller could step
around: they are compiled into the predicate the executor evaluates and into the
bounds it scans. Every read and write on `RecordTransaction` takes a
`SecurityContext`, so there is no unauthenticated door — bypassing is possible,
but only by naming `SecurityContext::superuser()`, which is one grep away.

Before any row is touched: an RBAC check against the catalog, a mandatory tenant
restriction on tenant-scoped tables, then the OR of the matching RLS policies.
Writes are checked in both directions as in Postgres — an update may only touch
a row the caller can already see and may only leave behind a row they could have
written, so a policy cannot be escaped by editing your way out of it.

Everything fails closed:

- An empty security catalog denies every non-superuser action.
- RLS enabled with no matching policy yields **no** rows, not all rows.
- A tenant-scoped table refuses a context with no tenant rather than defaulting
  to "show everything".
- A hidden row reports as *missing*, never as *forbidden* — otherwise the error
  code is an existence oracle, and an upsert against a hidden row would be a
  silent overwrite.

**Tenant scoping is physical.** The schema layer requires a tenant column to be
the first primary key column, so row keys partition by tenant for free; index
keys get the tenant prepended. A tenant restriction is a narrower scan, not a
filter applied after reading a neighbour's bytes. There is a pleasant
consequence: because the tenant leads every key, the *policy's own* tenant
predicate is what makes the secondary indexes usable at all. Security narrows
the scan rather than costing a filter pass.

**Three-valued logic is a security property, not a nicety.** Under two-valued
logic `NOT (owner = :caller)` is true for a row whose owner is null, so a
deny-style policy would hand out exactly the rows nobody owns. Evaluation
follows SQL: the comparison is unknown, the negation stays unknown, the row is
withheld.

### The planner costs plans, and can be asked why

Queries are planned against statistics, in units of one object-storage round
trip: opening a scan 1.0, a row from an open scan 0.01, a point read 1.0. The
ratio has a blunt consequence — **a non-covering index scan only beats a table
scan when it selects under roughly 1% of the rows the scan would touch.** That
is not a quirk of the constants; it is what storage where every lookup is a
network round trip implies, and it is why covering an index matters far more
here than on local disk.

Without a cost model the planner preferred whichever index matched the most
equality terms, which on the benchmark corpus chose a plan 30× slower than
ignoring the index. See [`docs/performance.md`](docs/performance.md).

```rust
let plan = txn.explain(&ctx, &table, &query)?;
println!("{plan}");
// Index Only Scan using by_kind on events  (rows=100 cost=2.00)
```

`analyze` gathers the statistics by reading the table. It is an ordinary read,
so it sees what the caller can see — statistics gathered under a restrictive
policy describe that slice rather than the table.

Scan bounds are still treated as an optimisation only. Every conjunct stays in
the plan's residual and is re-checked per row, even when the bounds already
imply it. That costs a little work and buys two things: a bound-derivation bug
can make a scan slow but not wrong, and a mandatory security predicate is
enforced by evaluation rather than by the planner having correctly turned it
into a range.

### Index-only scans

A query says which columns it needs. When an index holds all of them — its own
columns plus the primary key it carries — the row is assembled from the index
entry and never read:

```rust
// No rows read at all: `by_kind` holds everything this touches.
let query = Query::all()
    .filter(Expr::eq(kind, Value::Str("signup".into())))
    .select([id, kind]);

// Counting reads no columns, so any usable index answers it.
let total = txn.count(&ctx, &table, &Query::all()).await?;
```

The check is over the projected columns *union the predicate's*, and the
security filter is part of that predicate — so a policy on a column the index
lacks forces the row read rather than being skipped by the optimisation.

### Queries

```rust
let query = Query::all()
    .filter(Expr::eq(region, Value::Str("eu".into())))
    .and(Expr::compare(amount, CmpOp::Ge, Value::I64(100)))
    .sort_by([SortKey::desc(amount)])
    .limit(20)
    .offset(40);

let rows = txn.execute(&ctx, &table, &query).await?.collect().await?;

let groups = txn
    .group_by(&ctx, &table, &Query::all(), &[region],
              &[Aggregate::Count, Aggregate::Sum(amount)])
    .await?;
```

`ORDER BY` is planned, not just executed: each access path knows the order it
produces, and a requested order is satisfied when it is a prefix of that. When
nothing produces it the result is materialised and sorted, and the cost model
knows a sort must see every matching row before returning the first.

### Schemas are code, and so are policies

No dynamic DDL. A table is defined once through a builder that validates
everything the kernel later assumes, so nothing downstream re-checks it. Row
bodies carry the schema version they were written at and each column records the
version it was added in, which makes evolution precise rather than positional:
columns added later read back as null (hence must be nullable), and a row from a
schema newer than the running build is an error rather than a silent truncation.

Policies are Rust functions of the security context rather than templates with
parameter substitution. That removes a bug class and matches the rest of the
design.

## Topology

SlateDB gives a single fenced writer with multi-reader scale-out, so this is a
single-writer database head with read replicas — a Neon-shaped system, not a
Spanner-shaped one. That is the honest limitation, and it is deliberate.
[`docs/topology.md`](docs/topology.md) covers it properly, including the
improvements considered and left out.

The short version:

**Reads scale, writes do not.** A `RecordSnapshot` is the read-only counterpart
of a `RecordTransaction`, and a replica-backed store simply lacks the write
methods rather than having them and failing at runtime. Both go through the same
secured read path, so a replica cannot be a way around a policy.

**Consistency is a token.** A commit returns the sequence it landed at, and a
read carrying that sequence may only be served by a view that has reached it.
Threading the highest token seen gives monotonic reads for free.

```rust
let token = txn.commit().await?;
watermark.observe_commit(token);
let snapshot = pool.snapshot(watermark.freshness(), tenant.as_ref()).await?;
```

The token is a *durable* sequence — a replica reads object storage, so
read-your-writes through one requires durable commits.

**Routing is by tenant, not round-robin.** Reads come from object storage, so
cache hit ratio dominates latency far more than balance does; because the
keyspace is tenant-prefixed, one tenant's working set is a contiguous range that
stays warm on one replica. The prefix that isolates a tenant for security is the
one that makes it cacheable. Placement is rendezvous hashing, so losing a replica
moves only its own tenants.

**Conflicts are ordinary, fencing is terminal.** `RecordStore::transact` retries
only what can succeed on a second attempt, with jittered backoff. A second writer
fences the first, and a writer that retried past that would be a split brain, so
`WriterFenced` is a distinct non-retryable error.

Commit latency is bounded by the flush to object storage. `Durability::Visible`
returns as soon as a write is committed and visible to readers but before it is
durable; it is available and documented as losing acknowledged writes if the
writer dies before its next flush, so it is only for callers that can replay.
The default waits.

Isolation defaults to serializable snapshot. The record layer's own guarantees
hold under plain snapshot isolation, because they rest on write-write conflicts
rather than on read tracking; it is application invariants that read one row and
write another which need serializable. The safer level is the default and
stepping down is explicit.

## Storage

Anything speaking S3 works: AWS, MinIO, Tigris, Cloudflare R2. The differences
between them are an endpoint, an addressing style and how credentials resolve,
and they all live in `S3Config`.

```rust
let store = SlateStore::open_s3(
    "/records",
    S3Config::new("records")
        .with_endpoint("http://127.0.0.1:9000")   // MinIO; Tigris and R2 are HTTPS
        .with_credentials("minioadmin", "minioadmin")
        .allow_http(true),
)
.await?;
```

## Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo bench                                                # criterion
cargo run --release -p slate-kernel --example perf_report  # I/O counts
```

Tests are written around guarantees rather than API surface:

- The tuple codec's properties are checked at 400k cases: byte order matches
  value order under arbitrary per-column directions, encodings are prefix-closed
  and prefix-free, value equality and encoding equality are the same relation.
- `bounds_never_lose_rows` checks that a planned query returns exactly what
  brute-force filtering returns, across a spread of predicate shapes and both
  scan orders. It was verified to have teeth by breaking `<=` into `<` and
  confirming it caught the lost boundary row.
- The security suite is written around what a caller *cannot* do: probe for
  hidden rows via error codes, reach another tenant by asking explicitly, escape
  a policy by updating out of it, or leak null-valued rows through a negated
  policy.
- Routing is tested on the properties that justify it: a tenant always lands on
  the same replica, tenants spread within 20% of even across four replicas, and
  losing a replica moves *zero* tenants that were not on it.
- Storage is tested over the S3 protocol as well as an in-memory object store —
  the same assertions, both substrates — using an in-process S3 server so the
  suite stays hermetic and needs no Docker daemon. A CI job additionally runs it
  against MinIO.
- What only a real instance can show is tested against one: two concurrent
  writers unable to both take a unique index slot, a replica genuinely observing
  the writer's data, security applying identically on a replica, and a second
  writer fencing the first.
- Performance claims are tested in point reads, not milliseconds — "this reads
  no rows" survives a different machine in a way "this took 2 ms" does not.

Set `SLATE_S3_BUCKET` and friends to point the storage suite at your own MinIO,
Tigris or R2 bucket instead of the in-process server.

## Status

Built and tested:

- [x] Order-preserving tuple codec
- [x] Schema, catalog, row codec with versioned evolution
- [x] Keyspace, storage abstraction, in-memory backend
- [x] Record store: primary key CRUD with atomic index maintenance
- [x] Scan/filter executor and heuristic index selection
- [x] RLS predicate injection and RBAC catalog
- [x] `#[derive(Record)]` and the typed surface
- [x] SlateDB backend, and S3-compatible storage (AWS, MinIO, Tigris, R2)
- [x] Read replicas, read tokens, tenant-affinity routing
- [x] Conflict retry and writer fencing
- [x] Cost-based planning with statistics, `analyze`, and `EXPLAIN`
- [x] Projections and index-only scans; pipelined index lookups
- [x] Aggregates, `GROUP BY`, `ORDER BY`, `LIMIT`/`OFFSET`
- [x] Benchmarks and a recorded baseline ([`docs/performance.md`](docs/performance.md))

Not built:

- [ ] gRPC head node — the single-writer server the topology above describes
- [ ] Writer leadership: SlateDB fences but does not elect, so a lease has to
      come from outside the database
- [ ] Python, Go and TypeScript SDKs, which need the head node first
- [ ] Migrations beyond additive nullable columns (no column drop or rename)
- [ ] Joins — everything today is single-table
- [ ] A bulk-write path: every insert still does a read for the duplicate-key
      check, so a 100-row load spends 100 round trips before writing anything
- [ ] Histograms, so a range estimate is better than a fixed guess; correlated
      column statistics
- [ ] `IN` as multiple index ranges (today it is a residual filter)
- [ ] Partial and expression indexes; foreign keys; `DEFAULT` and `CHECK`

## License

Apache-2.0
