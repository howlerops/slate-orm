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

### Joins beyond two tables

A join is two secured cursors combined; three tables would be a tree of them,
and the planner would then have to choose a join order, which is the part of
query optimisation that actually needs a search. Two tables covers association
loading, which is what the typed layer is for. The shape composes — a
`JoinedRow` keeps each side's row intact rather than flattening them — so a
third side is a planner problem rather than an executor one.

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
