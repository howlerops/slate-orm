# Topology: one writer, many readers

SlateDB allows a single fenced writer and any number of readers over the same
object storage. That is the shape of the system, and it is a real limitation
rather than a temporary one: this is a Neon-shaped database, not a Spanner-shaped
one. Writes do not scale horizontally. Reads do.

This note records how the record layer uses that, and — as much as anything —
which improvements were considered and left out, so the next person does not
have to rediscover the reasoning.

```
                 ┌──────────────┐
   writes ─────► │  writer head │ ── fenced; exactly one at a time
                 └──────┬───────┘
                        │ flush
                 ┌──────▼───────┐
                 │object storage│ ── S3 / MinIO / Tigris / R2
                 └──────┬───────┘
                        │ manifest poll
       ┌────────────────┼────────────────┐
   ┌───▼───┐        ┌───▼───┐        ┌───▼───┐
   │replica│        │replica│        │replica│   reads, routed by tenant
   └───────┘        └───────┘        └───────┘
```

## What is built

| piece | where |
|---|---|
| Read/write split at the storage trait | `slate_kernel::store::{KvSnapshot, KvTransaction}` |
| Read-only record view | `slate_kernel::RecordSnapshot` |
| Read tokens, freshness, watermark | `slate_kernel::token` |
| Routing across replicas | `slate_kernel::pool::ReplicaPool` |
| Conflict retry | `slate_kernel::retry`, `RecordStore::transact` |
| SlateDB replica | `slate_slatedb::SlateReader` |
| S3-compatible storage | `slate_slatedb::S3Config` |

## Consistency

Adding replicas breaks two things callers assume, and both break silently and
only under load.

**Read-your-writes.** Write to the head, read from a replica, the row is not
there. **Monotonic reads.** Two reads hit replicas at different lag and the
second sees less than the first — data appears to move backwards.

One mechanism fixes both. A commit returns the sequence it landed at
(`ReadToken`); a read carrying that sequence may only be served by a view that
has reached it. A session threads the highest token it has seen
(`ReadWatermark`), so no read is ever served by a view behind one the caller
already observed.

```rust
let token = txn.commit().await?;              // Option<ReadToken>
watermark.observe_commit(token);
let snapshot = pool.snapshot(watermark.freshness(), tenant.as_ref()).await?;
```

Three limits worth stating plainly:

- **The token is a durable sequence.** A replica reads object storage, so a
  write acknowledged before its flush (`Durability::Visible`) is not on any
  replica yet and cannot be. Read-your-writes through a replica requires
  durable commits.
- **A following replica is not point-in-time.** It can advance between two
  reads inside one logical operation. `ReplicaMode::Pinned` gives a genuinely
  fixed view; use it for a consistent multi-read operation or an export. The
  kernel is told which it has, via `KvSnapshot::is_point_in_time`, so that an
  index entry whose row has since been deleted is skipped on a moving view and
  reported as corruption on a fixed one.
- **`Freshness::Latest` means the writer.** It is the only view that can see an
  unflushed write, so a pool without a writer cannot serve it and says so
  rather than substituting a replica.

## Routing

Round-robin is the obvious default and the wrong one. Reads come from object
storage, where a cache miss is a network round trip and a hit is free, so the
hit ratio dominates read latency far more than balance does. The keyspace is
tenant-prefixed, so a tenant's working set is a contiguous range: sending a
tenant's reads consistently to one replica keeps that range in its block cache,
while spreading them evenly gives every replica a cold copy of everything.

This falls out of making tenant scoping physical for security reasons. The same
key prefix that isolates a tenant also makes it cacheable.

Placement is rendezvous hashing, not modulo, so losing a replica remaps only the
tenants that were on it rather than reshuffling every cache in the fleet. The
hash is FNV-1a plus a MurmurHash3 finaliser — the finaliser is not decoration:
rendezvous compares whole hash values, and FNV's high bits mix weakly for short
inputs, which skewed a four-replica split to 13%/37% before it was added.

Reads with no tenant to key on fall back to round-robin, because there is no
locality to preserve.

## The writer

**Conflicts are ordinary.** A unique index is enforced by two writers colliding
on one key, so conflicts are part of normal operation. `RecordStore::transact`
runs a closure in a transaction and retries only what can succeed on a second
attempt: a unique violation, an access denial or a fenced writer fails
identically forever, and retrying them turns a clear error into a hang. Backoff
is jittered, because writers that collided once otherwise wake together and
collide again.

**Fencing is terminal.** A second writer takes over from the first. A first
writer that retried past that would be a split brain looping forever, so
`WriterFenced` is a distinct error and is not retryable. A head node should
shut down on it.

**A fenced writer cannot read either**, which is worth knowing before designing
a handover around it. `begin` returns `WriterFenced` before a transaction
exists, so reads in flight on the old writer fail alongside its writes — a
takeover is an interruption, not a graceful drain. That is defensible, since a
fenced writer's view is arbitrarily stale and it has no way to say how stale.
The consequence is that anything which must keep serving *through* a handover
has to be reading from a replica, which makes the read/write split above a
requirement rather than a preference. `handover.rs` pins this, along with the
cases either side of a takeover: a transaction opened before it is still
fenced at commit, fencing survives repeated attempts, the new writer inherits
the old one's rows *and* its unique index entries, and a chain of handovers
fences every earlier generation rather than only the last.

What is *not* handled: leadership. SlateDB fences, but it does not elect. If two
processes both try to be the writer, they will take turns fencing each other and
neither will make progress. Something outside the database has to hold a lease —
that belongs in the head node, not in this library.

## Considered and not built

### Group commit — probably already done

The obvious optimisation is to batch concurrent commits into one flush. SlateDB
already batches at the WAL and flush level, and several committers awaiting
durability naturally share a flush, so a batching layer here would duplicate it
and add a scheduling delay on top. Worth measuring before believing there is
anything to win.

### Pipelined index lookups — built

Deferred here on the grounds that the batch size was the whole question and
could not be chosen without a benchmark. With one, it can: sixteen reads in
flight took a sixty-row non-covering index scan from 128 ms to 10.8 ms.

The cost model deliberately does not divide the point-read cost by the prefetch
depth. Pipelining improves latency, not the number of round trips, and a planner
that costed latency would start preferring index scans under concurrency, where
round trips are exactly what is scarce.

### Covering (index-only) scans — built

Projections came first, as predicted, and then covering scans on top: 100 point
reads to none, 218 ms to 2.3 ms. See [`performance.md`](performance.md).

### Prefix bloom filters — cheap, unexplored

SlateDB exposes `PrefixExtractor` and `FilterPolicy`. Configuring one aligned
with the keyspace — table prefix, or table plus tenant — would let point lookups
skip SSTs that cannot contain the key. Given that tenant prefixes are already a
first-class part of the layout, this looks like a good fit and has simply not
been tried.

### Concurrent unique checks on the single-row path

`insert_many` overlaps its unique-index reads across the batch, but a single
`insert` with several unique indexes still does one read per index, serially.
Those reads are independent and could be issued together. Small, safe,
unmeasured — and worth less than the batch case, which is done.

### Join order, and streaming a chain

A chain joins its tables in the order written. Choosing that order is the part
of query optimisation that genuinely needs a search: the space is factorial,
the estimates feeding it compound, and a wrong choice costs round trips rather
than a constant factor. Doing it badly would be worse than not doing it, so it
is the caller's decision and the cursor reports what each step actually
produced so they can tell when they chose wrong.

A chain also materialises its intermediates, because each step is the next
step's build side. The last step need not: it could stream, the way a two-table
join's probe side does. That is a real optimisation, bounded in scope, and not
done.

### Pushing a cross-side condition down

A `having` conjunct that names only one side could become that side's filter,
where the planner could turn it into scan bounds — and for a nested loop it
could narrow each probe. That is sound for an `ON` condition on the inner side
of an outer join, which is the case that usually makes such a rewrite unsafe.
It is not done: the executor applies the whole condition per pair, and a caller
who wants the narrowing can put the conjunct in that side's `Query`, which is
what the docs tell them to do.

### Dropping redundant residual conjuncts

The planner keeps every conjunct in the residual and re-checks it per row, even
where the scan bounds already imply it. That costs CPU per row and buys two
things: a bound-derivation bug can make a scan slow but not wrong, and a
mandatory security predicate is enforced by evaluation rather than by the
planner having correctly turned it into a range. Dropping the redundant ones is
a real optimisation, but it moves security correctness into the planner and so
needs an argument, not just a benchmark.

## Failure modes

| what happens | what the caller sees |
|---|---|
| Two writers write the same key | `TransactionConflict`; `transact` retries |
| Two writers race for one unique slot | Same — the index key is the collision |
| A second writer starts | `WriterFenced` on the old one; terminal |
| Replica behind the required token | Pool waits, then falls back to the writer |
| Replica behind and no writer in the pool | `NoReplicaAvailable`, not a stale read |
| Replica advances mid-scan | Missing rows skipped, not reported as corruption |
| Index entry with no row on a pinned view | `CorruptIndexEntry` |

## Testing

The routing logic is unit-tested against fakes with controllable lag, including
the properties that justify the design: a tenant always lands on the same
replica, tenants spread within 20% of even across four replicas, and losing a
replica moves *zero* tenants that were not on it.

What only a real instance can show is tested against one: a replica genuinely
observing the writer's data, `wait_for_sequence` returning only once the replica
has actually reached the sequence, security applying identically on a replica,
and a second writer fencing the first.

Storage is tested over the S3 protocol as well as an in-memory object store —
the same assertions, both substrates — using an in-process S3 server so it stays
hermetic, plus a CI job against MinIO for behaviour specific to a
production-grade implementation.

## The head node

`crates/slate-server` is the process the note above describes and the library
deliberately is not: a gRPC surface over the record layer, plus the leadership
SlateDB does not provide. It answers the question left open under
[The writer](#the-writer) — *something outside the database has to hold a
lease* — and it puts that something in the same bucket the database is in.

```
                   ┌──────────────────────────────┐
   gRPC ──────────►│            Head              │
                   │                              │
                   │  writes ─► leader? ─► writer │──► object storage
                   │  reads  ─► pool ─► replica   │◄── manifest poll
                   └──────────────┬───────────────┘
                                  │ compare-and-set
                            ┌─────▼─────┐
                            │   lease   │  one object, same bucket
                            └───────────┘
```

### The lease decides who tries; the fence decides who wins

The lease is a single object, taken and extended with a conditional write
(`PutMode::Create`, then `PutMode::Update` against the version last read). It
carries a generation, a holder and an expiry, in plain text so an operator can
read it with `cat`.

It is worth stating plainly what that does and does not buy, because a lease is
the classic thing to believe more of than it says.

- **It cannot promise mutual exclusion.** The holder checks the expiry against
  its own clock, and between the check and the write it can be descheduled,
  garbage-collected or paused for longer than the whole term. There are moments
  when two processes both believe they hold it, and nothing at this layer
  removes them.
- **It does promise that exactly one process wins a contested acquisition**,
  because the write that takes it is a compare-and-set rather than a read
  followed by a write — and that generations never repeat, so two processes
  that briefly overlap can always be ordered.

Safety is still the fence. That division is the design: the lease exists to
stop the fencing happening over and over, not to prevent it. The head node is
wired to believe the fence over the lease, and steps down on `WriterFenced`
even while its own lease still looks perfectly valid — which, in the case that
matters, it does.

etcd or ZooKeeper would give a better lease, with a real session and
revocation. They would also be a second stateful system to run, and the
deployment this project targets is a bucket. Object storage already offers the
one primitive a lease needs.

### Being fenced is terminal, and the node keeps serving reads

This note says a head node should shut down on `WriterFenced`. The server does
something slightly different, and the difference falls out of the read/write
split: writes go to the leader's store, reads go through the pool, and the pool
does not care who the leader is. So a fenced node refuses writes permanently
and carries on answering reads. Shutting down would drop those connections for
no gain — the interruption this note describes is to reads *on the writer*, and
there are none once writes are refused.

Refusing writes is local, from a watch channel, rather than a forwarded storage
error. After a fence the writer's `begin` fails anyway, but it fails after a
call into SlateDB, and a node that has stepped down should not be making those.
The refusal is `UNAVAILABLE` with the current holder in a `slate-leader`
trailer, because the request should be retried — just not here.

Campaigning again is refused for the life of the process. A node that treated
fencing as a bad moment would win the lease, open a store that is permanently
fenced, and serve nothing while looking healthy.

### Where a request goes

| | destination |
|---|---|
| Write, node holds the lease | this node's writer store |
| Write, node does not | refused, `UNAVAILABLE` + `slate-leader` |
| Read, no transaction | the pool, by freshness and tenant affinity |
| Read, inside a transaction | that transaction, on the writer |

Reads are routed on the *principal's* tenant, not one dug out of the filter: on
a tenant-scoped table the security layer forces the principal's tenant onto the
predicate anyway, so it is the tenant whose key range the read will actually
touch. Every read response says which view served it and at what sequence, so
routing is visible rather than inferred.

Saying so truthfully constrains the API. Asking the pool where a read went,
after it has gone, is not the same question as asking where to send it: the
round-robin counter has already moved, and the answer names a replica that
served nothing. So `ReplicaPool::snapshot_from` returns the view *and* the store
that opened it, out of one decision, and the head node keeps no catalog,
security catalog or statistics of its own for reads — it reads them from the
pool it already routes through. The earlier shape held a second copy with
nothing but care keeping the two equal, which is the shape that already cost
this project a latency fixture and a cost model disagreeing about rows per
block.

A transactional read is the one thing a takeover still interrupts, and that is
inherent — it is a read of the writer's transaction. A client that must keep
reading through a handover reads outside a transaction.

### Naming a column when the row comes from more than one place

Joins, chains, aggregates and computed columns are the four things the kernel
could do and the wire could not, and the reason they were held back is that
each appears to need an ordinal space of its own. A joined row's columns come
from several tables. A computed value occupies a slot no table declares. A
grouped row is its keys followed by its aggregates. Three shapes, and three
ad-hoc encodings would have been worse than none.

There is one encoding. **A column reference names a producer and an index
inside it** — `ColumnRef`:

```proto
message ColumnRef {
  uint32 input = 1;            // which table of this request, counting from 0
  oneof of {
    uint32 column = 2;         // an ordinal that input's table declares
    uint32 computed = 3;       // the nth value the query for it computes
    uint32 group_key = 4;      // the nth GROUP BY key
    uint32 aggregate = 5;      // the nth aggregate
  }
}
```

Every place the protocol used to carry a bare `uint32 column` now carries one
of these, so the single-table case is the degenerate one — `input 0`, kind
`column` — rather than a second scheme sitting beside the first. The server
resolves it into the flat ordinal the kernel evaluates against.

That last part is the point. The kernel's own model *is* flat: `JoinSchema`
packs a joined row by table width, `Query::computed` puts the `i`th computed
value at `width + i`, and a group is `key.len() + n`. A client could do the
same arithmetic — and to do it, it would need every table's width.

#### What was rejected, and what it would have cost

- **Flat ordinals, computed by the client.** The kernel's model lifted
  verbatim. It needs widths this protocol deliberately does not publish, and
  the day a column is added to an earlier table every reference past it points
  somewhere else, with no error anywhere. That is the "same query, different
  answer" shape this project treats as the worst kind of bug, and it would
  arrive during a migration rather than during a test run.
- **A fixed stride** — input *i*'s column *c* at `i × 1024 + c`. Removes the
  width lookup and replaces it with a hard limit on table width that wraps
  silently past it. `ColumnRef` is this idea with an unbounded stride, which
  is the whole of the difference.
- **Qualified names** (`books.title`). Rejected twice over: the protocol
  addresses columns by ordinal precisely so a predicate needs no fallible name
  lookup — the derive macro generates the constants — and a self-join has two
  inputs sharing one table name. An input is a *position*, which a self-join
  distinguishes and a name cannot.
- **A separate expression type per shape** — `JoinExpr`, `GroupExpr`. The
  kernel refuses this for its own `having`, on the grounds that it would be a
  second place for three-valued null semantics to be got wrong. On the wire it
  would have been a third.
- **A `Describe` RPC** publishing the catalog so a client could do the
  arithmetic. It puts the schema on the wire, which the `.proto`'s opening
  comment refuses, and it turns every query into either two round trips or a
  cache that can go stale — a stale width being a silently re-pointed
  predicate again.

#### Two refusals it makes decidable

This is where the choice earns more than tidiness. Both of these are *kind*
mismatches rather than ordinals that happen to fall in range, so both can be
refused with a message that says which:

- **`HAVING` on a column that is not grouped.** SQL's "column must appear in
  the GROUP BY clause". Under flat ordinals, ordinal 2 of a grouped predicate
  is a perfectly legitimate group key and there is nothing to detect.
- **A computed value named from across a join.** The kernel's joined space is
  packed by *declared* table width, so a computed value has no slot in it and
  a flat ordinal naming one would land on the next table's first column. The
  reference is refused with the reason; the computed value is still returned in
  its own input's row and can still be used in that input's own filter.

#### What a multi-table read means for routing and freshness

A join is *n* secured reads of **one** snapshot. The head node routes once,
opens one view, and reads every input through it. So:

- there is one `served_by` in the whole response, not one per table;
- the freshness a client asked for applies to the entire result — a token
  satisfied for one table and not another is not a state the database was ever
  in;
- affinity is the principal's tenant if *any* input is tenant-scoped, since
  that is the key prefix the read will touch.

Doing otherwise would mean two calls into the pool, two views at two
sequences, and nothing truthful to put in `served_by` — the same constraint
that made `ReplicaPool::snapshot_from` return the view and the store together.

#### One request shape, two kernel paths

`JoinQuery` is a list of inputs joined in order. Two inputs become a kernel
`Join` and three or more become a `Chain`. That is a dispatch rather than a
difference the client sees: the kernel's two-table join can choose *which*
side to hold in memory and a chain step cannot, because its earlier rows are
already there.

Three things a join input may not carry, refused rather than ignored, because
the kernel documents them as ignored and a client setting one would be relying
on an accident: a `sort` (which would not order the join), a `limit` and an
`offset` (which would change the answer rather than shorten it). `JoinQuery`
carries the limit and offset that do apply. An algorithm may be forced, and
unlike an index hint it is not advice — a nested loop cannot preserve
unmatched rows of the second input, so asking for one on a right or full outer
join is an error rather than a quiet fallback to a hash join.

`build_limit` may be lowered by a client and not raised: the server's limit is
what stops a mistyped join key becoming an out-of-memory kill, and a limit the
client can raise is not a limit. Raising it is clamped and reported as a
warning rather than refused.

### Two things the wire format refuses to guess

An identity never appears in a request body. The `.proto` has no principal
field anywhere; the `SecurityContext` is built from transport metadata by an
`Authenticator` the deployment supplies, and nothing in the crate can produce a
superuser.

An unset `oneof` is an error, not a default. proto3 cannot tell an unset field
from a zero one, so a `Value` with no kind set is most likely a client built
against a newer schema — reading it as null would quietly change the predicate
it appears in.

### What was tested

- **The wire conversion round trip**, as a property over generated values,
  predicates, computed values, aggregates and whole queries: converting out and
  back must be the identity. A damaged wire form is fed back in twice, to show
  the comparison is sharp enough to notice a dropped `ILIKE` flag and a
  `DATE_TRUNC` moved from hours to minutes, and the generators are separately
  asserted to reach every variant — sixteen of them for `Scalar`, whose match
  is exhaustive so a new kernel variant fails to compile rather than going
  untested.
- **That the wire's arithmetic is the kernel's.** `ColumnRef` exists so a
  client never computes an offset; the server still does, and a unit test
  requires every resolution to land exactly where `JoinSchema` and
  `Query::computed` put it. A mismatch there would put a predicate on the wrong
  table with no error anywhere.
- **An oracle for joins, chains, aggregates and computed columns**, which is
  the strongest test in the crate. The same kernel `Join`, `Chain` or grouped
  query is run in process *and* converted to its wire form and asked over a
  socket, and the two must return identical rows. Ninety-six two-table cases —
  four join types against the planner's choice and each forced algorithm,
  across six request shapes — plus a three-table chain, a self-join whose
  second step joins back to the *first* input, every aggregate function,
  grouping by a computed value, `HAVING` over a key and over an aggregate, and
  a case per `Scalar` shape a string or integer column can reach. Rows are
  compared as multisets, since a join has no inherent order and the three
  algorithms genuinely produce different ones. Where the kernel refuses a
  combination the wire must refuse it too: a fallback to a hash join for a
  forced nested loop on a right outer join would return the inner rows and a
  short answer, which no assertion about rows would ever catch.
- **A row-level-security matrix for multi-table reads**, in the shape
  `rls_matrix.rs` established: every join type against every algorithm is one
  cell, plus the chain, the aggregate and the explanation, and every failure is
  collected before anything is asserted. Three policies of three different
  shapes, so a policy applied to the wrong input removes a different set of
  rows rather than the same one; a hidden row on the inner side makes the outer
  row *unmatched* rather than missing, so the join is not an existence oracle;
  a second tenant holds a row owned by the same principal id. The control runs
  the same join in process as a superuser and requires the forbidden rows to
  appear, since nothing on the wire can produce one and without it every cell
  could be passing because the rows were never there. And the explanation is
  checked separately from the rows, because a join can return the right rows
  for the wrong reason when two policies happen to overlap: each input's plan
  must carry its *own* policy in its residual and not the other's.
- **A query differential over gRPC.** Every filter, sort, limit and offset in
  the sweep is run under the planner's choice and under each index forced, and
  all of them must agree with a filter and comparator written out again in the
  test. Every sort ends in the primary key so ties are pinned. 448 queries;
  the filters are separately asserted to select some but not all rows.
- **The lease, against a deliberately wrong implementation.** A
  read-then-write lease passes every single-threaded test anyone would write,
  so the same harness runs against both: the compare-and-set one tells a
  replaced holder it was replaced, and the naive one renews straight over its
  successor. That failure is asserted, and is the reason to believe the harness
  proves anything.
- **A real fence through the head node.** Two `SlateStore`s over one object
  store, which is what a replacement node amounts to. The fenced node refuses
  writes as `UNAVAILABLE`, reports itself stepped down, keeps serving reads
  from a `SlateReader` replica — with the read that *asks for the writer*
  failing as the control — and releases its lease so the successor need not
  wait out a term.
- **That the refusal is local**, by counting calls into the store: five writes
  after the first fence reach it zero times.

### Not built

- **A grouped join.** The kernel groups over a single-table cursor and has no
  grouped join; offering one here would mean a second implementation of
  grouping living in the head node, over rows it had already streamed — which
  is exactly where the projection narrowing that lets `COUNT(*)` read no
  columns at all would be lost.
- **`ORDER BY` over groups**, and therefore a limit on them. Same reason: the
  kernel has no ordering over groups, so a comparator written in the head node
  would be a second statement of the sort rules with nothing to be an oracle
  against. Groups come back in the kernel's own order, ascending by encoded
  key, and all of them come back.
- **A read-only transaction pinned to a replica.** `ReplicaMode::Pinned` is the
  right substrate for a consistent multi-read export, and the session type for
  it is not a write transaction.
- **`delete_many`.** `insert_many` and `update_many` overlap their reads across
  a batch; a multi-row delete still costs a round trip per key, because a
  delete walks a foreign-key closure and two keys in one batch can reach the
  same doomed row by different paths. Batching it means unioning those closures
  before writing anything, which is a different change from the other two.
- **The head node under concurrency.** It is measured now — see
  `docs/performance.md` — but every measurement is one request at a time, so
  the per-stream channel and the task-per-transaction design have never been
  under pressure.
