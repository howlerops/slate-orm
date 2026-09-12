# Performance

Measured, not guessed. Everything below came from `cargo bench` and
`cargo run --release -p slate-kernel --example perf_report` on a 4-core Xeon at
2.8 GHz; the absolute numbers will differ on your machine, the ratios should
not.

## How to reproduce

```sh
cargo bench                                                   # criterion
cargo run --release -p slate-kernel --example perf_report     # I/O counts
```

Two profiles are used. `free` charges nothing and isolates CPU. `io` charges
object-storage round trips — a millisecond per point read, one per scan and one
per 256-row block. Prefer the **I/O counts** over the clock: "this removed 500
point reads" survives a different machine, provider and timer; a wall-clock
figure from a simulated profile survives none of them.

Charges are at least a millisecond deliberately. The runtime timer has roughly
millisecond granularity, so a profile built from microseconds measures the timer
rather than the workload — the first version of this harness did exactly that
and reported a 20µs read as 1.3ms.

## Baseline

Recorded before any optimisation work, on 10,000 rows across 4 tenants, with
three secondary indexes.

### Query shapes, at object-storage cost

| query | rows | point reads | scans | wall | plan chosen |
|---|---:|---:|---:|---:|---|
| point get by primary key | 1 | 0 | 1 | 16 ms | table scan |
| whole tenant | 2500 | 0 | 1 | 36 ms | table scan |
| indexed equality | 100 | **100** | 1 | 231 ms | index `by_kind` |
| indexed equality, limit 10 | 10 | 10 | 1 | 43 ms | index `by_kind` |
| indexed range | 500 | **500** | 1 | **1099 ms** | index `by_at_desc` |
| unindexed filter | 0 | 0 | 1 | 37 ms | table scan |
| insert 1 row (3 indexes) | 1 | 1 | 0 | 6 ms | — |
| insert 100 rows, one txn | 100 | 100 | 0 | 221 ms | — |

### CPU only

| operation | time |
|---|---:|
| encode a 2-column primary key | 63 ns |
| the same, into a reused buffer | **19 ns** |
| decode a 2-column primary key | 61 ns |
| encode 6 columns | 176 ns |
| decode 6 columns | 384 ns |
| build a row key | 148 ns |
| plan a 3-term predicate | **1.04 µs** |
| point get, end to end | 830 ns |
| scan 2500 rows | 1.60 ms (640 ns/row) |
| index scan, 100 rows | 134 µs (1.34 µs/row) |
| insert 1 row, 3 indexes | 8.7 µs |

## What the baseline said, and what was done about it

Every line below is a change the profile asked for, not one that seemed like a
good idea.

### An index scan cost one point read per row

Returning 100 rows through an index cost 231 ms while scanning the whole
2500-row tenant cost 36 ms — the index 6× slower while reading 25× less data.

Two things fixed it. **Index-only scans**: a query says which columns it needs,
and when the index holds them the row is assembled from the index entry and
never read. **Pipelining**: when the row must be read, the reads are overlapped
rather than done one at a time.

| | before | after |
|---|---|---|
| indexed equality, 100 rows | 100 reads, 218 ms | 0 reads, 2.3 ms |
| count over an index | 500 reads, 1080 ms | 0 reads, 4.8 ms |
| non-covering index scan, 60 rows | 128 ms | 10.8 ms |

### The planner had no idea any of that was true

It preferred whichever index matched the most equality terms. On a 500-row
range that was 30× worse than ignoring the index. Structure cannot tell you how
many rows a predicate selects, so the planner now costs plans against
statistics gathered by `analyze`.

| query | before | after |
|---|---:|---:|
| indexed equality, 100 rows | 218 ms | 23 ms |
| indexed range, 500 rows | 1099 ms | 24 ms |
| indexed equality, limit 10 | 43 ms | 3.0 ms |

Across the read workload: **1,111 point reads became 1**.

### A key lookup opened a scan

Now a point read: 16 ms to 2.3 ms.

### Key encoding was mostly allocation

Composing a key from a prefix plus a separately encoded tuple allocated twice
and copied once, on every read and every write. Building it into one buffer took
it from 148 ns to 30 ns, and a point get 19% faster end to end.

### Planning cost more than a point lookup

It did before this work (1.04 µs against 830 ns), and the cost model made it
worse before it made it better — 1.87 µs at its peak, now 1.27 µs after sharing
the residual instead of deep-cloning it, borrowing the query's literals instead
of copying them, and not building a set of every column per index to answer a
question with a known answer.

Still above where it started. That is the right trade: the model it pays for is
what turned a 1.1-second query into 23 ms.

### Every insert did a read, and waited for it

The duplicate-key check costs a round trip per row. A hundred-row load spent a
hundred of them, one after another, before writing anything: 222 ms to insert
100 rows.

The obvious fix was to drop the check and let the write-write conflict catch a
genuine duplicate. That was not needed. The reads are not the problem — waiting
for each one is. `insert_many` and `upsert_many` issue them concurrently
(`BULK_READ_CONCURRENCY = 32`), batch the unique-index probes the same way, and
then write. The same number of reads, in a fraction of the wall time:

| | before | after |
|---|---:|---:|
| insert 100 rows | 222 ms, 100 reads | 13 ms, 100 reads |
| insert 1000 rows | — | 82 ms, 1000 reads |

Duplicate detection is not weakened by this: a caller still gets
`DuplicatePrimaryKey` or `UniqueViolation` naming the index, not a conflict
error at commit. Validation and intra-batch collision detection run first, so a
batch that cannot be written spends no I/O at all.

### Joins, and a cost model that has to be right in both directions

Everything was single-table. Adding joins meant choosing between a hash join —
two scans, no probes — and a nested loop, one probe per outer row. The cost
model's constants decide it: a scanned row is a hundredth of a round trip and a
probe is at least one, so the loop wins only when the outer side is smaller than
roughly a hundredth of the inner one.

That is a strong claim in both directions, so both were measured, with each
algorithm forced against the case it should lose:

| | rows read | wall | chosen |
|---|---:|---:|---|
| 500 actors ⋈ 2500 events, hash | 3,000 scanned | 32 ms | yes |
| the same, forced to a loop | 3,000 scanned + 2,500 point reads | 153 ms | |
| one actor's events, loop | 5 scanned + 6 point reads | 6.7 ms | yes |
| the same, forced to a hash | 2,500 scanned | 30 ms | |

Hash wins the general case by 4.7×, the loop wins the single-outer-row case by
4.4×, and the planner picks correctly in both. The second case is not a corner:
"load one record, then its children" is what an ORM does all day.

Two things fall out of the constants rather than being tuned. Probes are
overlapped sixteen at a time — issued serially, a loop's latency would be the
sum of its probes, which is the mistake the index scan and the bulk write path
each had to be talked out of. And the hash build side is bounded, because a join
condition that does not relate the two tables is otherwise a way to be killed by
the allocator rather than told what is wrong.

### Chains cost what their steps cost

A chain of three tables runs as repeated two-table steps, each choosing its own
algorithm. Two things were worth checking rather than assuming.

The machinery is not a tax: the same two-table join expressed as a one-step
chain came out at 32.7 ms against the specialised path's 32.1 ms, with the same
reads and the same algorithm chosen. And a chain that starts from one row does
not automatically get cheap — `one team → actors → events` is 32 ms against the
whole-table version's 36 ms, because the last step scans every event either
way. The step counts say why at a glance: `[1, 50, 250]`. That number is
reported by the cursor rather than only estimated, because a step that was
predicted at ten rows and produced ten thousand is the usual reason a chain is
slow, and it is invisible otherwise.

### The planner was picking the slower plan, and the harness hid it

The cost model charged one round trip per point read. The executor has
pipelined those since the index-scan work — sixteen in flight — so an index
scan was overcharged sixteenfold, and the planner chose a table scan where the
index was measurably faster:

| | wall | model said |
|---|---:|---:|
| indexed equality, 100 rows, table scan | 57 ms | cost 26 ← chosen |
| the same, forced through the index | **20 ms** | cost 102 |

The first attempt at a fix was wrong, and the way it was wrong is the useful
part. Dividing the measured wall time by the number of reads gave an apparent
speedup of 6.6x, so the read charge was divided by six. But that figure came
from comparing against a 1 ms round trip when the fixture's is 2.2 ms — the
timer's resolution, not the model's. Probing the fixture directly settled it:
**any number of concurrent reads complete in the time of one.**

```
   1 gets at once   2.23 ms      16 gets at once   2.17 ms
   4 gets at once   2.18 ms      64 gets at once   2.25 ms
```

So the shape is waves, not a discount: `n` reads at depth `d` cost
`ceil(n / d)` round trips. That predicts 7 waves for 100 reads and 32 for 500,
and both matched the measurement exactly. It also explains why a small limit
gains least — one read and sixteen both cost one wave — and the executor now
caps its prefetch at the window rather than fetching sixteen rows to return
ten.

Then the harness itself, for the fourth time. With the wave model in place a
`LIMIT 10` still measured worse through the index than the model said it
should. Neither the model nor the engine was wrong: `SCAN_ROW_COST` of 0.01
says a block holds a hundred rows, and the latency fixture was charging one
block per 256. Every scan comparison had been confounded by that. Making them
state the same number reconciled the two:

| | model | measured |
|---|---:|---:|
| indexed equality, table scan | 26.0 | 57 ms |
| indexed equality, index scan | 9.0 | 20 ms |
| `LIMIT 10`, table scan | 3.5 | 7.3 ms |
| `LIMIT 10`, index scan | 2.1 | 4.4 ms |

Cost times 2.2 ms is the wall time, to within the timer, on every row. The
model went from ranking two of these backwards to predicting all of them.

The crossover moved with it: a non-covering index scan is now worth its
lookups up to roughly **6%** of the table rather than 1%. Covering an index
still matters far more here than on local disk — overlapping a round trip
makes it cheaper, not making it at all is still free.

### An `IN` over the key was a table scan

Loading ten records by primary key scanned all 2500 rows to find them, because
`IN` was only ever a residual filter. It is a set of reads, and the executor
already knows how to overlap those:

| keys | before | after | model | measured per unit cost |
|---:|---:|---:|---:|---:|
| 10 | 58.8 ms | 2.3 ms | 1.0 | 2.30 ms |
| 50 | 59.3 ms | 8.1 ms | 4.0 | 2.03 ms |
| 200 | 59.7 ms | 30.1 ms | 13.0 | 2.31 ms |

Twenty-six times faster at ten keys, and the estimate tracks the clock to
within a few percent at every size — which is the wave model from the previous
section paying off a second time.

The mistake worth recording: the first version returned the point-get set
*instead of* the primary key's scan rather than alongside it. Four hundred keys
cost twenty-five waves against the scan's three, so with the scan withdrawn the
planner had only the reads and an unrelated index to choose between, and picked
the index. A cheaper plan that is sometimes not cheaper has to be a candidate,
not a substitution. The test that caught it asserts a large set goes back to
scanning.

### The planner could not tell a narrow range from a broad one

`range_selectivity` was a flat third, ignoring the literal, so `at < 10` and
`at < 500` both costed 61.2. On a range selecting 0.4% of the corpus the
planner scanned all 2500 rows:

| | wall | model said |
|---|---:|---:|
| range over 0.4% of rows, table scan | 57.9 ms | cost 26 ← chosen |
| the same, forced through the index | **4.5 ms** | cost 61.2 |

`analyze` now builds an equi-depth histogram per column — buckets of equal
population, so a column with a long tail spends its resolution where the rows
are — from a reservoir sample. Reservoir rather than the first ten thousand
rows, because a scan arrives in key order and any column correlated with the
key would otherwise be described by one end of its own range. The generator is
deterministic and unseeded, so analysing the same data twice gives the same
statistics: a planner whose choices move between runs is one nobody can reason
about.

| | before | after |
|---|---:|---:|
| narrow range, chosen plan | Table Scan, 57.9 ms | **Index Scan, 4.5 ms** |
| narrow range, model cost | 61.2 | 3.2 |
| broad range, model cost | 61.2 | 36.9 |

Twelve times faster on the narrow one, and the broad one still correctly
scans — the model can now tell them apart at all, which it could not before.
The broad range's corrected estimate of 36.9 predicts 83 ms against 83 ms
measured.

Resolution is the bucket and no further. Interpolating inside one would need
arithmetic on `Value`, which is a closed type holding strings and uuids as
well as numbers, and sixty-four buckets already resolves to about 1.5% against
a crossover near 6%.

### SlateDB's readahead ships off, and we were passing the default

Every measurement above runs against a latency *model*. This one does not.
`examples/scan_tuning.rs` starts an S3 server in the process, writes a table,
reopens the database so the data is genuinely in object storage rather than a
memtable, and scans it.

SlateDB's `ScanOptions` defaults to `read_ahead_bytes: 1` and
`max_fetch_tasks: 1` — one block per request, one request at a time. That is
the right default for a library that cannot know its caller's access pattern.
A record layer does know: a table scan reads forward, from the first block to
the last. We were passing the default anyway.

20,000 rows, measured in both directions so a warming server cannot be
mistaken for a faster plan:

| setting | S3 GETs | forward | reversed |
|---|---:|---:|---:|
| SlateDB defaults (what we passed) | 1372 | 51 915 ms | 51 929 ms |
| 64 KiB, one task | 109 | 256 ms | 380 ms |
| 1 MiB, one task | 44 | 329 ms | 291 ms |
| **1 MiB, four tasks** (now the default) | **44** | **127 ms** | **121 ms** |
| 1 MiB, eight tasks | 40 | 131 ms | 86 ms |

**Thirty-one times fewer object-store requests.** That is the number worth
quoting: the wall-clock ratios here run from 158x to 409x depending on which
pair you compare, because this server's per-request cost is its own, but 1372
requests against 44 is arithmetic.

The two levers do different things. Readahead removes requests — that is the
31x. Concurrency then halves the time again at the *same* request count, which
is latency overlap rather than less work. Eight tasks is not reliably better
than four, so four is the default.

Nothing here was built. The capability was already in SlateDB and we were
declining it, which is worth saying plainly: the first thing to check before
writing an optimisation is whether the layer below already has one.

One thing this measurement implies and the model does not yet know: with
readahead on, 20,000 rows arrive in 44 requests, so a scanned row costs about
a fifth of the `SCAN_ROW_COST` of 0.01 the planner assumes. That constant is a
property of the deployment — row size, block size, readahead — rather than a
universal, and eventually it should come from the deployment. It is recorded
here rather than retuned, because retuning it against one synthetic corpus of
220-byte rows would be fitting to this fixture the way the block-size mismatch
above already caught us once.

### ClickBench found a bug in `ORDER BY`, and then found where the time went

Running an adversarial workload — see [clickbench.md](clickbench.md) — turned
up a correctness bug first. **A column the sort orders by was never decoded**
unless the projection or the predicate already named it. An undecoded column
reads back as null, so every row compared equal and the sort silently did
nothing, returning rows in whatever order the scan produced. The same hole let
an index-only scan be chosen over an index that does not hold the sort column.

Then the performance question, where the first answer was wrong. `SELECT
SearchPhrase … ORDER BY EventTime LIMIT 10` took 4.5-5.5 s against a 0.92 s
plain scan, and the obvious suspect was the sort: the executor collected every
matching row, sorted it, and returned ten. A bounded heap fixed exactly that
and moved the clock by almost nothing — 47.2 s to 44.9 s over the whole set.

The cost was `Projection::All`: decoding all 105 columns of every surviving row
to return one of them. Naming the column took Q25 from 4.29 s to 1.53 s, and
the set from 44.9 s to 36.4 s.

Both changes stay, for different reasons. The projection is the time. The
bounded heap is the memory — sorting to return ten rows used to hold every
surviving row decoded, which on a wide table is gigabytes to produce a handful.
And the projection could only be applied *because* the sort column now gets
decoded: the correctness bug was hiding the performance one.

### Grouping paid for an order nobody had asked for yet

Groups accumulated into a `BTreeMap`, so a result came out ordered by key with
no extra step. That was worth having until ClickBench measured it: Q33 groups
`(WatchID, ClientIP)` with no filter, which on that corpus is exactly a million
groups — one per row — and cost 6.53 s against 1.73 s for the same query with a
filter that cut the keys down. The ordering was being paid on every insert, as
O(log k) comparisons of a `Vec<Value>`.

Hashing the encoded key and sorting once at the end took it to **4.74 s**. The
order callers see is unchanged, because the encoding sorts as the values do —
the property the whole keyspace already rests on. What is left is allocation: a
million groups is a million key vectors and a million accumulator pairs.

### Computing a value per row, quadratically

Scalar expressions landed so the last eight ClickBench queries could run, and
one of them measured the implementation immediately. Q30 is ninety
`SUM(ResolutionWidth + n)` over ninety computed columns, and it took **90.85 s**
against about a second for a plain scan.

Appending a computed value has to evaluate it against the row *as it stands*,
so a later expression can read an earlier one. The first version did that by
rebuilding a `Row` each time — which clones every value once per computed
column. At ninety columns on a hundred-and-five-column table that is roughly
thirteen thousand value clones per row, a million times over.

Evaluating against the values as a plain slice instead took Q30 to **2.95 s**,
and the forty-two-query set from 167.7 s to 75.2 s. The fix was to let the
evaluator read a `[Value]` rather than insisting on a `Row`.

## Current numbers

Wall times below are higher than earlier revisions of this document because
the fixture now charges a block per hundred rows rather than per 256, matching
what the cost model believes. The engine did not get slower; the measurement
got honest.

| query | rows | point reads | wall | plan |
|---|---:|---:|---:|---|
| point get by primary key | 1 | 1 | 2.3 ms | Point Get |
| whole tenant | 2500 | 0 | 58 ms | Table Scan |
| indexed equality | 100 | 100 | 20 ms | Index Scan |
| indexed equality, limit 10 | 10 | 10 | 4.4 ms | Index Scan |
| indexed range | 500 | 0 | 58 ms | Table Scan |
| covered equality, keys only | 100 | 0 | 4.5 ms | Index Only Scan |
| covered count | 500 | 0 | 13 ms | Index Only Scan |
| 10 keys by primary key | 10 | 10 | 2.3 ms | Point Gets |
| 200 keys by primary key | 200 | 200 | 30 ms | Point Gets |
| range over 0.4% of rows | 10 | 10 | 4.5 ms | Index Scan |

| join | rows out | point reads | scanned | wall | plan |
|---|---:|---:|---:|---:|---|
| every actor to their events | 2500 | 0 | 3000 | 76 ms | Hash |
| one actor's events | 5 | 6 | 5 | 6.6 ms | Nested Loop |
| every team → actors → events | 2500 | 0 | 3010 | 80 ms | hash + hash |
| one team → actors → events | 250 | 1 | 3000 | 75 ms | hash + hash |

| write | rows | point reads | wall |
|---|---:|---:|---:|
| insert 1 row | 1 | 1 | 5.4 ms |
| insert 100 rows, one at a time | 100 | 100 | 223 ms |
| insert 100 rows, batched | 100 | 100 | 13 ms |
| insert 1000 rows, batched | 1000 | 1000 | 82 ms |

| operation | before | now |
|---|---:|---:|
| build a row key | 148 ns | 30 ns |
| point get, end to end | 830 ns | 690 ns |
| plan a 3-term predicate | 1.04 µs | 1.27 µs |
| scan 2500 rows | 1.60 ms | 1.51 ms |

## Two results worth keeping

**A non-covering index scan needs to select under about 6% of the rows a scan
would touch to be worth using.** That falls out of the cost ratio — a scanned
row is a hundredth of a round trip, and sixteen overlapped lookups are one — 
and it is why the plans above split the way they do. It is not a quirk of the
constants; it is what storage where every lookup is a network round trip
implies once you are allowed to make sixteen at a time. Covering an index still
matters far more here than on local disk: overlapping a round trip makes it
cheaper, not making it at all is free.

**A limit cannot change which plan wins; `ORDER BY` with a limit can.** Reading
`L` rows through an index costs `L` point reads, and finding `L` matches by
scanning costs `L / selectivity` rows: both linear in `L`, so their ratio is
whatever it was unlimited. A sort is different, because it must see every
matching row before returning the first — so an ordered index that streams beats
a sort that cannot stop early, and that flip is tested.

## Fixture notes

Two things in the harness were wrong before the numbers above were trustworthy,
and both flattered or distorted the results:

- `MemoryStore` copied the whole map on every `begin`, so any benchmark above a
  few hundred rows measured the copy. Snapshots are now shared by pointer.
- `MemoryStore::scan` copied the whole map and then filtered, making every scan
  O(database). Ranging the map directly took a 2500-row tenant scan from 14.5 ms
  to 1.6 ms — a 9× difference that was entirely fixture.

- `LatencyProfile` charged one block per 256 rows while the cost model's
  `SCAN_ROW_COST` said a hundred. Nothing was wrong with either number on its
  own; holding both at once meant every measurement of a scan against an
  estimate was comparing two different beliefs.

It is worth stating plainly that three of the first four "findings" from this
harness were bugs in the harness. `examples/concurrency_probe.rs` exists
because of the last one: when a plan measures worse than it costs, ask the
fixture what it is actually charging before changing the planner.
