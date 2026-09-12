# ClickBench, and the pgrust comparison

## What was asked, and what is actually possible

The ask was to review [pgrust](https://github.com/malisper/pgrust), replicate its
benchmark data here, and compare the queries. The first and third are done. The
second is not possible, and the reason matters more than the attempt would have.

## pgrust, as it describes itself

A reimplementation of PostgreSQL in Rust, built with AI assistance — the
README's own title is "rebuilding Postgres in Rust with AI". It claims wire and
SQL-dialect compatibility and passing all 46,066 tests in Postgres' regression
suite. Architecturally it is a vectorised, JIT-compiled push-based executor,
thread-based rather than process-based, with a query scheduler, an OOM killer,
and a column-oriented storage format.

Its published numbers:

- **ClickBench combined score: 18.5% faster than ClickHouse**, and "hundreds of
  times faster" than PostgreSQL.
- **sysbench `oltp_read_only`: 30% higher throughput than Postgres 18.3** at
  300 GB.

Its own stated caveats are worth repeating, because they are unusually
forthcoming:

- The numbers come from builds tuned for Graviton4 (`-Ctarget-cpu=neoverse-v2`),
  and **the JIT only targets Graviton4**. Published binaries will not reproduce
  them.
- They *revised a claim downward*: "We had previously reported that pgrust was
  over 50% faster than Postgres… we have not isolated why the same binaries
  behave differently [on Kubernetes] than on bare EC2, so we quote the lower
  number."
- "pgrust is not production ready. Do not put data you care about in it."

The methodology in `benchmarks/README.md` is better than most vendor
benchmarking: per query, stop the server, drop the OS page cache, restart, three
tries with cold and hot reported separately; both OLTP arms on identical configs
with `fsync` and `synchronous_commit` left at Postgres' defaults; binary SHA256
recorded with every result; and the kit handed to an outside auditor with sole
control of the machines.

None of that is verified here. It is a fair account of what the project claims
and how it says it measured — no more.

## Why the numbers cannot be replicated

Three independent reasons, any one of which is sufficient:

1. **Their raw results are not published.** `benchmarks/` holds the harness and
   the 43 queries; results are generated into a directory at run time and not
   checked in. There is nothing to diff against.
2. **The hardware is part of the claim.** c8g.4xlarge Graviton4, with a JIT that
   targets only that chip. This runs in a container on shared x86.
3. **The scale is 100M rows and 300 GB.** One ClickBench partition — a hundredth
   of the dataset — is what fits here.

## Why a head-to-head would be dishonest anyway

pgrust is a column store with a vectorised JIT executor across 16 vCPUs.
This is a row store with a row-at-a-time executor, single-threaded per query,
whose cost model counts object-storage round trips. ClickBench is an analytics
benchmark: one wide table, forty-three scan-and-aggregate queries, no joins.

Those are different classes of system answering different questions. Putting the
numbers side by side would produce a ratio that means nothing.

## What was done instead

The **benchmark** is public and standard even though their results are not. So
the real ClickBench schema and the real 43 queries were run against this engine,
at the scale that fits, and reported on their own terms.

That is worth doing for a reason unrelated to pgrust: ClickBench is
*adversarial*. It is a standard, public, unsympathetic workload nobody here
designed for, which makes it a far better source of findings than another
benchmark written by whoever wrote the engine.

- `crates/slate-clickbench` loads a ClickBench parquet partition (1,000,000
  rows, all 105 columns) and runs every query the engine can express.
- All 105 columns are loaded, not just the ones the queries read. Loading a
  subset would flatter a row store, since reading columns nobody asked for is
  precisely the cost a row store pays and a column store does not.
- The primary key is ClickHouse's own `ORDER BY` for this dataset —
  `(CounterID, EventDate, UserID, EventTime, WatchID)` — with a row ordinal
  appended to make it unique. That is the fairest mapping onto a key-ordered
  store.
- In memory, not on object storage. ClickBench measures a query engine; running
  it over S3 would measure SlateDB's block fetching instead, which
  `slate-slatedb`'s `scan_tuning` example measures separately and honestly.

## Results

1,000,000 rows, all 105 columns, in memory, in a container. **These are not
comparable to any published ClickBench score** — those are 100M rows on
dedicated hardware — and lining them up beside one would be dishonest.

**35 of 43 queries run.** The eight that do not all need the same missing
thing: an *expression*. This language has predicates over columns, not scalars
computed from them, so `length(URL)`, `EventTime`'s minute, `ClientIP - 1`,
`DATE_TRUNC`, `REGEXP_REPLACE` and `CASE WHEN` have nowhere to be written. That
is one feature, not eight.

| Q | wall | scanned | answer |
|---:|---:|---:|---|
| 1 `COUNT(*)` | 1.09 s | 1,000,000 | 1000000 |
| 2 `COUNT(*) WHERE AdvEngineID <> 0` | 1.51 s | 1,000,000 | 14174 |
| 3 `SUM/COUNT/AVG` | 1.41 s | 1,000,000 | 80778 / 1000000 / 1604.09 |
| 4 `AVG(UserID)` | 0.96 s | 1,000,000 | 1.948e18 |
| 5 `COUNT(DISTINCT UserID)` | 1.08 s | 1,000,000 | 79842 |
| 6 `COUNT(DISTINCT SearchPhrase)` | 1.66 s | 1,000,000 | 18316 |
| 7 `MIN/MAX(EventDate)` | 1.02 s | 1,000,000 | 15901, 15901 |
| 8 group by AdvEngineID | 1.30 s | 1,000,000 | 5 groups |
| 9 distinct users per region | 1.60 s | 1,000,000 | 1242 groups |
| 10 five aggregates per region | 1.41 s | 1,000,000 | 1242 groups |
| 11 distinct users per phone model | 1.44 s | 1,000,000 | 30 groups |
| 12 …per (phone, model) | 1.33 s | 1,000,000 | 59 groups |
| 13 group by SearchPhrase | 1.46 s | 1,000,000 | 18315 groups |
| 14 distinct users per SearchPhrase | 1.51 s | 1,000,000 | 18315 groups |
| 15 group by (engine, phrase) | 1.47 s | 1,000,000 | 19300 groups |
| 16 group by UserID | 1.10 s | 1,000,000 | 79842 groups |
| 17 group by (UserID, phrase) | 1.62 s | 1,000,000 | 98484 groups |
| 18 same, unordered | 1.59 s | 1,000,000 | 98484 groups |
| 20 `WHERE UserID = …` | **0.01 s** | 0 | — |
| 21 `COUNT(*) WHERE URL LIKE '%google%'` | 2.47 s | 1,000,000 | 95 |
| 22 group by phrase, URL matched | 2.51 s | 1,000,000 | 1 group |
| 23 two `LIKE`s and a distinct count | 3.06 s | 1,000,000 | 53 groups |
| 24 `SELECT *` … order by, limit 10 | 2.58 s | 1,000,000 | 10 rows |
| 25 order by EventTime limit 10 | 1.58 s | 1,000,000 | 10 rows |
| 26 order by SearchPhrase limit 10 | 1.52 s | 1,000,000 | 10 rows |
| 27 order by two columns limit 10 | 1.44 s | 1,000,000 | 10 rows |
| 31 group by (engine, IP) | 2.01 s | 1,000,000 | 22830 groups |
| 32 group by (WatchID, IP), filtered | 2.08 s | 1,000,000 | 69354 groups |
| 33 …unfiltered | **4.74 s** | 1,000,000 | **1,000,000 groups** |
| 34 group by URL | 3.09 s | 1,000,000 | 275494 groups |
| 37 URL page views, July 2013 | 1.15 s | **413,825** | 171171 groups |
| 38 Title page views | 0.78 s | **413,825** | 26185 groups |
| 39 with `OFFSET 1000` | 0.61 s | **413,825** | 7385 groups |
| 41 with `IN (-1, 6)` | 0.68 s | **413,825** | 23599 groups |
| 42 with `OFFSET 10000` | 0.65 s | **413,825** | 7006 groups |
| | **55.55 s** | | 35 of 43 |

### The answers are right, not just fast

A benchmark that reports only timings cannot be checked, and a wrong answer
produced quickly is the easiest result to get. Every answer that can be
computed independently was, with `pyarrow` over the same parquet:

| | slate-orm | pyarrow |
|---|---:|---:|
| `COUNT(*)` | 1000000 | 1000000 |
| `COUNT(*) WHERE AdvEngineID <> 0` | 14174 | 14174 |
| `COUNT(DISTINCT UserID)` | 79842 | 79842 |
| `COUNT(DISTINCT SearchPhrase)` | 18316 | 18316 |
| `MIN/MAX(EventDate)` | 15901, 15901 | 15901, 15901 |
| `COUNT(*) WHERE URL LIKE '%google%'` | 95 | 95 |

The results are also consistent with each other in a way that would be hard to
fake: Q16 finds 79,842 distinct `UserID` groups, matching Q5's distinct count
exactly; Q13 finds 18,315 `SearchPhrase` groups, which is Q6's 18,316 minus the
empty string Q13 filters out.

## What it found

### The key design works, exactly

Q37–42 filter `CounterID = 62 AND EventDate BETWEEN '2013-07-01' AND
'2013-07-31'`. `CounterID` leads the primary key, so those become a range.
They scanned **413,825 rows of 1,000,000** — and checking the parquet
directly, `CounterID = 62` has exactly 413,825 rows. Not approximately: the
range is precisely the matching rows, with no over-scan.

### A correctness bug, in `ORDER BY`

`SELECT SearchPhrase … ORDER BY EventTime` is an ordinary shape, and it
exposed one: **a column the sort orders by was never decoded unless the
projection or the predicate already named it.** An undecoded column reads back
as null, so every row compared equal and the sort silently did nothing —
returning rows in whatever order the scan produced, which looks like an order.

The same hole let an index-only scan be chosen over an index that does not hold
the sort column. Both are fixed, and the fix is what made the next paragraph
safe to do.

### The cost was decoding, not sorting

Q25–27 took 4.5–5.5 s against a 0.92 s plain scan. The obvious suspect was the
sort: the executor collected every matching row, sorted it, and returned ten. A
bounded heap fixed that — and moved the clock by almost nothing, 47.2 s to
44.9 s overall.

The actual cost was `Projection::All`: the executor decoded all 105 columns of
every surviving row to return one of them. Asking for the column the query
selects took Q25 from 4.29 s to **1.53 s**.

Both changes were worth keeping, for different reasons. The projection is the
time. The bounded heap is the memory: sorting to return ten rows used to hold
every surviving row decoded, which on a 105-column table is gigabytes to
produce a handful. And the projection could only be applied *because* the sort
column now gets decoded — the correctness bug was hiding the performance one.

### Grouping on a high-cardinality key is still the slowest thing here

Q33 groups by `(WatchID, ClientIP)` with no filter, and the answer column says
why it is slow: **exactly 1,000,000 groups**, one per row. It was 6.53 s when
groups accumulated into a `BTreeMap` for ordering, paying O(log k) comparisons
of a `Vec<Value>` on every row. Hashing the encoded key and sorting once at the
end took it to **4.74 s**, and the order callers see is unchanged — the
encoding sorts as the values do, which is the property the whole keyspace rests
on.

The remaining 4.74 s is allocation: a million groups means a million key
vectors and a million accumulator pairs. That is the next thing here, and it is
not done.

### What the second pass added

`COUNT(DISTINCT)` and `LIKE` between them turned eleven of the nineteen
unsupported queries into supported ones. Neither is exotic; both were simply
missing.

`COUNT(DISTINCT)` is exact and counts over the *encoded* value, so two rows
count as one exactly when they would collide in an index — the same definition
of equality the rest of the layer uses. Exactness costs memory proportional to
the distinct count; an approximate counter is a different aggregate, not a
cheaper version of this one.

`LIKE` matches iteratively with backtracking rather than recursively, because a
pattern is caller input and a recursive matcher on `%a%a%a%…` is a stack
overflow waiting to be sent. A pattern anchored at the front is a key range
rather than a filter: every value starting with `abc` encodes to something
beginning with the encoding of `abc` minus its terminator, so the matches are
one contiguous span. The test that matters there is the one asserting a bound
never loses a row, across every pattern shape including escapes and a
`\u{1f600}` prefix — a bound that is too narrow drops rows silently.

### `analyze` is slow on a wide table

11.8 s for 1M rows across 105 columns — it encodes every value of every column
to count distinct values, and samples every column for a histogram. Fine at
five columns, noticeable at a hundred.

## What was not taken from pgrust

The original interest was vectorised execution. It is still not the thing to
build here, and now for a measured reason rather than a guessed one: the
per-row costs this benchmark exposes are *decode* and *grouping*, not predicate
evaluation, which an earlier profile put at 15 ns — 2.5% of the cost of a row.
Vectorising an evaluator that is not the bottleneck would buy nothing. Skipping
the decode entirely, which the projection fix does, bought 3.6x.
