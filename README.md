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

### Reading a set of keys

`WHERE id IN (…)` over a primary key becomes the reads it actually is, issued
together, rather than a scan looking for them:

| keys | before | after | model says | measured per unit cost |
|---:|---:|---:|---:|---:|
| 10 | 58.8 ms | **2.3 ms** | 1.0 | 2.30 ms |
| 50 | 59.3 ms | **8.1 ms** | 4.0 | 2.03 ms |
| 200 | 59.7 ms | **30.1 ms** | 13.0 | 2.31 ms |

Loading a set of records by id is the commonest thing an ORM does after
loading one, and it was scanning the whole table to do it.

The set is a *candidate*, not a replacement: four hundred keys cost twenty-five
waves where the scan they replace costs three, so the cost model has to be able
to choose the scan. Offering only the reads hid the better plan and let an
unrelated index win by default — which is what the test caught.

### Overlapped reads are cheaper, and the model has to know

An index scan does not wait for each row lookup in turn — it keeps sixteen in
flight — so `n` lookups cost `ceil(n / 16)` round trips, not `n`. The model
charged them one apiece, which made it prefer a 57 ms table scan over a 20 ms
index scan. Fixing that moved the crossover from about 1% of the table to about
6%, and a plan's estimated cost is now within the timer's resolution of its
measured wall time on every shape in the benchmark.

A window narrows the pipeline: fetching sixteen rows to return ten spends six
round trips on rows nobody sees, so the cursor caps its prefetch at
`limit + offset` and the cost model uses the same depth.

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

### Patterns and vectors

```rust
// LIKE, ILIKE, and a regular expression. An anchored pattern becomes scan
// bounds; the rest are filters.
Expr::like(url, "https://example.com/%");
Expr::ilike(title, "%Rust%");
Expr::matches(referer, r"^https?://(?:www\.)?([^/]+)/");
```

Nearest-neighbour search is not a separate engine. A distance is an ordinary
computed value, so k-NN is `ORDER BY` a distance with a `LIMIT`, and the
bounded heap that serves every other `ORDER BY ... LIMIT` serves this one:

```rust
let distance = Query::computed(&table, 0);
let query = Query::all()
    .filter(Expr::eq(tenant, Value::Str("acme".into())))
    .computing([Scalar::column(embedding).distance(target, Metric::Cosine)])
    .sort_by([SortKey::asc(distance)])
    .limit(10);
```

Because it is an ordinary query, a filter, row-level security and a projection
all apply to it without the search path learning what they are — a tenant
cannot be shown its neighbour's nearest neighbours.

This is exact search, not an approximate index: it reads every row the filter
admits. That is the right trade at the scale a filter leaves behind, and the
wrong one for unfiltered search over a very large table. A vector is refused in
a primary key or an index, at schema-definition time, because its order is
deterministic but says nothing about similarity — an index on one would sort
correctly and answer nothing.

### Joins

```rust
let join = Join::equating(Author::COLUMNS.id, Book::COLUMNS.author_id)
    .left(Query::all().filter(Expr::eq(country, Value::Str("UK".into()))));

let pairs: Vec<(Author, Option<Book>)> = txn.join_records(&ctx, &join).await?;
```

A join is **two secured reads**, not one privileged one. Each side is planned
through the same path a single-table query uses, so each is authorised and each
carries its own row filter. A join cannot see a row either side's policy hides,
because it never asks storage for rows — it asks two cursors that already
applied their policies. That is structural, not careful.

All four join types are here. A right or full outer join has to return inner
rows that matched *nothing*, which is not a fact any single probe can establish
— it is the absence of a match over the whole other side. A hash join already
holds one side in memory, so it flags the buckets that were probed and drains
the rest at the end; a nested loop cannot, so the planner will not choose one
there and refuses one that is forced.

A condition only a formed pair can answer goes in `having`, over the ordinal
space `JoinSchema` defines — the left table keeps its ordinals, the right
table's shift past its width:

```rust
let at = JoinSchema::of(&authors, &books);
let join = Join::equating(author_id, book_author_id).having(
    Expr::compare_columns(at.right(published), CmpOp::Lt, at.left(died)),
);
```

It is an ordinary `Expr`, so it goes through the same evaluator and the same
three-valued logic the security filter uses — not a parallel expression type
that would be a second place for those null semantics to drift. It behaves like
SQL's `ON`: a left row whose every candidate is rejected comes back unmatched,
not missing.

Comparing two columns of *different* types is refused rather than answered.
`Value`'s order is type-first — which is what makes the key encoding sortable —
so an integer against a float would order by type and give the same answer for
every row.

The planner picks between a hash join and a nested loop by cost, and both
directions are worth measuring, because the cost model makes a strong claim in
each:

| | rows read | wall | plan |
|---|---:|---:|---|
| 500 actors ⋈ 2500 events | 3,000 | 76 ms | Hash |
| the same, forced to a loop | 3,000 + 2,500 point reads | 158 ms | Nested Loop |
| one actor's events | 5 | 6.6 ms | Nested Loop |
| the same, forced to a hash | 2,500 | 62 ms | Hash |

A scanned row costs a hundredth of a round trip and a probe costs at least one,
so a hash join wins the general case by 2.1×. But "one row against ten thousand"
is what an ORM does all day — load a user, then their orders — and there the loop
wins by 9.4×. The planner gets both right; `explain_join` says which it chose.

### Chains of more than two tables

```rust
let at = JoinSchema::over([&teams, &actors, &events]);
let chain = Chain::from(one_team)
    .join(JoinStep::equating(at.at(0, team_name), actor_team))
    .join(JoinStep::equating(at.at(1, actor_name), event_actor));

let rows: Vec<(Option<Team>, Option<Actor>, Option<Event>)> =
    txn.chain_records(&ctx, &chain).await?;
```

Left-deep and explicit: the tables join in the order written, and there is no
join-order search. That is the part of query optimisation that genuinely needs
one — the space is factorial and the estimates feeding it compound — and doing
it badly would be worse than not doing it, because a wrong order here costs
round trips rather than a constant factor.

A step can join back to any earlier table, not only the one before it, which is
what the positional ordinal space is for. Each step chooses its own algorithm
by the same rule as a two-table join, and each is a secured read, so a policy on
the third table applies as surely as one on the first.

Intermediates are materialised — each step is the next step's build side — so
the accumulated set is bounded at every step rather than left to the allocator.
`step_counts()` reports what each step actually produced, which is the first
thing to look at when a chain is slow and the estimate said it would not be:

```
every team -> actors -> events   2500 rows   3 scans, 3010 rows read   80 ms   [10, 500, 2500]
one team   -> actors -> events    250 rows   2 scans, 3000 rows read   75 ms   [1, 50, 250]
```

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

## Benchmarks

- `crates/slate-kernel/examples/perf_report.rs` — query shapes against a
  latency model, in I/O counts as well as milliseconds.
- `crates/slate-slatedb/examples/scan_tuning.rs` — scans against a real S3
  server, counting object-store requests.
- `crates/slate-clickbench` — the 43 official ClickBench queries, all 43 of
  which run, with every independently checkable answer verified against the
  same data read through `pyarrow`. Not comparable
  to published ClickBench scores; run because it is an adversarial workload
  nobody here designed for. See
  [`docs/clickbench.md`](docs/clickbench.md), which also covers the pgrust
  review.

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
- A **planner oracle** generates random queries and requires every access path
  to agree: the planner's choice, a forced table scan, and each index in turn
  must return identical rows. An optimiser that changes the answer is not
  optimising. It found three bugs on its first run — see
  [`docs/correctness.md`](docs/correctness.md).
- **Write failures are injected at every position** a transaction writes, and
  after each the store must still satisfy a full index/row consistency check.
  A row and its index entries land together or not at all.
- **Contention is tested by counting.** Eight tasks incrementing one row must
  total exactly the number of increments — a lost update is a number too small,
  a double-apply one too large — and the test also asserts that transactions
  really did conflict, since tasks that never overlapped would pass while
  proving nothing.
- **Untrusted input** — arbitrary bytes into the decoder, and caller-supplied
  `LIKE` patterns and regexes — must return an error rather than panic, hang or
  overflow the stack.
- The oracle extends to **joins, chains and aggregates**: four join types
  against three algorithms, a three-table chain against a hand-written triple
  loop, and grouping against a fold written out over the rows. Each also runs
  over a **tenant-scoped table with a composite key and mixed-direction
  indexes**, since the single-column case exercises none of the keyspace code
  that matters.
- **Writer handover** is covered on both sides of the takeover, including two
  writers genuinely overlapping across a lease change, and replicas are read
  from while the writer commits — a replica may lag, but the state it serves
  must be one the database was actually in.
- **Faults are injected below the storage engine too**, at the object store, so
  a crash lands inside SlateDB rather than above it. What survives must be
  coherent even when committed data is lost.
- The **cost model is calibrated against a real S3 server**, not a latency
  fixture. Doing that found it overcharging scans eighty times and
  undercharging index lookups forty — see
  [`docs/correctness.md`](docs/correctness.md).
- **Plans are snapshotted.** Eighteen query shapes across three table sizes,
  committed as text, so a cost-model change is one reviewable diff rather than
  a series of surprises: reverting one constant reports eleven changed plans at
  once. Borrowed from how Postgres' regression suite works; see
  [`docs/clickbench.md`](docs/clickbench.md).
- Row-level security is checked as a **matrix**, once per access path, rather
  than as a set of scenarios: a policy honoured by the table scan and skipped
  by the k-NN search is a leak, and the paths that skip it are the ones added
  last. Every single-table path is one row of that table.
- A store is closed and reopened to prove rows, index entries, tombstones and
  tenant isolation all survive a restart — including that a covering scan and
  a table scan still agree afterwards, which is how an index that came back in
  a different state from its table would show up. These run over the S3
  protocol as well as in memory, since reopening is exactly where a backend's
  manifest handling could differ.
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
- [x] Aggregates including `COUNT(DISTINCT)`, `GROUP BY`, `ORDER BY`,
      `LIMIT`/`OFFSET`
- [x] `LIKE` and `ILIKE`, with an anchored pattern becoming scan bounds
      rather than a filter
- [x] Regular expressions (`~`, `~*`, `REGEXP_REPLACE`) on a linear-time
      engine, because a pattern is caller input
- [x] Vectors and similarity search: `L2`, cosine and inner-product
      distance as an ordinary computed value, so k-NN is `ORDER BY`
      distance with a `LIMIT` and needs no separate search path
- [x] Scalar expressions — arithmetic, `length`, `CASE WHEN`, `COALESCE`,
      `DATE_TRUNC`, `extract` — computed per row and addressed by ordinal, so
      grouping, sorting and aggregation take them without changing
- [x] `HAVING`
- [x] Inner, left, right and full outer joins, hash or nested-loop by cost,
      both sides secured; conditions spanning both sides
- [x] Chains of three or more tables, in the order written, with per-step plans
- [x] `IN` over a primary key as a set of overlapped reads, not a scan
- [x] Equi-depth histograms, so a range estimate knows which range it is
- [x] Scan readahead on the SlateDB backend: 31x fewer object-store requests
- [x] Bulk writes: `insert_many`/`upsert_many` overlap the duplicate-key and
      unique-index reads (100 rows in 13 ms, down from 223 ms)
- [x] Benchmarks and a recorded baseline ([`docs/performance.md`](docs/performance.md))
- [x] A planner oracle, an access-path security matrix, restart/durability
      tests over both substrates, write-failure injection, contention tests and
      untrusted-input suites ([`docs/correctness.md`](docs/correctness.md)),
      which between them found a covering scan and a point get ignoring the
      projection, and a panic on contradictory bounds

Not built:

- [ ] gRPC head node — the single-writer server the topology above describes
- [ ] Writer leadership: SlateDB fences but does not elect, so a lease has to
      come from outside the database
- [ ] Python, Go and TypeScript SDKs, which need the head node first
- [ ] Migrations beyond additive nullable columns (no column drop or rename)
- [ ] Correlated column statistics — selectivities still multiply, which
      assumes the columns are independent. Measured: estimates run up to 20x
      out, and the plan chosen is unchanged in every shape tested, so this is
      not currently worth fixing
      (`cargo run --release -p slate-kernel --example correlation`)
- [ ] `IN` on a *secondary* index as several index ranges — worth about 1.6x
      against the 26x the primary-key case bought, so it waits
- [ ] Partial and expression indexes; foreign keys; `DEFAULT` and `CHECK`

## License

Apache-2.0
