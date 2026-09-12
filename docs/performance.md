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

## Current numbers

| query | rows | point reads | wall | plan |
|---|---:|---:|---:|---|
| point get by primary key | 1 | 1 | 2.3 ms | Point Get |
| whole tenant | 2500 | 0 | 24 ms | Table Scan |
| indexed equality | 100 | 0 | 24 ms | Table Scan |
| indexed equality, limit 10 | 10 | 0 | 3.0 ms | Table Scan |
| indexed range | 500 | 0 | 24 ms | Table Scan |
| covered equality, keys only | 100 | 0 | 2.3 ms | Index Only Scan |
| covered count | 500 | 0 | 4.8 ms | Index Only Scan |

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

**A non-covering index scan needs to select under about 1% of the rows a scan
would touch to be worth using.** That falls out of the cost ratio — a point read
is a round trip, a scanned row is a hundredth of one — and it is why so many
plans above are table scans. It is not a quirk of the constants; it is what
storage where every lookup is a network round trip implies, and it is why
covering an index matters far more here than on local disk.

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

It is worth stating plainly that the first two "findings" from this harness were
both bugs in the harness.
