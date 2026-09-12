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

**All 43 queries run.**

| Q | wall | scanned | answer |
|---:|---:|---:|---|
| 1 `COUNT(*)` | 0.78 s | 1,000,000 | 1000000 |
| 2 `COUNT(*) WHERE AdvEngineID <> 0` | 1.10 s | 1,000,000 | 14174 |
| 3 `SUM/COUNT/AVG` | 1.17 s | 1,000,000 | 80778 / 1000000 / 1604.09 |
| 4 `AVG(UserID)` | 0.79 s | 1,000,000 | 1.948e18 |
| 5 `COUNT(DISTINCT UserID)` | 0.91 s | 1,000,000 | 79842 |
| 6 `COUNT(DISTINCT SearchPhrase)` | 1.26 s | 1,000,000 | 18316 |
| 7 `MIN/MAX(EventDate)` | 0.83 s | 1,000,000 | 15901, 15901 |
| 8 group by AdvEngineID | 1.14 s | 1,000,000 | 5 groups |
| 9 distinct users per region | 1.27 s | 1,000,000 | 1242 groups |
| 10 five aggregates per region | 1.37 s | 1,000,000 | 1242 groups |
| 11 distinct users per phone model | 1.21 s | 1,000,000 | 30 groups |
| 12 …per (phone, model) | 1.21 s | 1,000,000 | 59 groups |
| 13 group by SearchPhrase | 1.31 s | 1,000,000 | 18315 groups |
| 14 distinct users per SearchPhrase | 1.37 s | 1,000,000 | 18315 groups |
| 15 group by (engine, phrase) | 1.39 s | 1,000,000 | 19300 groups |
| 16 group by UserID | 1.03 s | 1,000,000 | 79842 groups |
| 17 group by (UserID, phrase) | 1.61 s | 1,000,000 | 98484 groups |
| 18 same, unordered | 1.53 s | 1,000,000 | 98484 groups |
| 19 `extract(minute FROM EventTime)` in the key | 2.25 s | 1,000,000 | 387401 groups |
| 20 `WHERE UserID = …` | **0.08 s** | 0 | — |
| 21 `COUNT(*) WHERE URL LIKE '%google%'` | 2.58 s | 1,000,000 | 95 |
| 22 group by phrase, URL matched | 2.45 s | 1,000,000 | 1 group |
| 23 two `LIKE`s and a distinct count | 3.03 s | 1,000,000 | 53 groups |
| 24 `SELECT *` … order by, limit 10 | 2.70 s | 1,000,000 | 10 rows |
| 25 order by EventTime limit 10 | 1.50 s | 1,000,000 | 10 rows |
| 26 order by SearchPhrase limit 10 | 1.29 s | 1,000,000 | 10 rows |
| 27 order by two columns limit 10 | 1.27 s | 1,000,000 | 10 rows |
| 28 `AVG(length(URL))` … `HAVING COUNT(*) > 100000` | 1.93 s | 1,000,000 | 2 groups |
| 29 `REGEXP_REPLACE` … `HAVING COUNT(*) > 100000` | 3.12 s | 1,000,000 | 2 groups |
| 30 ninety `SUM(width + n)` | 3.00 s | 1,000,000 | 1604089590, 1605089590 |
| 31 group by (engine, IP) | 1.91 s | 1,000,000 | 22830 groups |
| 32 group by (WatchID, IP), filtered | 2.11 s | 1,000,000 | 69354 groups |
| 33 …unfiltered | **5.27 s** | 1,000,000 | **1,000,000 groups** |
| 34 group by URL | 3.21 s | 1,000,000 | 275494 groups |
| 35 `GROUP BY 1, URL` | 2.99 s | 1,000,000 | 275494 groups |
| 36 `GROUP BY ClientIP, ClientIP - 1, - 2, - 3` | 1.50 s | 1,000,000 | 68330 groups |
| 37 URL page views, July 2013 | 1.16 s | **413,825** | 171171 groups |
| 38 Title page views | 0.72 s | **413,825** | 26185 groups |
| 39 with `OFFSET 1000` | 0.51 s | **413,825** | 7385 groups |
| 40 `CASE WHEN … THEN Referer ELSE ''` | 2.20 s | **413,825** | 242387 groups |
| 41 with `IN (-1, 6)` | 0.58 s | **413,825** | 23599 groups |
| 42 with `OFFSET 10000` | 0.54 s | **413,825** | 7006 groups |
| 43 `DATE_TRUNC('minute', EventTime)` | 0.55 s | **413,825** | **1440 groups** |
| | **69.70 s** | | 43 of 43 |

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
| distinct `ClientIP` (Q36's group count) | 68330 | 68330 |
| `SUM(ResolutionWidth)` | 1604089590 | 1604089590 |
| `SUM(ResolutionWidth + 1)` | 1605089590 | 1605089590 |
| distinct minutes (Q43's group count) | 1440 | 1440 |

The results are also consistent with each other in a way that would be hard to
fake:

- Q16 finds 79,842 distinct `UserID` groups, matching Q5's distinct count.
- Q13 finds 18,315 `SearchPhrase` groups — Q6's 18,316 less the empty string
  Q13 filters out.
- Q35 groups by a *constant* and `URL` and finds 275,494 groups, exactly what
  Q34 finds grouping by `URL` alone. A constant key adds no groups.
- Q36 groups by `ClientIP` and three values derived from it and finds 68,330,
  which is the number of distinct `ClientIP`s: the derived keys are
  functionally dependent and add nothing.
- Q43 truncates `EventTime` to the minute and finds **1440** groups. The
  partition is a single day. There are 1440 minutes in a day.
- Q30's `SUM(width)` and `SUM(width + 1)` differ by exactly 1,000,000 — one
  per row.

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

### The third pass: one feature wearing eight disguises

The eight queries left after `COUNT(DISTINCT)` and `LIKE` all needed the same
thing — a value *computed* from a row rather than read out of one. `length(URL)`,
a timestamp's minute, `ClientIP - 1`, `CASE WHEN`, a literal as a grouping key,
`DATE_TRUNC`, ninety `SUM(width + n)`. One feature, not eight.

It reaches the rest of the layer without being woven through it. A query can
*compute* extra values, which are appended after the table's own columns and
addressed by ordinal like anything else — the same trick `JoinSchema` uses for
a joined row. So grouping, sorting, filtering and aggregation all work over a
computed value without any of them learning what an expression is.

`HAVING` follows: a group laid out as its key then its aggregates is a row, so
the ordinary predicate language filters groups without gaining a notion of what
an aggregate is.

And a mistake worth recording, because the benchmark found it immediately. Q30
is ninety sums of ninety computed columns, and it took **90.85 s**. The first
implementation of "append a computed value" rebuilt the row to evaluate the
next one against, cloning every value once per computed column — quadratic.
Evaluating against the values as a slice took it to **2.95 s**, and the whole
set from 167.7 s to 75.2 s.

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

### The last query: a regex, and two ways to be slow with one

Q29 rewrites every `Referer` with `REGEXP_REPLACE` and groups by the result. It
was the one query left unrun, on the reasoning that a regular-expression engine
is a dependency rather than a feature of this layer. That was the wrong call for
a simple reason: `LIKE` was already here, and a pattern is caller input either
way, so the question was never whether to match untrusted patterns but which
engine does it. The `regex` crate was chosen for its linear-time guarantee —
an engine that can backtrack catastrophically turns a caller-supplied pattern
into a denial of service.

Turning it on cost **66.11 s** for that one query, because the pattern was
compiled once per row. A thread-local cache took it to **10.52 s**.

The interesting half is what was left. Q28 is the same query without the regex
and runs in 1.93 s, so the regex was still costing eight seconds over a million
rows — far more than matching should. The cache was handing back a **clone**:

| per call | |
|---|---:|
| clone a `Regex` and drop it | 0.15 µs |
| match on a regex held across rows | 0.6 µs |
| match on a fresh clone | **7.0 µs** |

A `Regex` owns the scratch space its matcher needs, so a clone starts with none
and rebuilds it the first time it is used. Cloning is cheap; *using* a clone is
eleven times slower than using a shared one. Handing out an `Arc<Regex>`
instead took Q29 to **3.12 s** — within 1.2 s of the same query without the
regex, which is the cost of actually matching a million strings.

Rewriting `\1` into the replacement syntax the crate wants stayed per-row: it
measured 16 ms per million rows against seconds for the match, and hoisting it
would have meant storing the rewritten form in a public field, where it would
read as the wrong syntax to anyone who looked.

### A 50% regression from adding an enum variant

Adding `Value::Vector` made **every query in the benchmark about 50% slower**.
Not the vector ones — all of them, including `SELECT COUNT(*)`, on a dataset
with no vectors in it. The suite went from 68 s to 104 s.

It was nearly missed. The suite had grown a query and the machine is shared, so
"a bit slower" had two innocent explanations available. What settled it was
building the previous commit in a worktree and running the two binaries
alternately, on the same machine, in the same minute:

| | Q1 | 43 queries |
|---|---:|---:|
| previous commit | 0.20 s | 11.6 s (42 queries) |
| with vectors | 0.30 s | 18.9 s |

Then the diagnosis went wrong twice. `Value` was the same size afterwards — 40
bytes, still set by `Bytes` — so layout was not it. A decode microbenchmark and
a skip microbenchmark both came back *identical*, which looked like proof the
codec was innocent, and it was not proof of anything: those benchmarks decoded
and skipped, and the regression was in neither.

Callgrind found it in one run. Instructions rose only 5.7% while wall time rose
49%, and one symbol appeared in the new profile that was absent from the old:

```
2,394,003,996 (10.32%)  <slate_tuple::value::Value as core::clone::Clone>::clone
```

`Value::clone` used to be inlined into the row decoder. Adding a variant
holding a `Vec<f32>` made the generated `clone` big enough that LLVM stopped
inlining it — and the row decoder began every row with

```rust
let mut values = vec![Value::Null; table.columns().len()];
```

which fills by **cloning**. A hundred and five out-of-line clone calls per row,
to produce a hundred and five nulls. Building the vector by repetition instead
removed them:

| | Q1 | 43 queries |
|---|---:|---:|
| previous commit | 0.20 s | 11.6 s (42 queries) |
| with vectors | 0.30 s | 18.9 s |
| after the fix | **0.17 s** | **11.6 s (43 queries)** |

At full scale that is **69.70 s for 43 queries**, against 75.22 s for 42 before
any of this — faster than the baseline, with one more query in it.

Three things are worth keeping from that:

- `vec![value; n]` fills by cloning. For a cheap `Copy`-like value that is free
  only while the clone inlines, and nothing warns when it stops.
- A microbenchmark that comes back identical has not exonerated the code. It
  has only exonerated the path it measured. Both of mine measured the wrong one.
- An A/B against the previous commit, built and run on the same machine at the
  same time, is the only measurement that can distinguish a regression from a
  busy afternoon. It cost two builds.

### `analyze` is slow on a wide table

11.8 s for 1M rows across 105 columns — it encodes every value of every column
to count distinct values, and samples every column for a histogram. Fine at
five columns, noticeable at a hundred.

## The regression-suite article, and what it is actually about

A second pgrust write-up ([the regression
suite](https://malisper.me/postgres-in-rust-regression-suite/)) covers how they
reached 100% of Postgres' regression tests. It is worth reading, and worth
being clear about what it is: an account of **porting** Postgres with AI agents
across four attempts at roughly $100,000, not of designing a planner.

That distinction settles the obvious question. pgrust does not have planner
optimisations to copy, because it did not design any — it inherited Postgres'
50k lines of planner C, first through `c2rust` and then rewritten crate by
crate. Their engineering problem was fidelity; ours is judgement. There is no
cost model in that article to learn from.

What *is* there is worth more than a technique.

### Their first attempt died on plan representation

Postgres compiles `SELECT name FROM users WHERE age > 30` into **one** node:

```
SeqScan { scanrelid = 1, targetlist = [name], qual = [age > 30] }
```

Their first attempt produced three, nested: projection over filter over scan.
The article calls the difference small-looking and "massive" in effect — "in C
lots of functions will take a sequential scan node with a filter. In rust,
those functions would sometimes need to take a sequential scan, sometimes take
a filter node, and other times need to take a projection node." It broke the
attempt.

This project's `Plan` is the fused shape, not the tree: one struct carrying
`access`, `residual`, `predicate_columns`, `output_columns` and `filter_first`
together. That was not foresight about porting — it fell out of late
materialisation, which *needs* the filter and the projection visible at the
same level to decode a predicate's columns first and the rest only for rows
that survive. A three-node tree cannot express that; the filter node would have
to be handed rows already decoded. Two unrelated pressures, one answer, and it
is mildly reassuring that Postgres landed there too.

### What was worth taking: plans as a reviewable artifact

Postgres' regression suite works substantially by diffing `EXPLAIN` output, and
that is the applicable idea.

Recalibrating the cost model here was correct and necessary, and it changed
seven tests. Each was discovered separately, over seven build-and-run cycles,
each looking like an isolated surprise rather than one deliberate change with a
wide blast radius. Every fact needed to review it at once already existed.
Nothing collected it.

`crates/slate-kernel/tests/plan_snapshots.rs` now does: eighteen query shapes
against three table sizes, rendered as text and committed. Reverting
`POINT_READ_COST` from 3.0 to 1.0 reports **eleven changed plans in one
output** — the whole blast radius, in the form a reviewer can actually read.

It found two things on its first run. One is a real inconsistency introduced by
the empty-range fix: a plan that reads nothing was reporting the cost and row
count of the range it would have scanned, so `EXPLAIN` described a plan reading
88,209 rows at cost 12.25 while returning none. Fixed. The other is not a bug
but is worth having visible — at a thousand rows a `Point Get` is chosen at
cost 3.00 where a table scan costs 1.12, because a full-key equality replaces
the key-range candidate rather than competing with it, and because a
request-counting unit ignores the thousand rows of bytes a scan would move to
return one.

### What was not taken

The rest of the article is a porting workflow: `find-next-crate`,
`port-crate`, `audit-crate` as named skills, forty concurrent subagents,
`c2rust` as a scaffold to refactor away from. All of it is shaped by having an
existing implementation to be faithful to. There is nothing here to be faithful
to, and a correctness strategy built on diffing against a reference is not
available — which is why the equivalent here is oracles that diff *plans
against each other* and measurements that diff the *model against the
substrate*. Both were found necessary independently, and both found bugs the
other could not.

## What was not taken from pgrust

The original interest was vectorised execution. It is still not the thing to
build here, and now for a measured reason rather than a guessed one: the
per-row costs this benchmark exposes are *decode* and *grouping*, not predicate
evaluation, which an earlier profile put at 15 ns — 2.5% of the cost of a row.
Vectorising an evaluator that is not the bottleneck would buy nothing. Skipping
the decode entirely, which the projection fix does, bought 3.6x.
