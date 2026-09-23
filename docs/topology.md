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

### The poll interval and `catch_up` are one number, and were two

`RoutingPolicy::catch_up` is how long the pool waits for a replica to reach a
required sequence before giving up and sending the read to the writer. A
replica can only reach that sequence when it next reads the manifest. So the
poll interval is not a neighbouring setting: it is *what creates the lag*
`catch_up` waits out, and a poll longer than the budget means no read carrying
a sequence can ever be served by a replica.

The shipped daemon had them 40× apart in the wrong direction. `SlateReader::open`
takes `DbReaderOptions::default()`, whose `manifest_poll_interval` is **ten
seconds**, against a `catch_up` of 250 ms — so every read-your-writes read waited
out the whole budget and fell through to the writer, which is the one node this
whole topology exists to keep free. Measured over the routing section:

| poll | `AtLeast(just-committed)` | fell back to the writer |
|---|---:|---:|
| 50 ms | 26.98 ms [10.90–46.32] | 0 of 64 |
| 10 s | 251.87 ms [251.66–252.27] | 64 of 64 |

The tell is the spread rather than the median: ±0.2% is not a wait, it is a
timeout expiring every single time. The controls did not move —
`Freshness::Any` 206 vs 232 µs, `Latest` 205 vs 227 µs, `pool.route` 96 vs
97 ns — so the replicas and the routing were fine and only the poll was wrong.

Nothing about the *rows* was wrong, which is why nothing caught it: the writer
answers the same query with the same answer. `served_by` is the difference, and
that is what the regression test asserts on.

`slate-serverd` now derives the poll from `catch_up` — a fifth of it, floored at
20 ms — rather than carrying a second constant that has to be kept below the
first by hand, and refuses a configured `[[replicas]] poll_interval` at or above
`catch_up` rather than starting into that failure deliberately.

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

#### Except where it does not

"Object storage already offers the one primitive a lease needs" is true of S3,
R2, Tigris, GCS, Azure and MinIO, and false of `object_store`'s
`LocalFileSystem`, which implements `PutMode::Create` and returns
`NotImplemented` for `PutMode::Update`. There is no compare-and-swap on a POSIX
file by ETag to implement one with.

That asymmetry used to be silent, and silent in the worst available way. A
`local` node *took* the lease — `Create` works — and then every renewal failed
with a storage error that looks exactly like a slow bucket, which the leadership
loop correctly declines to hand the database over for. The term lapsed under a
healthy leader, nothing was released, and no successor could take over an
expired term either, because that is a conditional update as well. A `local`
database was a one-start database.

`ObjectStoreLease::acquire` now asks the store first, with a conditional write
that cannot land — an impossible version, at a scratch path — and refuses with
`LeaseError::Unsupported` when the store says `NotImplemented`. One request, on
the first acquisition only, and nothing is written. `Leadership` treats that
refusal as terminal rather than retrying it forever: a node pointed at such a
store is permanently a reader and its `Leadership` RPC says so, which is the
whole difference between a limitation and a defect.

What `local` uses instead is an advisory `flock`, in `slate-serverd`'s
`filelease.rs`. For the deployment `backend = "local"` describes — one machine,
one directory — it is a *better* lease than the object one: mutual exclusion
the kernel enforces rather than two clocks agreeing, and released when the
process exits however it exits. Its limits are that it is advisory and
host-local, so two machines over one NFS mount are not separated by it, and a
deployment with more than one machine uses `s3`.

The alternative that would have removed even that caveat — a lease whose
generation lives in the object's *name*, taken with `Create`, which is how
SlateDB writes its own manifest and therefore how SlateDB fences over a local
directory — is argued and rejected in `filelease.rs`: it is slower than the
conditional update everywhere the conditional update exists, weaker than a
`flock` on one host, and its one advantage is an NFS safety claim this
repository has no way to test.

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

### A node that never won the lease serves reads too

The paragraph above was true and incomplete, and the gap was large enough to
contradict this note. A *fenced* node keeps serving reads because it already
has a store open. A node that loses the campaign at **startup** has no store,
and until recently could not get one safely: `Head::new` required a writer, and
opening a SlateDB writer is the fence. Its only options were to kill a healthy
leader on the way to discovering it was not the leader, or to refuse to start.
`slate-serverd` refused to start, and this note went on describing a follower
that served reads while no process could be one.

`Head::read_only` is the missing half. It takes replicas, a lease and an
authenticator, and no writer store at all:

```
   node A (leader)                      node B (read-only)
   ├── SlateStore  ── writes            ├──   (nothing)   ── writes refused
   └── pool ── replica ── reads         └── pool ── replica ── reads
        │                                              │
        └────────── one object store ──────────────────┘
```

Three consequences, all of them the point:

- **A follower coming up does not fence the leader**, because it opens nothing
  that could. That is the assertion the tests are built around; a test that
  only checked the follower's reads would pass against the design this
  replaces, since a fenced leader's replica goes on answering perfectly well.
- **`Freshness::Latest` is refused on a follower**, not substituted. The pool
  has no writer, and the one view that can see an unflushed write is the
  writer. This is the existing rule — a pool without a writer refuses `Latest`
  — arriving at the node that most obviously has none.
- **A follower must never campaign.** Winning would leave it holding the writer
  role with nothing to write to, having taken it from a node that could have
  used it: the fenced-node failure from the other side. `leadership::follow` is
  its loop — it reads the lease so the `slate-leader` redirect keeps naming a
  node that exists, and never acquires. `leadership::maintain` is only for a
  node that has a writer.

**Promotion is a restart, and that is not a temporary state of affairs.** A
writer store is a database that has been opened, and it may only be opened
after the campaign is won. There is no way to grow one into a running process
without reintroducing exactly the ordering this design exists to preserve. A
supervisor restarting a read-only node is what promotes it; the node opens the
database at startup, after campaigning, like every other leader.

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
warning rather than refused. It bounds a **hash** build side and nothing else —
a nested loop has no build side — so lowering it protects a node only on the
plans the planner costed as hash joins. The field comment says so now; a
caller that must have the bound forces `hash_build` and takes the plan it asked
for.

### Checking a client's copy of the schema, without publishing one

`ColumnRef` removed the arithmetic *across* tables. It did not remove the
ordinal *within* one, and the first client written outside Rust said so: a
request still says "input 1's column 3", and a hand-written table declaring
`category` where the catalog has `kind` filters the wrong column, is accepted
because the ordinal is legitimate, and returns plausible rows. The derive macro
generates the Rust constants from the same declaration the server serves, so
Rust cannot disagree. Nothing else had an equivalent.

`SchemaCheck` is the equivalent: `columns`, and a 64-bit fingerprint over the
table's name, each ordinal's column name and type in order, and the ordinals
that form the primary key. It hangs off `Query` — so it covers every input of a
join, every explain and every aggregate — and off `Get`, `Insert`, `Update` and
`Delete`. It is optional; a request without one is served as before.

The traffic is one way, which is the whole of the distinction from the
`Describe` the `.proto` refuses. A client may *assert* what it believes and be
refused; nothing tells it what the answer is instead.

#### The design constraint is the migration, not the check

A fingerprint that broke on every migration would be worse than none, because
the second thing an operator does after it takes the fleet down is turn it off.
What makes one possible here is a property of the schema layer rather than of
the wire: **an ordinal never moves.** A column is appended; a dropped column
keeps its ordinal for ever and holds nothing; a rename records the previous
name and goes on resolving it. So every migration this project supports leaves
every existing reference naming the same column, and the check is built to
agree:

| migration | what the check does |
|---|---|
| a column added | the old client's declaration is a *prefix*, and the server hashes exactly that prefix. It keeps working across the deployment |
| a column dropped | the ordinal and the declaration are unchanged, so the fingerprint is unchanged. A client still *writing* it is refused by name |
| a column renamed | accepted under either name: the server knows the previous one, because the schema layer promises code written against it keeps working |
| `DEFAULT`, `CHECK`, foreign keys | not fingerprinted at all |

What it refuses is a declaration that is *wrong*: a column inserted in the
middle, two same-typed columns swapped, a name this column never answered to, a
key of the wrong shape. Those are the ones that otherwise return rows.

Nullability, constraints and indexes are left out because none of them
addresses a column — a write that violates one is refused by name, which is a
better error, and an index added for performance must not invalidate a client
that never names it. Table ids, index ids and schema versions are left out
because a client cannot state them, and a fingerprint a client cannot compute
is a constant it has to be told, which is the schema on the wire by another
route.

#### Why the client asserts rather than the server advertising

The cheaper design — the one the finding proposed — is one opaque fingerprint
on every response, compared by the client at startup: one field, no round trip.
It was rejected, and the reason is the migration table above. With one opaque
value a mismatch is *uninterpretable*: the client cannot tell "your declaration
is wrong" from "the server has one more column than when you were written", and
the only safe reaction to an uninterpretable mismatch is to refuse to start.
That is the additive migration taking down the fleet, arrived at from the other
direction.

The party that holds both statements is the server. So the claim travels to the
server, which can be exactly as tolerant as its own schema rules are, can say
which way the disagreement runs, and refuses *the request that would have been
wrong* rather than hoping somebody ran a startup check. The cost is a field on
five requests instead of one on a response, and nine bytes on the wire.

The hash is FNV-1a over a length-prefixed canonical form, specified in the
`.proto` in a paragraph. FNV rather than SHA-256 because every language has to
reimplement it byte for byte, and ten lines that can be checked by eye beat a
dependency; length-prefixed rather than delimited so that no column name can be
spelled to look like the end of a field. It is a check against drift, not
against an adversary — a client that wants to lie about its schema can simply
not send one. `schema_check.rs` pins the constants against a second
implementation written in Python, because agreeing with itself proves nothing.

### A computed value comes back beside the row, not inside it

`Row` carries `values` and `computed` as two lists. Concatenated, reading the
nth computed value meant `table_width + n` — the arithmetic `ColumnRef` exists
to remove, from a width this protocol does not publish, moving the day a column
is added to the table. `JoinedRow` and `Group` had already been split for that
reason and the single-table row had not, which made it an omission rather than
a trade-off. Because the split is on `Row` itself it applies inside a
`JoinedRow` too, where each input is split at *its own* table's width.

`computed` is empty on a row travelling the other way, and a request that sets
one is refused rather than having it dropped: an insert that appeared to accept
values it discarded is the same failure in the opposite direction.

### Explaining a grouped read is a different question

`Explain` and `ExplainJoin` describe the read as written. An `Aggregate` does
not run that read: grouping narrows each input's projection to the group keys
plus what the aggregates take out of the row, which is what lets an index
answer a `COUNT(*)` without a single row fetch. So the plan of the underlying
`Query` or `JoinQuery` is a plan the aggregate will not execute, and "will my
grouped read go index-only" had no way to be asked.

`ExplainAggregate` takes the whole `AggregateQuery` — the same message
`Aggregate` takes — rather than a grouping bolted onto `ExplainJoinRequest`.
That is what makes the answer honest by construction: the message explained is
the message that would run, and the one-table case, which narrows too, is
covered by the same RPC rather than left out.

Underneath, the narrowing lives in one function per shape, returning the
narrowed read beside its plan, and both the execution path and the explain path
call it. An `EXPLAIN` that narrowed separately could come to describe a plan
nothing runs — and the divergence would be invisible, since both halves would
remain plausible plans for plausible joins.

#### `decodes`, and why it had to exist

The first version of this shipped an RPC that could not show its own point.
Narrowing a projection changes what a plan *decodes* and nothing about how it
*reaches* rows, so wherever no index applies, the grouped and ungrouped plans
rendered as identical strings — access path, cost, row estimate, all the same.
`ExplainResponse` published only the access path.

`decodes` is the plan's output columns: the projection plus whatever the
residual reads, since those are decoded anyway. It is the field the difference
is visible in, and the field every client's test for this asserts against. The
residual's contribution is not incidental — on this engine the residual carries
the security filter, so `decodes` is also where a policy's cost shows up.

### Warnings belong to the request that caused them

An ignored index hint and a clamped build limit used to be reported by
`Explain` and by nothing else, so a caller whose hint did nothing had to issue a
*different* request and trust the planner had decided the same way on it — at a
different moment, possibly against a different view. `QueryResponse`,
`JoinResponse` and `AggregateResponse` carry `warnings` on their first message,
which is the message that is always sent even for an empty result because it
carries `served_by`. There was a header to put them in all along.

### The status code is lossy, so something structured travels with it

The mapping in `status.rs` is many-to-one and cannot be otherwise: `UNAVAILABLE`
is four kernel errors wanting four different responses, and `ALREADY_EXISTS` is
"that id is taken" and "that **email** is taken", where the second needs the
index name that `UniqueViolation` is already carrying. The distinction was
recoverable only from the message, and a message is not an interface — nothing
tests its wording, so a client that branches on it breaks when somebody improves
it.

Every status now also carries a `google.rpc.ErrorInfo` in
`grpc-status-details-bin`: a `reason` token per kernel variant, a `domain`, and
the variant's own payload as metadata (`table`, `index`, `action`, `replica`,
`required`, `visible`, `limit`). That is the standard shape, so a client using
its language's rich-error helper needs no special support, and one that ignores
details is unaffected.

One family of metadata keys is *specified* rather than incidental, and is the
only one clients parse: a check violation sends a `violations` count and
`check.N`, `column.N`, `message.N` indexed from zero. All three clients read it
into typed values — see `docs/validation.md` — because a refused form needs to
know which field, and every other key here varies per variant. The two well-known messages are declared in this
repository's own `proto/google/rpc/` rather than pulled in as a dependency —
they are twelve lines, the build is hermetic on purpose, and what has to agree
is the bytes rather than the crate.

`slate-leader` stays where it is. It predates this, clients read it, and a
redirect only a rich-error client could follow would be worse. The test that
matters is not that `UniqueViolation` says `UNIQUE_VIOLATION` — that is a rename
away from meaning nothing — but that no two errors behind one status code share
a reason.

### Two things the wire format refuses to guess

An identity never appears in a request body. The `.proto` has no principal
field anywhere; the `SecurityContext` is built from transport metadata by an
`Authenticator` the deployment supplies, and nothing in the crate can produce a
superuser.

An unset `oneof` is an error, not a default. proto3 cannot tell an unset field
from a zero one, so a `Value` with no kind set is most likely a client built
against a newer schema — reading it as null would quietly change the predicate
it appears in.

A `bool` inside a `oneof` breaks that rule from the inside, and three of them
did. `Freshness{latest: false}` selects the `latest` arm while meaning nothing
of the kind — it is what zeroing the struct produces — so the server read it
back as `ANY`, with the right reason (routing every zeroed read to the writer
would point the fleet at the scarcest resource in the deployment) and the wrong
outcome: a field that said "the writer" and meant "any replica". `AccessHint`
and `JoinAlgorithm` had the same shape. All three arms are now a one-value
`Unit` enum, which is the trick `NullValue` already played for "the client sent
a null" against "the client sent nothing": the false spelling is
unrepresentable rather than reinterpreted, and a client with nothing to say
leaves the message absent, which has always meant `ANY`. The old field numbers
are reserved — `false` on the old field would decode as `UNIT` on a new one and
select the arm it was trying not to select.

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
- **A second node that does not fence the first.** The same two `SlateStore`s
  shape, except that the second node opens no store at all: it loses the
  campaign, builds a `Head::read_only` over a `SlateReader`, serves a read that
  names the leader's own commit sequence, and refuses a write with the leader
  in the trailer. The assertion that carries the test is the last one — the
  **leader writes again** afterwards. A test that only checked the follower's
  reads would pass against the design this replaces, because a fenced leader's
  replica goes on answering perfectly well, and a read on the fenced node
  itself would not settle it either: the fence test above is the proof that a
  fenced SlateDB is still readable. Only a write distinguishes them. The same
  property is asserted a second time end to end, as two `slate-serverd`
  processes over one local directory.
- **That a read-only node refuses what it cannot serve**, three ways: a write
  (`UNAVAILABLE` with `slate-leader`), a `Begin` (there is no store to open a
  transaction on), and a `Freshness::Latest` read — the control that separates
  "no writer" from "a writer it is choosing not to use". Plus the unreachable
  case made legible rather than left to an `unwrap`: a read-only node that
  somehow holds the lease says which wiring mistake produced it.
- **That a following node never acquires.** `leadership::follow` is run against
  a real object-store lease under a controlled clock, across a handover it
  could twice have won, and must end with the lease still somebody else's and
  the redirect naming the *new* holder rather than the one it saw at startup.
  With the two states an observation must not overwrite asserted separately: it
  cannot demote a leader, and it cannot revive a node that has stepped down.
- **The lease against a store that cannot back one**, over a real directory
  through `LocalFileSystem` rather than a fake missing the method. Acquisition
  is refused with a distinct error naming the store and the remedy, no lease
  object is left behind claiming this node holds the database, the *second*
  node gets the same refusal rather than being told the lease is held — which
  would look like an election that worked — and a node pointed at such a store
  steps down permanently instead of campaigning forever. The control runs the
  same probe against a store that does support conditional updates and requires
  it to write nothing and disturb nothing.
- **The schema check, as migrations rather than as cases.** Each is two table
  definitions, v1 and v2, exactly as a schema evolving in a repository would
  be, with the claim computed from v1 and checked against v2: a column added
  (twice — nullable, and `NOT NULL` with a default), a column dropped, a column
  renamed and named under both spellings, a `CHECK`, an index and a `DEFAULT`
  added. Against them, the drifts that must be refused: a name this column
  never answered to, two same-typed columns swapped, a column inserted in the
  middle, a different key, a different type, a declaration wider than the
  table. The canonical form is pinned against a reference implementation
  written in Python and printed in the test, because a fingerprint that agrees
  only with itself is not a fingerprint — and a change to the form has to fail
  here rather than in somebody else's client after an upgrade.
- **That a malformed primary key is a bad request.** `found: false` is
  deliberately indistinguishable from "a row your policy hides", so it must not
  also mean "your key was malformed": the wrong arity, no values at all, the
  right arity with the wrong integer width, and a null are each refused by
  name, on `Get` and on `Delete`, inside a transaction and outside one — with
  the well-formed key for a row that is not there still reading as absent, as
  the control that keeps the rest of it meaningful.
- **That warnings reach the request that ran**, on all three streams, with the
  same request run again with nothing to complain about as the control, and
  with a join carrying two warnings from two different places in the conversion
  — an input's hint and the join's build limit — so that plumbing only one of
  them fails.
- **That no two errors behind one status code share a reason.** Asserted as a
  set rather than one by one: the property is that the details undo the
  collapse, not that any particular variant spells its token any particular
  way.

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
- **Promoting a read-only node without restarting it.** A writer store is a
  database that has been opened, and it may only be opened after the campaign
  is won — so a running follower cannot grow one without reintroducing the
  ordering that keeps it from fencing the leader. A supervisor restart is the
  promotion, and a read-only node deliberately does not campaign, so nothing
  triggers one automatically: an operator or an orchestrator decides. What
  would remove the restart is a `Head` whose writer can arrive at run time,
  which is a lifecycle this crate does not have and should not grow casually —
  every request handler currently gets to assume the store it has is the store
  it started with.
- **A lease over a store with no conditional update, in the library.** The
  create-only design that would work — the generation in the object's name, as
  SlateDB's own manifest does it — is specified and argued against in
  `slate-serverd`'s `filelease.rs`. Over a local directory `slate-serverd` uses
  a `flock` instead, and `ObjectStoreLease` refuses rather than degrading.
- **`delete_many`.** `insert_many` and `update_many` overlap their reads across
  a batch; a multi-row delete still costs a round trip per key, because a
  delete walks a foreign-key closure and two keys in one batch can reach the
  same doomed row by different paths. Batching it means unioning those closures
  before writing anything, which is a different change from the other two.
- **A batched `Get`.** `Get` takes one key where the three writes take
  `repeated`, and the asymmetry is deliberate rather than an oversight. The
  batched read already exists and is a `Query` with `IN` over the primary key,
  which the planner turns into the point reads it actually is, issued together
  — that overlap is the whole win, and it is what makes `insert_many` worth
  having. `repeated Row primary_keys` could not reproduce it: `Expr::In` names
  one column, so the overlap is available for a single-column key and not for a
  composite one, and the server would fall back to a loop. The result would be
  a batch that costs one round trip on some tables and fifty on others with
  nothing in the response to say which — which is a worse thing to have on a
  wire than an asymmetry. The `.proto` says all of this under `GetRequest`,
  which is the actual gap: the second spelling existed and was not written
  down.
- **A write by predicate.** `DELETE FROM docs WHERE …` is a query followed by a
  batch delete, in one transaction, and `UPDATE … SET size = size + 1` cannot
  be said at all. Both would be new: the second needs the expression evaluator
  applied to a write, which is a kernel change, and the first needs a write
  that walks a cursor, which the head node would have to implement over rows it
  had already streamed — the same objection that keeps a grouped join out. What
  was missing was the sentence saying so, and `UpdateRequest` now carries it: an
  update is a row replacement, there is no predicate form, and here is what to
  write instead.
- **The head node under concurrency.** It is measured now — see
  `docs/performance.md` — but every measurement is one request at a time, so
  the per-stream channel and the task-per-transaction design have never been
  under pressure.
