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

A transactional read is the one thing a takeover still interrupts, and that is
inherent — it is a read of the writer's transaction. A client that must keep
reading through a handover reads outside a transaction.

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
  predicates and whole queries: converting out and back must be the identity.
  A damaged wire form is fed back in to show the comparison is sharp enough to
  notice a dropped `ILIKE` flag, and the generators are separately asserted to
  reach every variant.
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

- **Joins, chains, aggregates and computed columns on the wire.** Each needs an
  ordinal space or a grouping model of its own in the schema, and half of one
  would be worse than none.
- **A read-only transaction pinned to a replica.** `ReplicaMode::Pinned` is the
  right substrate for a consistent multi-read export, and the session type for
  it is not a write transaction.
- **`update_many`.** `insert_many` overlaps its reads across a batch; a
  multi-row update still costs a round trip per row, here as in the kernel.
- **Any performance number.** Nothing in the head node has been benchmarked.
  The batch size on a query stream and the lease's fifteen-second term are both
  chosen by argument, not measurement, and are marked as such where they are
  defined.
