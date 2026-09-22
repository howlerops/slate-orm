# Performance

Measured, not guessed. Everything below came from `cargo bench` and
`cargo run --release -p slate-kernel --example perf_report` on a 4-core Xeon at
2.8 GHz; the absolute numbers will differ on your machine, the ratios should
not.

## How to reproduce

```sh
cargo bench                                                   # criterion
cargo run --release -p slate-kernel --example perf_report     # I/O counts
cargo run --release -p slate-headbench --example head_report  # the head node

# the head node under concurrent load, and the two questions that came out of
# the single-request report
cargo run --release -p slate-headbench --example head_concurrency
cargo run --release -p slate-headbench --example stream_step

# the cost model above the 200,000 rows it was calibrated at
SCALE_ROWS=200000,600000,1200000 \
  cargo run --release -p slate-slatedb --example cost_at_scale
```

### Every run says what build it is

Each command above prints a `build:` line before its first number, and a table
recorded from one should carry that line with it:

```
build: slate-slatedb 0.0.1 | features aws, cache | off dhat-heap | release (opt-level 3, debug false) | x86_64-unknown-linux-gnu | slatedb 0.16.0, foyer 0.22.3, object_store 0.14.1, tokio 1.53.1
```

**This exists because a number without it cost nine tasks.** `POINT_READ_COST`
was recorded at 3.0, re-measured at 1.0 by #269 with the change unexplained,
and finally traced by #278 to a build with SlateDB's block cache compiled out
— a cargo feature, three times the object-store requests, and nothing in any
output that said so. The figures were right; the records were incomplete.

Two lines are printed only when they apply, and both mean *do not compare this
with the tables below*:

- `!! this is a debug build` — wall clock in a debug build is a different
  program, not a slower one. Note that `scripts/run_examples.sh` builds debug,
  so a CI transcript says this and a `--release` run does not.
- `` !! `cache` is OFF `` — the defect above, caught at the source.

`scripts/check_build_stamp.py` fails if a feature is added without the stamp
reporting it, or if a program that measures something does not print it.

Four dependencies are named rather than one. #269 offered three explanations
for the point-read figure it could not reproduce — a SlateDB release, a block
size, the readahead — and the code behind all of them is in `slatedb` and
`foyer`. `foyer` is the cache whose *absence* was the whole of finding 8, so a
different version of it is as worth recording as none of it; `object_store`
issues the requests every GET count here counts, and `tokio` schedules the
concurrency the pipelined-read costing is about. `rustls` and `aws-lc-rs` are
under every S3 byte too and are deliberately left out: nothing here isolates a
connection cost, and the line has to stay readable.

**Tables on this page that predate 2026-09-22 have no build line**, and one
cannot be reconstructed for them. Exactly one does carry it — the re-measured
cost model in [§8b](#8b-the-same-measurement-with-the-cache-on-at-release--and-the-first-table-here-that-says-what-built-it),
which is the first table here recorded from a run that could say what built
it. `scripts/check_table_provenance.py` holds
that boundary rather than leaving it to this sentence: it finds every table in
`docs/`, decides which record a measurement, and fails on one that is neither
stamped, marked `<!-- not a measurement -->`, nor frozen in
[`scripts/frozen_tables.json`](../scripts/frozen_tables.json) as predating the
rule. The roster pins table *contents*, so editing a number in one of those 70
legacy tables takes it off the list and asks for the line — which is right,
because a changed number means a run happened, and that run could say what
built it. What is recorded about them is what their own
prose says — fixture size, run count, machine load — plus the `--release` in
the commands above. Finding 8 and the `POINT_READ_COST` history in
[`docs/correctness.md`](correctness.md) are the only two places in this
repository where the cargo features behind a measurement were written down
before this existed, and both are write-ups of that defect.

Every command above runs at the size the figures on this page were taken at.
Three environment variables shrink them — `KERNELBENCH_ROWS` for the kernel's
examples, `HEADBENCH_*` for the head node's, `SCALE_ROWS` for the storage ones
— and `scripts/run_examples.sh --smoke` sets all of them, which is how CI runs
every benchmark in seconds. **A smoke run's numbers are not the numbers on this
page and are not comparable to them**; each example prints the size it ran at,
so a pasted line says which it was.

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

> **Corrected later.** "This server's per-request cost is its own" turned out
> to be 34 ms of Nagle: the in-process S3 server accepts connections without
> `TCP_NODELAY`. With that fixed the same two settings measure 827 ms against
> 103 ms — about 8x, not 158x-409x. The 31x request count is unaffected. See
> section 9 below.

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

### Two ways a clone stops being free

The last round found the same shape twice more, in places nothing here would
have thought to look. Both are recorded in full in
[`docs/clickbench.md`](clickbench.md); the short versions:

- **A cached `Regex` was handed out by clone.** A `Regex` owns the scratch
  space its matcher needs, so a clone starts with none and rebuilds it on
  first use. Cloning cost 0.15 µs; *matching on the clone* cost 7.0 µs against
  0.6 µs for one held across rows. Sharing an `Arc<Regex>` took ClickBench's
  Q29 from 10.52 s to **3.12 s**.

- **Adding an enum variant made every query 50% slower.** `Value::Vector`
  pushed `Value::clone` past the inlining threshold, and the row decoder began
  each row with `vec![Value::Null; 105]` — which fills by cloning. A hundred
  and five out-of-line calls per row, on a dataset containing no vectors.
  Building the vector by repetition took the suite from 104 s back to
  **69.70 s**, now with all 43 queries rather than 42.

The second is the one worth internalising. `Value` did not change size, two
microbenchmarks of the codec came back *identical*, and the suite had grown a
query — so every cheap check said nothing was wrong. What found it was building
the previous commit in a worktree and running both binaries alternately on the
same machine, then callgrind: instructions up 5.7%, wall time up 49%, and one
symbol in the new profile that was absent from the old.

## The cost model was measured, and was wrong

Everything below this line predates a calibration against real object storage,
and the constants it rests on have since changed. `slate-slatedb`'s
`cost_calibration` example found the model overcharging scans about eighty
times and undercharging index lookups about forty, in the same direction — so
the planner preferred a plan doing **58x the object-store requests, taking 9x
as long**. The full account is in
[`docs/correctness.md`](correctness.md#the-cost-model-was-calibrated-against-itself).

Two things to carry into any reading of the numbers here:

- The unit is now **object-store requests**, measured, not round trips inferred
  from a latency fixture. A scan returns about 8,000 rows per request; a point
  read costs about 1. ~~A point read costs about 3~~ — corrected on
  2026-09-22. It was 3 when this paragraph was written, and #269 re-measured
  it at 1.0; #278 then found that the 3 came from a build with SlateDB's
  block cache compiled out (finding 8 below). This sentence was the last place
  the old figure survived, because `scripts/check_cost_prose.py` read `crates/`
  and not `docs/`. Since #283 it reads both, so a fourth place cannot open up
  the way this one did.
- Wall times below were derived as cost × 2.2 ms against the old constants.
  They are kept because the *relative* findings they record — late
  materialisation, the projection fix, hash grouping — were measured directly
  in wall time and still hold. The costs beside them no longer are.

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

## The head node, measured

Everything above is the record layer as a library. `crates/slate-server` is the
process that puts it behind a socket, and until now it was the one component
here with no number attached to it at all — including the two constants it is
tuned by, both of which say in their own doc comments that they were chosen by
argument rather than measurement.

`crates/slate-headbench` is the harness. It runs a **real head node on a
loopback TCP socket over a real SlateDB**, and beside it an **in-process
control** built from the same catalog, the same rules and the same stores
(`harness::InProcess`) — the pool and the record store the head node itself
holds, called the way its handlers call them, with no protobuf type constructed
anywhere. The difference between the two is the head node.

```sh
# Every benchmark in the crate, at its recorded size. `--smoke` runs the same
# five at the smallest fixture each accepts: worthless numbers, seconds rather
# than minutes, and the only thing that catches a benchmark that has stopped
# working. CI runs `--smoke` on every push, which is new — for weeks nothing
# ran any of them and four were broken, twice over, by security fixes
# elsewhere in the tree.
crates/slate-headbench/run.sh
crates/slate-headbench/run.sh --smoke

# The same for the storage examples, which is where the planner's two cost
# constants come from. `SCALE_ROWS` shrinks their fixtures the way
# `HEADBENCH_ROWS` shrinks the head node's.
crates/slate-slatedb/run.sh --smoke

cargo run --release -p slate-headbench --example head_report
cargo run --release -p slate-headbench --example head_report -- stream lease

# More runs when the machine is busy; a different batch sweep to chase a step.
HEADBENCH_RUNS=21 cargo run --release -p slate-headbench --example head_report
HEADBENCH_BATCHES=118,124,126,128 cargo run --release -p slate-headbench \
    --example head_report -- stream

# the dependency-defaults sweep: whether SlateDB has a cache at all, whether
# the in-process S3 server's socket is Nagled, and what the daemon's replica
# poll interval costs
cargo run --release -p slate-headbench --example cache_probe
cargo run --release -p slate-headbench --example s3_nodelay
HEADBENCH_POLL_MS=10000 cargo run --release -p slate-headbench \
    --example head_report -- routing
```

`HEADBENCH_ROWS` overrides the 20,000-row fixture in all five; it used to work
in three, and the two that hard-coded it were the two a smoke run had to wait
two minutes for. The default is unchanged, so every number below still
describes 20,000 rows.

### Conditions, and why they are stated first

Numbers below are the median of 21 runs with the full range beside them, taken
on a 4-core Firecracker VM which **had up to three cargo builds running on it
during development**. That is not an aside. Medians here move by a factor of
two with what else is on the machine, and every measurement was repeated across
load levels from 0.2 to 5.0 before anything was written down. Where a
difference could not be told apart from that, it is reported as noise and no
number is claimed. The run recorded here was taken at load 0.49 falling to
0.24.

The object store under SlateDB is in-memory. So the WAL, the memtable, the
manifest and the replica reader are all real and a replica genuinely lags, but
**there is no network under the object store**. Every place that changes what a
number means is called out below.

Two things the harness does that are not optional, both because it got them
wrong first:

- **The channel is warmed with 2,000 requests before anything is timed.** An
  earlier version measured the empty RPC first and an insert last and reported
  the insert as *cheaper than an empty call*, which cannot be true. HTTP/2 opens
  with a small flow-control window and grows it. The floor is now measured again
  at the *end* of the section as the check; the drift across the section is
  0.6–5.1 µs, inside noise.
- **Controls are measured before the workload that changes the data.** See the
  replica finding below, which reversed once this was fixed.
- **The listening socket had Nagle's algorithm on.** Found last, and it was in
  every streaming number this harness had produced. It has its own section
  immediately below, because until it is read the batch-size table underneath
  cannot be.

### 0. A fourth harness bug, and it was inside every stream measurement

`tonic::transport::Server` sets `TCP_NODELAY` by default. Its own
documentation for `serve_with_incoming` says the setting *"is ignored when
using this method"* — and `serve_with_incoming` is exactly what a harness that
lets the operating system choose its port has to call. So every measurement in
this crate, and every streaming figure in the section below, was taken against
a server socket with **Nagle's algorithm enabled**, which nothing reaching a
deployment through `Server::serve` would have.

Nagle holds a small write back until the previous one has been acknowledged,
and Linux delays acknowledgements. A unary reply is a single write and never
notices. A stream is at least two — the header message that names the replica,
then the first batch of rows — and when the second is not ready in the same
poll as the first, it waits.

Both states, same fixture, same process, four clients, with the kernel's own
delayed-acknowledgement counter beside them:

| | ops/s | p50 | p99 | worst | delayed ACKs /op |
|---|---:|---:|---:|---:|---:|
| Nagle on, unary `Get` | 7,109 | 102.2 µs | 323.0 µs | 8.47 ms | **0.00** |
| Nagle on, 1-row query (`Point Get` plan) | 58 | 601.0 µs | 48.01 ms | 48.18 ms | **0.45** |
| Nagle on, 10-row query (scan plan) | 22 | **44.01 ms** | 48.05 ms | 48.05 ms | **1.16** |
| `TCP_NODELAY`, unary `Get` | 7,010 | 109.3 µs | 294.2 µs | 6.41 ms | 0.00 |
| `TCP_NODELAY`, 1-row query | 4,210 | 229.9 µs | 377.3 µs | 4.24 ms | 0.00 |
| `TCP_NODELAY`, 10-row query | 3,672 | 267.2 µs | 374.8 µs | 1.01 ms | 0.00 |

**A ten-row streaming query went from 44.01 ms to 267 µs — 165×.** The unary
call is the control and it did not move: same socket, same server, same
authentication and conversion, and only the *shape of the response* differs.

Those rows were taken while the machine was at load 8.9, which is why the
one-row query's median is 601 µs there. On a quieter pass the same three
Nagled measurements came out at 190 µs, 243 µs and **44.00 ms** — so the scan
plan stalls on essentially every call and the `Point Get` plan stalls only
sometimes, which is what a race between two writes reaching the socket should
look like. The 44 ms figure is the one that does not move.

The last column is why this is a mechanism and not a coincidence.
`TcpExt: DelayedACKs` counts each time the kernel's delayed-acknowledgement
timer expires — the event a Nagled sender is waiting on. It is **zero per
operation for every unary call and for every call once `TCP_NODELAY` is set**,
and roughly one per operation for exactly the two shapes that stall. Nothing
about the head node predicts that pattern; the socket option does.

**This is not only the harness.** `crates/slate-serverd`, the daemon that
actually ships, serves with `serve_with_incoming_shutdown` over a bare
`TcpListenerStream`, which ignores `tcp_nodelay` for the same reason. On the
evidence above that is tens of milliseconds on every streaming response. It is
reported rather than fixed here: that crate is outside this benchmark's
ownership, and the fix is one line — `set_nodelay(true)` on each accepted
connection, which is what `harness::serve` now does.

### 1. A request over gRPC costs about 130 µs, and it is almost all transport

The same operation, over the wire and against the kernel directly. `Leadership`
is the floor: it reads a watch channel, touches no storage and does not even
authenticate, so it is what an empty round trip costs.

| operation | over gRPC | in process | difference |
|---|---:|---:|---:|
| **empty RPC** (`Leadership`) | **119.3 µs** [111.1 – 347.5] | — | — |
| get by primary key | 132.4 µs [125.7 – 144.9] | 2.92 µs [2.78 – 3.25] | 129.5 µs (45×) |
| explain | 130.1 µs [125.8 – 137.3] | 1.53 µs [1.50 – 1.86] | 128.5 µs (85×) |
| query returning 1 row | 166.9 µs [161.6 – 616.8] | 4.31 µs [4.11 – 4.73] | 162.6 µs (39×) |
| insert 1 row, autocommit | 180.8 µs [168.7 – 629.2] | 49.9 µs [48.9 – 55.2] | 130.9 µs (3.6×) |

The same thing over `MemoryStore`, with the storage term driven to nearly
nothing, so that whatever is left is the head node:

| operation | over gRPC | in process | difference |
|---|---:|---:|---:|
| empty RPC (`Leadership`) | 112.6 µs [109.8 – 127.2] | — | — |
| get by primary key | 136.3 µs [118.1 – 768.0] | **472 ns** [470 – 580] | 135.8 µs (289×) |
| explain | 131.1 µs [125.0 – 339.4] | 1.49 µs | 129.6 µs (88×) |
| query returning 1 row | 157.6 µs [151.4 – 832.4] | 1.40 µs | 156.2 µs (112×) |
| insert 1 row, autocommit | 131.0 µs [123.9 – 147.6] | 2.56 µs | 128.5 µs (51×) |

**The head node's share of a `get` is 129.5 µs over SlateDB and 135.8 µs over
`MemoryStore`.** Two backends whose own cost differs by a factor of six agree
on the overhead to within 5%, which is the cross-check: the difference is a
property of the head node, not of the storage under it.

Now subtract both the floor and the storage, and what is left is the head
node's own work — authenticating from transport metadata, the catalog name
lookup, converting the request in and the response out:

| | SlateDB arm | `MemoryStore` arm |
|---|---:|---:|
| get | 10.3 µs | 23.2 µs |
| explain | 9.3 µs | 17.0 µs |
| insert, autocommit | 11.6 µs | 15.8 µs |
| query returning 1 row | **43.4 µs** | **43.6 µs** |

So of the ~130 µs a request costs, **110–120 µs is the wire and roughly 10–23 µs
is the head node's own work**. The two arms disagree by about as much as this
harness can resolve at that scale — the two floors themselves differ by 6.6 µs —
so treat the range, not either end, as the answer. The protobuf conversion layer
that `convert.rs` spends hundreds of lines on is not where the time goes.

The one shape that is different is the **streaming query, which costs ~43 µs
of head-node work rather than ~10–23 µs even when it returns a single row**. It
is also the one row of that table where the two backends agree to three
significant figures — 43.4 µs and 43.6 µs — which is what a fixed per-call cost
looks like. And it carries a much heavier tail: 167 µs median against a worst
run of 617 µs, where a `get` in the same section ranged 126–145 µs, and on a
loaded machine the same measurement reached 11 ms.

That is the price of the design in `service.rs`: a query spawns a task, hands
the routing result back over a `oneshot`, and streams rows through an
`mpsc::channel(2)`. Three scheduler hand-offs where a `get` has none. The cost
is measured; the attribution to those three is read off the code rather than
measured separately.

**What this does not measure.** The client is in the same process on the same
four cores, so this is head node plus client stub plus loopback, with no real
network. Against a client one datacentre hop away the transport term grows and
the head node's 10–23 µs grows not at all — so 130 µs is an *upper* bound on the
head node's share of a request and a *lower* bound on what a remote caller
sees.

### 2. Stream throughput saturates at a batch of about 32, and 256 is fine

20,000 rows, whole-table scan, `Limits::rows_per_message` swept, 21 runs.
**Re-measured with `TCP_NODELAY` set** — the table in earlier revisions of this
document was taken through the Nagled socket of section 0 and its first-row
column was measuring the kernel's acknowledgement timer. The in-process floor
— the same scan with no head node in front of it — is **26.15 ms
(765,000 rows/s)** in this run.

Drain time is quoted as the **best of 21** and first-row latency as **both** the
median and the best of 21, because on a shared box the median of either column
carries whatever else was running and the best does not.

| batch | messages | drain, best of 21 | rows/s | first row, median | first row, best |
|---:|---:|---:|---:|---:|---:|
| 1 | 20,001 | 70.7 ms | 283,000 | 596 µs | 366 µs |
| 8 | 2,501 | 52.9 ms | 378,000 | 995 µs | 407 µs |
| 32 | 626 | 44.4 ms | 451,000 | 3.73 ms | 577 µs |
| 64 | 314 | 43.3 ms | 462,000 | 1.79 ms | 573 µs |
| 96 | 210 | 43.0 ms | 466,000 | 1.16 ms | 524 µs |
| 112 | 180 | 42.3 ms | 473,000 | 815 µs | 547 µs |
| 127 | 159 | 43.6 ms | 459,000 | 1.13 ms | 826 µs |
| 128 | 158 | 44.5 ms | 449,000 | 1.39 ms | 773 µs |
| 129 | 157 | 41.5 ms | 482,000 | 1.44 ms | 824 µs |
| 160 | 126 | 41.8 ms | 479,000 | 2.58 ms | 834 µs |
| **256** | **80** | **41.3 ms** | **485,000** | **2.62 ms** | **1.13 ms** |
| 512 | 41 | 40.5 ms | 494,000 | 2.26 ms | 1.74 ms |
| 1024 | 21 | 40.7 ms | 492,000 | 3.00 ms | 2.68 ms |
| 4096 | 6 | 38.9 ms | 514,000 | 10.95 ms | 9.26 ms |
| 16384 | 3 | 40.9 ms | 489,000 | 41.28 ms | 37.00 ms |

Two things changed when the socket did, and both are corrections to what this
section used to say.

**The drain column is no longer bimodal.** It used to sit at either ~36 ms or
~76 ms with no relation to the batch size, and that was written down as "the
machine". It was not: with `TCP_NODELAY` the best drain is **38.9–44.5 ms at
every batch size from 32 up**, a spread of 14% rather than a factor of two. The
bimodality was the Nagled socket, and calling it the machine was wrong.

**Throughput saturates by a batch of about 32, not 64.** Against the 26.15 ms
in-process floor, streaming 20,000 rows over gRPC adds ~16 ms at batch ≥32
(**0.8 µs/row**) and ~44 ms at batch 1 (**2.2 µs/row**). Per-message framing is
real, it costs about 1.4 µs a message, and it is paid off by 32.

First-row latency is the other half of the trade, and above a batch of about
1,000 it grows with the batch as producing that many rows must: 2.7 ms at 1,024,
9.3 ms at 4,096, **37.0 ms at 16,384**.

#### The step at 125 rows: it was the socket

The step was real, it was reproducible, and it was **not in the head node**.
Setting `TCP_NODELAY` removes it.

The measurement that settles it separates two times the earlier sweeps did not:
the **header message**, which the head node sends before it has produced a
single row, and the **first message carrying rows**. Routing, opening the
cursor and the request round trip are all in the first; only making and
shipping the batch is in the difference. That difference is a within-run
subtraction, so it survives a busy machine in a way the absolute times do not.

The difference, `first row − header`, 21 runs per point, both sockets, same
process, same fixture:

| batch | 118 | 120 | 122 | **124** | **125** | **126** | 127 | 128 | 130 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Nagle on | 272 µs | 90 µs | 89 µs | 148 µs | 306 µs | 264 µs | **3.17 ms** | **2.76 ms** | 113 µs |
| `TCP_NODELAY` | 181 µs | 65 µs | 101 µs | 133 µs | 262 µs | 356 µs | 362 µs | 448 µs | 385 µs |

and the first-row latency those add up to:

| batch | 118 | 120 | 122 | 124 | 125 | 126 | 127 | 128 | 130 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Nagle on, median | 1.91 ms | 1.71 ms | 2.91 ms | **1.10 ms** | **3.30 ms** | 3.64 ms | 4.04 ms | 3.60 ms | 3.68 ms |
| `TCP_NODELAY`, median | 1.62 ms | 867 µs | 854 µs | 842 µs | 1.02 ms | 1.04 ms | 1.06 ms | 1.15 ms | 1.14 ms |
| Nagle on, worst run | 3.75 ms | 5.96 ms | 6.14 ms | 5.58 ms | 6.71 ms | 18.97 ms | 4.98 ms | 7.62 ms | 18.55 ms |
| `TCP_NODELAY`, worst run | 7.86 ms | 7.50 ms | 1.08 ms | 1.28 ms | 1.44 ms | 1.46 ms | 1.43 ms | 2.19 ms | 2.80 ms |

**With Nagle on the step is there, at 2.5–3 ms, in the same place it was first
found. With `TCP_NODELAY` it is gone.** Above the boundary the Nagled arm's
worst run reaches 19 ms; the `TCP_NODELAY` arm's worst run reaches 2.8 ms.

The Nagled row of the first table is ragged where the second is not, and that
is worth reading rather than tidying: at 126 and 130 the 2.7 ms landed *before*
the header rather than after it, so the difference column looks clean while
first-row latency is stepped anyway. A stall that can attach itself to either
of two messages is a stall in the transport under both of them, not in the work
between them. A 15-run sweep taken an hour earlier put the step at 126 in the
difference column and 130 in the clean one — it moves between sweeps, which is
the same bimodality the original write-up noticed and could not place.

Two further arms rule out the remaining candidates.

**It is not a fixed count of operations, which retires the tokio-budget
hypothesis on mechanism rather than on an anomalous run.** Making a row five
times wider — a 220-byte `note` where the fixture had a null — moves the
threshold *down*, from about 125 rows to somewhere between 48 and 64:

| batch | 8 | 16 | 24 | 32 | 48 | 64 | 96 | 124 | 126 | 256 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| wide rows, Nagle on, `first − header` | 15 µs | 32 µs | 75 µs | 107 µs | 161 µs | **925 µs** | 1.37 ms | 988 µs | 840 µs | 1.70 ms |
| wide rows, `TCP_NODELAY` | 20 µs | 35 µs | 36 µs | 128 µs | 128 µs | 116 µs | 103 µs | 200 µs | 649 µs | 1.23 ms |

tokio's cooperative budget is 128 *operations*, and 128 operations do not become
56 because the rows got longer. Whatever the trigger is, it moves with how long
the first batch takes to produce and how big it is — which is what decides
whether the head node's two messages reach the socket as one write or two.

**And it is not a constant number of bytes either**: ~125 narrow rows is roughly
6 KB and ~56 wide rows is roughly 15 KB. So the trigger is a race, not a
buffer boundary, which is also what the bimodality above the threshold always
said it was — a race sometimes goes the other way, and a fixed buffer never
does.

What the delayed-acknowledgement counter in section 0 adds is the last piece:
the shapes that stall are exactly the shapes that make the kernel's
acknowledgement timer fire, and setting `TCP_NODELAY` takes that count to zero
and the stall with it.

**What is left unexplained, and it is small.** With `TCP_NODELAY` there is
still a rise across the same region — about 130 µs below the boundary and
about 350–450 µs above it, with non-overlapping run-to-run ranges either side.
That is a real difference of roughly 250 µs, seven to ten times smaller than
the step it replaced, and it is consistent with the per-row cost of producing a
larger batch (the same column reaches 912 µs at 256 and 1.23 ms for wide rows
at 256). It is not claimed as anything more than that, and it is not worth a
constant.

#### What that says about `rows_per_message = 256`

**The constant stays at 256, and now for a measured reason rather than because
of a cliff nobody could explain.**

The argument for lowering it has been withdrawn, because the thing it rested on
was a socket option. It used to read: a batch of 112 delivers its first row in
674 µs where 256 takes 3.07 ms at identical throughput, so 2.4 ms of latency
was being paid for nothing. With `TCP_NODELAY` those two numbers are **547 µs
and 1.13 ms** (best of 21) or **815 µs and 2.62 ms** (median of 21 on a busy
box). What is left is roughly 600 µs, it is the genuine cost of producing 144
more rows before sending any of them, and it is bought back in framing: 256
sends a third as many messages as 112.

The range the constant should stay inside is now measured at both ends:

- **32 at the bottom.** Below it per-message framing costs real throughput —
  batch 1 drains 20,000 rows in 70.7 ms against 41–44 ms from 32 up, and adds
  2.2 µs a row against 0.8.
- **About 1,000 at the top.** Above it, time to first row grows with the batch:
  2.68 ms at 1,024, 9.26 ms at 4,096, 37.0 ms at 16,384.

256 sits in the middle of that, and its original rationale — keeping a wide
row's batch under a megabyte — is untouched and is still the binding one. These
rows are roughly fifty bytes of protobuf each, so a 256-row message here is
under 20 KB; a 4 KB row would put the same message at exactly 1 MB, which is
the case the constant was chosen for.

### 3. Autocommit against an explicit transaction: the answer is the flush

Two arms, because the answer is entirely different depending on one setting
that is not the head node's.

**At `Durability::Visible`**, where a commit returns as soon as the write is
visible, the difference is exactly the RPC count:

| | per row | total |
|---|---:|---:|
| 1 row, autocommit (1 RPC) | 174.1 µs [167.3 – 622.4] | — |
| 1 row, begin + insert + commit (3 RPCs) | 505.2 µs [470.4 µs – 1.35 ms] | — |
| 100 rows, one autocommit call | **11.6 µs** [10.1 – 451.2] | 1.16 ms |
| 100 rows, txn + 100 insert calls | 177.8 µs [167.5 – 617.9] | 17.8 ms |
| 100 rows, txn + 1 insert call | 14.5 µs [12.7 – 19.8] | 1.45 ms |

Wrapping a single write in a transaction costs **331 µs (2.9×)**, which is two
extra round trips at the ~130 µs each section 1 measured — the two measurements
agree, which is the point of quoting both. And a row written one RPC at a time
costs 178 µs against 11.6 µs written in a batch: **15× for the same hundred
rows**, all of it round trips, none of it storage.

**At `Durability::Durable`**, the default, none of that is visible at all:

| | per row |
|---|---:|
| 1 row, autocommit | 101.10 ms [100.99 – 101.25] |
| 1 row, begin + insert + commit | 101.17 ms [101.04 – 101.35] |
| 100 rows, any of the three shapes | 1.01 ms |

Every durable commit costs **101 ms, with a range of 0.3%**. That is SlateDB's
`flush_interval`, which defaults to 100 ms: a durable commit waits for the next
scheduled WAL flush, and the wait dominates everything else by three orders of
magnitude. The transaction question becomes unanswerable — 69 µs apart, inside
noise — because both shapes commit exactly once.

The finding is the one that falls out of that: **a durable commit costs a flush
interval, so the only thing that matters is how many rows share one.** A
hundred rows in one commit is 1.01 ms per row against 101 ms per row one at a
time — a **100× difference**, and the number is not a property of the head node
or of the record layer. It is `flush_interval`, and a deployment that writes
row-at-a-time durably should be looking at that setting before anything in this
repository.

### 3b. The same question from a client, where the answer is *bigger*

Section 3 is a Rust wire test: it goes through `tonic` and nothing else. Every
README sentence saying "a batch is a round trip" was a claim about the three
SDKs resting on a measurement that had never gone through one.
`examples/batchbench` is that measurement — 100 single inserts against one
batch of 100, five runs, against one head node on memory storage.

| client | one row at a time | in one batch | ratio |
|---|---:|---:|---:|
| Python | 969.6 µs [925.9 – 1038.4] | 30.7 µs [29.7 – 31.8] | **31.6×** |
| Go | 763.4 µs [733.9 – 772.3] | 35.7 µs [33.8 – 36.1] | **21.4×** |
| TypeScript | 1155.8 µs [1114.7 – 1768.2] | 34.0 µs [33.5 – 64.6] | **34.0×** |

**The hypothesis this was built to test was wrong, and in the direction that
matters.** `docs/orm-comparison.md` predicted a *smaller* multiplier from a
client than from the wire:

> The clients add per-request work batching does not save — schema claims,
> value encoding, per-operation table resolution — so their multiplier is
> smaller by an unknown amount.

Measured, it is larger: 21–34× against section 3's 15×. The reasoning had the
arithmetic backwards. That per-request work is *per request*, so it multiplies
the one-at-a-time arm by a hundred and the batched arm by one — it widens the
gap rather than narrowing it. A client pays more per round trip than `tonic`
does, which is exactly why saving round trips is worth more to a client, not
less. **The prediction is withdrawn.**

The absolute numbers are the loopback's, so they are the conservative end: the
round trip a batch saves is cheapest here and costs more over a network.

**The cap question, decided from the number.** The item asked whether a client
should check `max_batch_operations` before sending, and said to decide from the
measurement rather than guess. The default cap is 1,000 operations. A batch of
1,000 costs about 30 ms of the numbers above; a refused round trip costs one
RPC, which is the single-insert column — about 1 ms. So the refusal is **about
3% of the work the batch would have done**, paid once, at the moment a caller
discovers their batch is too big.

Three percent does not buy a client-side copy of a server-configurable limit.
Two numbers that can disagree is a client refusing a batch a differently
configured server would have taken, which is a worse failure than a wasted
millisecond. **Left server-side**, as the item allowed for.

### 4. Routing is free; the freshness wait is the manifest poll

Three following replicas over the same object store, `manifest_poll_interval`
50 ms, tenant-scoped table so affinity has something to key on. Sanity first,
because a routing benchmark that is quietly reading from the wrong place is
worthless: `Freshness::Any` was served by `replica-c`, `Freshness::Latest` by
`writer`, `AtLeast(2)` by `replica-c`, and every read asserted that it found
its row.

**The decision itself:**

| | |
|---|---:|
| `pool.route`, tenant affinity (rendezvous hashing) | **103 ns** [100 – 105] |
| `pool.route`, round robin | 27 ns [26 – 28] |

Rendezvous hashing costs 76 ns more than round robin. Against a request that
costs 130 µs that is **0.06%** — the cache-locality argument in
[`topology.md`](topology.md) does not have to justify itself against a routing
cost, because there is not one.

**The read, at each freshness:**

| | over gRPC |
|---|---:|
| `Freshness::Any` (a replica) | 134.0 µs [129.6 – 356.3] |
| `Freshness::Latest` (the writer) | 135.8 µs [127.9 – 351.0] |
| `AtLeast(a sequence already reached)` | 133.0 µs [129.4 – 348.4] |
| the same read in process, no gRPC | **3.91 µs** [3.84 – 4.09] |

All three are the same read. **Proving freshness against a replica that has
already caught up costs nothing measurable** (1.0 µs, inside noise), and so
does choosing a replica over the writer (1.8 µs, inside noise). The head node's
share is 130.1 µs, the same figure as every other unary RPC in this document.

**The case the replica has to catch up** — commit durably, then immediately
demand that sequence:

| | |
|---|---:|
| `AtLeast(a sequence just committed)` | **28.3 ms** [5.4 – 43.6] |
| the same read with nothing to wait for | 133.0 µs |
| fell back to the writer | **0 times in 176** |

A read that has to wait costs **213× one that does not**, and the distribution
is what it should be: roughly uniform between 5 ms and 44 ms against a 50 ms
manifest poll, because the commit lands at a uniformly random point in the
replica's polling cycle. **The freshness wait is half a manifest poll interval
on average, and that is a configuration value, not a code cost.** A deployment
that finds read-your-writes too slow should look at
`DbReaderOptions::manifest_poll_interval` first; the 250 ms `catch_up` budget in
`RoutingPolicy` never came close to expiring, so no read was pushed onto the
writer.

> **And cannot, today.** `slate-serverd` never sets that interval, so a
> deployed replica polls every 10 seconds — forty times the `catch_up` budget.
> At that setting this same measurement reads 251.87 ms and falls back to the
> writer 64 times out of 64. See section 10 below.

#### A replica that has just taken writes reads 2.4× slower

This one is here because the harness got it wrong first and the wrong version
was more interesting than the right one.

The in-process control for the replica read was originally measured *after* the
catch-up loop above, which commits 176 times. It came out at 107 µs against the
writer's 2.9 µs, and the obvious story — "reading from a replica costs 25× more
than the writer's memtable, because a replica reads object storage" — was wrong.
Measured before those commits, the same read is **3.91 µs**: a replica read and
a writer read cost the same.

What the 176 commits actually did is worth its own line, measured directly by
repeating the gRPC read after them:

| `Freshness::Any` over gRPC | |
|---|---:|
| before the catch-up loop | 134.0 µs [129.6 – 356.3] |
| after 176 durable commits | **327.4 µs** [304.7 – 388.1] |

**2.4×**, and it is not the head node — it is a replica with 176 commits' worth
of un-compacted recent state to consult on every read. That is a real property
of a following replica under write load and it is invisible unless the
measurement is ordered deliberately.

### 5. A renewal is one conditional PUT; the term is a failover budget

| | |
|---|---:|
| cold acquire | **1 GET + 1 conditional PUT** |
| one renewal | **0 GET + 1 conditional PUT** |
| `lease.renew`, in-memory object store | 756 ns [719 – 1020] |
| `lease.observe` (read only) | 568 ns [562 – 638] |
| `Leadership::is_leader` (the write-path check) | **17 ns** [15 – 18] |

The request counts are the numbers that travel; the clock is against an
in-memory object store, so it is the compare-and-set, the encode/decode and the
mutex, with the network — which is all of the cost in a bucket — removed.

A renewal is a *single* conditional write: the client keeps the object version
it last saw, so it does not re-read. Against the 5 s renewal interval that
`Cadence::for_term` derives from the 15 s term, the local cost is
**0.000015% of the interval**. And the check that runs on every write —
`Leadership::is_leader`, which is what makes a fenced node's refusal local
rather than a round trip into a dead store — is **17 ns**. Neither of these is
a reason to choose any particular term.

Renewing does not disturb serving either. At **250× the real renewal rate** (a
60 ms term renewing every 20 ms) a `get` was 154.4 µs against 143.1 µs with no
renewal running: inside noise. At the real rate the effect is 1/250th of
something already unmeasurable.

**So what does the term actually buy, and cost?** It buys tolerance of renewals
that do not arrive. It costs how long a *crashed* leader keeps its successor
out, because a process that dies cannot release — and that had never been
measured:

| term | takeover after a crash (tight poll) | at the real campaign cadence | after a graceful release |
|---|---:|---:|---:|
| 300 ms | 300.8 ms | 303.5 ms | 3.8 µs |
| 600 ms | 601.9 ms | 604.1 ms | 2.9 µs |
| 1.2 s | 1.20 s | 1.20 s | 3.1 µs |

**Takeover after a crash costs exactly the term**, and the successor's own
campaign interval (`term / 3`) is added on top of it. A holder that releases
hands over in the time of one conditional write — 3 µs here, one round trip in
a bucket — which confirms what [`topology.md`](topology.md) claims for the
`release` on a clean shutdown, and is a five-orders-of-magnitude difference
between a graceful restart and a crash.

At the default 15 s term this is **up to 15 s of refused writes after a crash,
plus up to 5 s before the successor next campaigns**. That sentence is
arithmetic from the mechanism above rather than a measurement; three terms an
order of magnitude apart all took exactly their term, and nothing in the
mechanism is nonlinear.

#### Is 15 seconds sensible? Half the question is answerable here

The half that is: **the term is not paying for renewal cost.** A renewal is one
conditional PUT and the cadence gives it 5 seconds to complete. Even a very bad
object store does not need 5 seconds for a single conditional write, and the
term tolerates two consecutive failures on top of that. The margin is large.

The half that is not: **how long a conditional PUT to real object storage
actually takes at the tail.** This harness measures it against an in-memory
store, where it is 756 ns. On S3 it is tens of milliseconds typically and can be
seconds at the tail under throttling, and that tail is exactly what the term
exists to survive. Nothing here measures it, so nothing here says how much
margin is needed.

What the measurement does establish is the price list, which was missing:

| term | tolerates a renewal stall of | worst-case write outage after a crash |
|---:|---:|---:|
| 15 s (current) | up to ~10 s across two failures | ~20 s |
| 5 s | up to ~3.3 s across two failures | ~6.7 s |

**No change is proposed.** Choosing between those rows needs the conditional-PUT
tail latency of the deployment's object store, which is a measurement to take
against MinIO or a real bucket, and it belongs beside the existing S3 work in
`slate-slatedb` rather than in a guess here. What should not survive is the
current position, which is that 15 s is a "starting point" with nothing
attached: it is now a starting point with a 20-second failover attached to it,
and that is the number an operator has to agree to.

### 6. Under concurrency

Everything above is one request at a time, which left the two designs the head
node actually rests on — the per-stream `mpsc::channel(2)` in `service.rs` and
the task-per-transaction registry in `session.rs` — never once under pressure.
`examples/head_concurrency.rs` puts them under it.

**How this section is arranged, and why.** The server and the load generator
run on **separate tokio runtimes with different thread names**, still sharing
the same four cores through the OS scheduler. Not to isolate them — a benchmark
whose client is on the box should say so — but so that
`/proc/self/task/*/schedstat` can attribute the CPU. `srv` and `cli` below are
cores' worth spent by each side; `cores` is the whole process.

The load is **closed loop**: each client waits for its own reply before sending
again. Throughput is therefore a completion rate and latency percentiles are
honest, but nothing here can be read as an open-loop saturation curve.

The four sweeps below come from one run; the sub-sections after them
(`max_transactions`, the slow consumer, the lease) come from a second run of
the same binary about twenty minutes later, and each says what it was compared
against inside itself.

**Read the `cpu/op` column first.** It is `cores ÷ ops/s` — CPU spent per
operation across both sides — and it is the column other agents' builds cannot
move, because a process that is given half as much of the machine does the same
work half as fast and spends the same CPU doing it. The `ops/s` column on its
own cannot tell a slower head node from a busier box; `cpu/op` can. Runs were
taken at machine load between 3 and 9; the tables below are the median of five
1.2 s windows per point with the range beside them.

#### The empty RPC, which nothing else can beat

| clients | ops/s | [min – max] | p50 | p99 | cores | srv | cli | cpu/op |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | 6,824 | [6,519 – 9,577] | 133 µs | 288 µs | 1.19 | 0.41 | 0.75 | 175 µs |
| 2 | 14,057 | [12,699 – 14,990] | 132 µs | 359 µs | 1.67 | 0.67 | 1.00 | 119 µs |
| 4 | 22,730 | [19,578 – 24,386] | 151 µs | 766 µs | 1.87 | 0.85 | 1.02 | 82 µs |
| 8 | 34,266 | [33,809 – 34,854] | 210 µs | 717 µs | 2.78 | 1.22 | 1.56 | 81 µs |
| 16 | 40,690 | [37,244 – 41,968] | 363 µs | 1.13 ms | 2.86 | 1.24 | 1.62 | 70 µs |
| 32 | 38,799 | [35,066 – 45,214] | 733 µs | 2.70 ms | 2.41 | 0.99 | 1.42 | 62 µs |
| 64 | 51,327 | [50,888 – 52,686] | 1.16 ms | 2.89 ms | 3.43 | 1.38 | 2.04 | 67 µs |
| 128 | **53,693** | [50,987 – 54,260] | 2.20 ms | 5.74 ms | 3.60 | 1.45 | 2.15 | 67 µs |

#### Point read, streaming query, and write

`Get` by primary key; a ten-row streaming query, which is the shape that spawns
a task, hands the routing result back over a `oneshot` and pushes rows through
the `mpsc::channel(2)`; and a one-row autocommit insert at
`Durability::Visible`.

| clients | get ops/s | get p50 | get p99 | get cpu/op | stream ops/s | stream p50 | stream p99 | stream cpu/op |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 5,246 | 187 µs | 277 µs | 246 µs | 3,478 | 278 µs | 414 µs | 366 µs |
| 2 | 11,252 | 169 µs | 342 µs | 174 µs | 6,533 | 290 µs | 519 µs | 323 µs |
| 4 | 17,068 | 215 µs | 552 µs | 155 µs | 11,128 | 333 µs | 820 µs | 242 µs |
| 8 | 25,556 | 284 µs | 870 µs | 115 µs | 17,229 | 420 µs | 1.24 ms | 173 µs |
| 16 | 26,445 | 522 µs | 2.05 ms | 97 µs | 19,605 | 722 µs | 2.19 ms | 168 µs |
| 32 | 25,843 | 1.08 ms | 4.53 ms | 88 µs | 21,706 | 1.33 ms | 3.61 ms | 160 µs |
| 64 | 30,084 | 1.89 ms | 7.13 ms | 84 µs | 24,717 | 2.40 ms | 5.85 ms | 143 µs |
| 128 | **32,627** | 3.44 ms | 10.72 ms | 88 µs | **25,953** | 4.59 ms | 11.15 ms | 142 µs |

| clients | write ops/s | [min – max] | p50 | p99 | cores | cpu/op |
|---:|---:|---|---:|---:|---:|---:|
| 1 | 4,484 | [4,013 – 4,620] | 202 µs | 417 µs | 1.11 | 247 µs |
| 2 | 6,106 | [5,873 – 6,692] | 283 µs | 838 µs | 1.37 | 225 µs |
| 4 | 8,813 | [8,560 – 9,003] | 374 µs | 2.70 ms | 1.51 | 171 µs |
| 8 | 10,945 | [10,708 – 11,827] | 620 µs | 3.29 ms | 1.58 | 144 µs |
| 16 | 20,340 | [20,120 – 21,095] | 733 µs | 1.83 ms | 3.23 | 159 µs |
| 32 | 21,916 | [21,680 – 22,689] | 1.36 ms | 3.18 ms | 3.35 | 153 µs |
| 64 | **22,193** | [21,140 – 22,764] | 2.72 ms | 5.64 ms | 3.30 | 149 µs |
| 128 | 21,398 | [20,359 – 22,478] | 5.84 ms | 11.24 ms | 3.24 | 152 µs |

**Zero errors at every point of every table, up to 128 concurrent clients.**
No refusal, no timeout, no dropped stream, no lost row: every `get` asserted it
found its row, every stream asserted it received exactly ten, and every insert
asserted it wrote one.

#### Where the box becomes the bottleneck, and how to tell

At the top of each table `cpu/op` has gone flat: **67 µs** for an empty RPC,
**88 µs** for a `get`, **142 µs** for a ten-row stream, **152 µs** for a write.
It stops falling by about eight clients — below that, per-operation cost is
dominated by a runtime that is mostly idle — and from there it does not rise.
*Nothing inside the head node gets more expensive as callers are added.*

What does happen is that the process runs out of cores.

`cpu/op` is *defined* as `cores ÷ ops/s`, so "cores divided by cpu/op gives the
throughput" is arithmetic and not a finding — it would be true of any numbers
at all. What is not arithmetic is that **the left-hand side stops moving**: once
`cpu/op` is flat, every remaining change in throughput is a change in how many
cores the process was granted, and none of it is a change in what the work
costs.

The clearest instance is in the empty-RPC table, where the run at 32 clients
looks like a regression and is not:

| clients | ops/s | cores the process got | cpu/op |
|---:|---:|---:|---:|
| 16 | 40,690 | 2.86 | 70 µs |
| 32 | **38,799** | **2.41** | 62 µs |
| 64 | 51,327 | 3.43 | 67 µs |

Throughput fell by 5% between sixteen clients and thirty-two, and the cost of
the work fell too. What fell furthest was the share of the machine this process
was given, from 2.86 cores to 2.41, because something else on the box wanted
them. Reading the ops/s column alone, that row is a scalability cliff. Reading
the other two, it is a build starting.

At 128 clients the four workloads were given 3.60, 2.88, 3.68 and 3.24 cores of
the four on the machine, so between 72% and 92% of it, with the load generator
taking about 40–60% of that.

So the answer to "past what concurrency am I measuring the box" is: **past
about 8 clients for a point read, and about 16 for a stream or a write.** Above
those, throughput moves by less than 30% while p50 grows in direct proportion
to the client count — a `get` goes from 284 µs at eight clients to 3.44 ms at
128, twelve times, for 1.28× the throughput. That is queueing, and on this
machine the queue is for a core rather than for anything in `slate-server`.

The `cli` column says the rest of it plainly: **the load generator uses between
a third and three-fifths of every core the process gets** — 2.15 of 3.60 on the
empty RPC. A real client one network hop away would not be spending those, so
these ceilings are floors for what the same head node would do with the box to
itself, and no attempt is made here to say by how much.

#### Concurrent durable commits share a flush

This is the one where the answer could have gone either way, and the difference
between the two is a factor of sixty-four.

| clients | ops/s | p50 | p90 | p99 | worst | cpu/op |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 10 | 101.25 ms | 102.83 ms | 106.58 ms | 106.58 ms | 2.08 ms |
| 2 | 20 | 101.07 ms | 101.88 ms | 103.73 ms | 103.85 ms | 892 µs |
| 4 | 40 | 101.18 ms | 101.96 ms | 103.73 ms | 103.84 ms | 536 µs |
| 8 | 79 | 101.19 ms | 102.05 ms | 104.60 ms | 104.99 ms | 319 µs |
| 16 | 158 | 101.28 ms | 102.67 ms | 104.03 ms | 105.81 ms | 238 µs |
| 32 | 316 | 101.15 ms | 102.66 ms | 106.90 ms | 108.10 ms | 192 µs |
| 64 | 633 | 101.10 ms | 102.20 ms | 103.07 ms | 104.17 ms | 164 µs |

**Throughput is exactly ten times the client count and latency does not move.**
Sixty-four writers each see the same 101.1 ms a single writer sees, and between
them get 633 durable commits a second. The p99 rises from 106.6 ms to 103.1 ms
— that is, it does not rise; the numbers are inside each other's noise at every
row.

That settles the question section 3 could only pose. A durable commit costs one
`flush_interval` of *waiting*, and waiting is shareable: every commit that
arrives inside the same 100 ms window rides the same WAL flush out. The 100×
gap between writing a hundred rows in one commit and a hundred rows one at a
time is about **round trips**, not about the flush — and a deployment that
cannot batch its writes can get the same effect by having a hundred callers.

The measurement was run three times over ninety minutes and produced
10 / 20 / 40 / 79 / 158 / 316 / 633 every time, with a single 632 in place of a
633 in one of them — which is one commit a second out of six hundred.

#### The write sweep, and a hypothesis withdrawn in the middle of writing it

The write table above says "a fresh database at every point" for a reason. The
first version of the sweep ran ascending on one database, and produced this:

| clients | 1 | 2 | 4 | 8 | 16 | 32 | **64** | **128** |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| ops/s, ascending, one database | 3,947 | 7,347 | 12,481 | 18,982 | 25,058 | 22,344 | **9,982** | **9,887** |
| cpu/op | 325 µs | 280 µs | 213 µs | 157 µs | 128 µs | 150 µs | **382 µs** | **387 µs** |

Read on its own that is a clean finding: write throughput peaks at sixteen
concurrent writers and collapses by 2.5× at sixty-four, with CPU per write
rising to match — contention in the write path, exactly where the brief said to
look for it. It was drafted.

Then the same sweep run **descending**, on the table the ascending pass had
left behind:

| clients | 128 | 64 | 32 | 16 | 8 | 4 | 2 | 1 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| ops/s, descending | 5,886 | 5,129 | 5,452 | 5,589 | 7,856 | 2,643 | 1,752 | **1,258** |
| cpu/op | 382 µs | 385 µs | 356 µs | 410 µs | 457 µs | 729 µs | 736 µs | **815 µs** |

A *single* writer, on the big table, costs 815 µs of CPU per row and manages
1,258 a second — against 4,484 a second and 247 µs on a fresh one. There is no
concurrency in that row at all. The collapse was **the table growing under the
benchmark**: the sweep writes about six hundred thousand rows across its own
levels, so the widest level always runs against the largest and most
un-compacted database.

Rebuilding the fixture between points removes it, and what is left is the table
printed above: **write throughput rises to about 22,000 a second by sixteen
clients and is flat from there to 128, with `cpu/op` flat at ~150 µs.** There is
no write-path contention to find in this range.

The lesson is one this document has now learned twice in different clothes.
`scan_tuning` measures forward and reversed "so a warming server cannot be
mistaken for a faster plan"; the same trick, applied to a benchmark whose
workload *is* mutation, turned a finding into an artefact. A sweep that changes
the fixture as it runs has to be run in both directions before any of it is
believed.

#### At and past `max_transactions`

Sixty-four clients looping on begin → insert → commit against a node capped at
sixteen open transactions, for one 1.2 s window:

| | |
|---|---:|
| accepted | 2,604 |
| refused with `ResourceExhausted` | 21,199 |
| failed some other way | **0** |
| mean latency of an accepted `Begin` | 3.32 ms |
| mean latency of a **refused** `Begin` | **2.45 ms** |

**The refusal is cheaper than the acceptance, which is the whole question.**
`Sessions::begin` checks the registry before it spawns anything, so a client
past the limit is told so rather than queued behind one that got in; the 89% of
attempts that were refused cost less each than the 11% that succeeded. Nothing
hung, nothing timed out, and no attempt failed in any other way.

And the cost of transactions that are simply *open* — each one a task, an
`mpsc::channel(1)` and a pinned snapshot — measured against point reads from
four other clients:

| open transactions | ops/s | p50 | p90 | p99 |
|---:|---:|---:|---:|---:|
| 0 | 13,427 | 257 µs | 446 µs | 867 µs |
| 64 | 13,528 | 253 µs | 445 µs | 1.12 ms |
| 512 | 13,215 | 259 µs | 456 µs | 1.39 ms |
| 1,023 | 12,536 | 272 µs | 489 µs | 1.31 ms |

**A thousand and twenty-three idle transactions cost 6% of read throughput and
15 µs of median latency**, which is inside this harness's run-to-run spread.
The task-per-transaction design is not paying for itself in the steady state.
With 1,024 open the next `Begin` was refused in 496 µs.

#### A slow consumer, and an abandoned one

A **slow** consumer stops draining its stream; `mpsc::channel(2)` means the
server's scan task parks after two batches. Eight of those, then thirty-two,
held open against a whole-table scan while four other clients work.
The write baseline is measured *first* here, before any stalled stream exists,
because the writes themselves grow the table:

| | ops/s | p50 | p90 | p99 |
|---|---:|---:|---:|---:|
| point reads, nothing else running | 13,550 | 246 µs | 437 µs | 1.47 ms |
| point reads, 8 stalled streams | 14,015 | 235 µs | 427 µs | 1.21 ms |
| point reads, 32 stalled streams | **5,899** | 278 µs | **1.74 ms** | **6.06 ms** |
| writes, nothing stalled | 9,870 | 346 µs | 588 µs | 1.58 ms |
| writes, 8 stalled streams | **4,364** | 874 µs | 1.38 ms | 2.36 ms |
| writes, 32 stalled streams | **4,229** | 811 µs | 1.40 ms | 4.59 ms |

**Eight parked stream consumers halve write throughput. Thirty-two also halve
read throughput, and put 4× on the read p99 while leaving the p50 alone.**

What it is *not* is a block: nothing deadlocked, no request failed, and a point
read's median never moved. What it is, read off the code rather than measured
separately, is that each parked scan is holding an open snapshot — and with no
replicas configured, `Freshness::Any` routes to the writer, so all thirty-two
of those snapshots are pinned on the writer that the inserts are also using. A
deployment with replicas would put them somewhere else. That attribution is a
reading of `service.rs` and `pool.rs`, not a measurement, and the number that
is measured is the 2.3× on writes.

An **abandoned** consumer drops the stream entirely, and `Scan::run` is written
to return when its send fails — "an abandoned scan should stop reading object
storage". Whether it does cannot be seen from the client side at all, so it is
counted in server CPU over a fixed 600 ms window, eight whole-table scans of
88,000 rows each:

| | server CPU over 600 ms |
|---|---:|
| nothing running | 0.002 core-seconds |
| 8 scans drained to the end | **1.935 core-seconds** |
| 8 scans abandoned after one batch | **0.016 core-seconds** |

**An abandoned scan costs 0.8% of a completed one.** A head node that ignored
the hang-up would have spent the same 1.9 core-seconds either way, and it spends
1/120th. The back pressure works and so does the cancellation.

#### A lease renewal is still invisible, now with something to see

Section 5 asked this one request at a time against an in-memory object store,
where a renewal is 756 ns, and found nothing — which is not much of a test. The
sharper version puts eight clients under load and gives the renewal something
to stall *with*: a lease whose conditional PUT takes 15 ms, which is at the fast
end of what S3 does, renewing every 100 ms.

| arm | ops/s | p50 | p90 | p99 | worst | renewals in the arm |
|---|---:|---:|---:|---:|---:|---:|
| no renewal at all | 17,956 | 353 µs | 629 µs | 3.19 ms | 24.68 ms | — |
| renew every 20 ms, in-memory PUT | 19,583 | 331 µs | 596 µs | 2.14 ms | 21.14 ms | 238 |
| renew every 100 ms, **15 ms PUT** | 16,700 | 448 µs | 671 µs | **1.65 ms** | **20.42 ms** | 42 (slowest 19.18 ms) |

**Forty-two renewals, each taking fifteen to nineteen milliseconds, are not
visible anywhere in the distribution** — the arm that has them has the *lowest*
p99 and the *lowest* worst sample of the three. The renewal count is printed
because an arm that quietly renewed zero times would be the first arm wearing a
different label; 42 over 4.5 s at a 100 ms cadence is what it should be, and 238
is what the 20 ms arm should give.

The reason is that `Leadership::renew` awaits the store and holds only a
`tokio::sync::Mutex` that no request path touches; the write path's own check is
a watch-channel read at 17 ns. That is read off `leadership.rs`; what is
measured is that a 19 ms stall inside the renewal reaches no caller.


### 7. Above 200,000 rows: what could be measured, and what stopped it

`slate-slatedb`'s `cost_calibration` measured the two constants the planner
rests on against a real S3 server at **200,000 rows**, and nothing above that
had been measured. `examples/cost_at_scale.rs` is the same fixture with three
things added: several sizes, request counts taken **cold as well as warm**, and
point reads measured in bulk over a pseudo-random walk of the keyspace rather
than one at a time.

**First, that it is the same fixture.** At 200,000 rows this example reproduces
the calibration's decisive measurement in the unit that travels: forcing the
contested query through the index costs **1,223 GETs** here against the
**1,217 requests** [`correctness.md`](correctness.md) records — half a percent
apart, and the same number in two runs an hour apart. The wall time is 4.3 s
against the 3.6 s recorded, which is this machine being busier and is why the
request count is the one quoted. The table scan the planner actually picks
costs 19–23 GETs and 0.5–0.7 s. Nothing below is being compared against a
different fixture.

#### The calibration was not measuring a warm cache

This was a live worry and it turns out to be unfounded, which is worth a line
because the alternative would have invalidated both constants.
`cost_calibration` runs `analyze` — a full read of the table — before it
measures anything, so every number it published was taken against a block cache
that had just seen the whole database. If the cache were doing the work, the
constants would describe a hit ratio rather than storage.

| 200,000 rows | before `analyze` | after `analyze` |
|---|---:|---:|
| GETs per point read | 3.54 / 3.57 | 3.40 / 3.41 |
| rows per GET, full scan | 9,524 / 8,000 | 9,524 / 10,526 |

Two independent runs, cold and warm, and the difference between the columns is
smaller than the difference between the runs. **The cache is not what the
calibration measured.**

**Superseded: it is not the cache being warm, it is the cache being *partly*
warm.** Re-measured later, the same full scan of the same 200,000 rows costs
whatever the reads before it left behind:

| the store has already | GETs | rows/GET |
|---|---:|---:|
| read nothing at all | 53 | 3,774 |
| served 200 random point reads | 204, 208, 205 | 980 |
| served `analyze` and 400 of them | 369 | 542 |

Three consecutive scans give the middle row, so this is not "the first one
paid and the rest are free". A *partially* populated block cache fragments a
scan into many small ranged reads instead of a few large ones, and more of it
fragments it further — which is why a scan measured after `analyze` costs
*more* than one measured before, an ordering no cache can produce and the
thing that gave this away.

`SCAN_ROW_COST` says 8,000 rows per request, which is none of the three. It is
what a scan costs when the cache already holds the whole table.

This also explains a disagreement that looked like a defect: `cost_calibration
--cold` measures that scan at 58 requests and `cost_at_scale` at 205, on a
byte-identical fixture. Both are right — the first scans a store that has done
nothing, the second one that has just walked 200 random keys.

#### The two constants at 200,000 rows

| | the model says | measured |
|---|---:|---:|
| `SCAN_ROW_COST` → rows per request on a scan | 8,000 | **8,000 – 10,526** |
| `POINT_READ_COST` → requests per point read | 3.0 | **3.40 – 3.57** |

~~The scan constant is right, on the conservative side of right. The point-read
constant is **13–19% low** — a read really costs about three and a half
requests, not three. Both errors point the same way (the model slightly
under-charges reads relative to scans, which makes an index look marginally
better than it is), and at this scale neither is close to changing a decision:
the plan the model rejects it rejects by a factor of 46 in cost and 58 in
requests. Adjusting `POINT_READ_COST` from 3.0 to 3.5 is not proposed here,
because it is a 17% change to a constant whose own spread across two runs is 5%
and which is a property of the row width and the deployment rather than of
anything in this repository — the same argument `SCAN_ROW_COST` was left alone
under, one section up.~~

<!-- not a cost-model claim -->

**Both halves withdrawn.** The point-read figure does not reproduce: four
measurements across three benchmarks and two fixtures give 1.02, 1.09,
1.16/0.96 and 1.015 requests per row, and `POINT_READ_COST` is now **1.0** —
see [`correctness.md`](correctness.md). The scan figure is not a constant at
all: it is 3,774 rows per request on a store that has read nothing, 980 after
200 random point reads and 542 after `analyze`, per the table above.
`SCAN_ROW_COST` is unchanged at 8,000 rows per request, which is the
fully-cached case, because choosing among three measurements eight times apart
is a modelling decision and not a calibration.

The sentence that has aged worst is "at this scale neither is close to changing
a decision". `cost_at_scale`'s own verdict line now reads **chose TableScan but
Index is faster — WRONG at this scale**: at 200,000 rows the index does 368
requests against the scan's 370 and finishes 11× sooner, while the model calls
it 15× worse.

#### Where this stops, and it is not the model

The intended measurement — the same constants at 600,000 and 1,200,000 rows —
**was not taken, because the fixture cannot be built at those sizes here.** The
loader is linear and then it is not:

| rows | load | per row | PUTs | GETs during load |
|---:|---:|---:|---:|---:|
| 100,000 | 2.6 s | 25.6 µs | 33 | 58 |
| 200,000 | 4.9 s | 24.6 µs | 54 | 62 |
| 300,000 | 7.3 s | 24.5 µs | 75 | 74 |
| 400,000 | 9.8 s | 24.6 µs | 96 | 80 |
| 500,000 | 11.1 s | 22.2 µs | 118 | 84 |
| 600,000 | **did not finish in five minutes**, on three attempts | | | |

Twenty-two to twenty-six microseconds a row, dead flat over a factor of five,
and then something between five and six hundred thousand rows that this harness
cannot get past.

**It is reproducible and it is not attributed, and the difference matters.**
Three attempts at 600,000 rows and one at 1,200,000 all failed to finish
loading inside five minutes, from three different starting states — as the
second size in a sweep, as the sixth, and alone in a fresh process. Against
that:

- It is **not simply a busy machine**. The 400,000 and 500,000 rows above were
  loaded at machine load 8.6 in **9.9 s and 11.4 s**; on a quiet box at load 2.5
  the same two sizes took **9.8 s and 11.1 s**. The loader does not notice a
  busy box at those sizes.
- It **may be the disk**. This machine ran out of disk twice during these
  measurements — other agents were building on it throughout — and the
  in-process S3 server writes its bucket to a temporary directory on that same
  disk. Two of the four stalled attempts began within minutes of a
  `No space left on device`. That is not an alibi for all four, and it is not
  ruled out for any of them.

The shape of it is suggestive: these rows are about 110 bytes, so 600,000 of
them is roughly 66 MB against SlateDB's default 64 MB `l0_sst_size_bytes`, and
`insert_many`'s duplicate-key check is a read per row that costs nothing while
the memtable holds the table and costs object-store requests the moment it does
not. The `GETs during load` column exists to test exactly that. It is **zero
per row at every size that completes**, which is consistent with the story and
confirms nothing, because the size that would confirm it is the size that will
not finish.

So, plainly:

- **The cost model was not shown to break above 200,000 rows.** It was not
  shown to hold there either. What was measured is that it holds *at* 200,000,
  cold as well as warm, and that the shapes it ranks it still ranks correctly.
- **Something stops this fixture at about half a million rows**, and it is
  reproducible across four attempts and three starting states. Whether it is
  the write path or the disk under the S3 server is *not established*, and the
  first thing to do next is to run it on a machine with room and nothing else
  on it — which would settle it in twenty minutes and could not be done here.

Nothing here says the planner is wrong at a million rows, and nothing here says
a bulk load of a million rows is slow — only that this harness could not
perform one, four times, and could perform one of five hundred thousand rows in
eleven seconds every time it tried.


### 7b. The same question in memory, on the real NYC taxi month

Section 7 could not get the SlateDB-on-S3 fixture past about half a million
rows and could not attribute the wall. This does not settle that. It narrows
it, which is worth writing down because the narrowing was free — it fell out
of putting a real dataset behind the browser workbench.

**The whole month loads, in memory.** 2,964,619 real January-2024 yellow-taxi
trips — eleven columns, one secondary index — into `MemoryStore` through the
same `insert_many` the S3 fixture uses:

| rows | build `Row`s | `insert_many` | rate | `analyze` | RSS | bytes/row |
|---:|---:|---:|---:|---:|---:|---:|
| 100,000 | 0.1 s | 0.30 s | 329k/s | 0.16 s | 132 MB | 1,282 |
| 500,000 | 0.5 s | 3.49 s | 143k/s | 0.72 s | 643 MB | 1,278 |
| 1,000,000 | 0.3 s | 6.89 s | 145k/s | 1.39 s | 1,330 MB | 1,326 |
| 2,000,000 | 0.3 s | 14.71 s | 136k/s | 2.64 s | 2,607 MB | 1,301 |
| 2,964,619 | 0.5 s | 17.77 s | 167k/s | 4.08 s | 3,839 MB | 1,293 |

Release build, one process, 4 vCPU / 15 GB, nothing else running. Single run
per size, so treat the rate column as an order of magnitude and not a
measurement with a spread — the point of the table is the two things that do
not happen.

**Nothing stops at 600,000.** The write path takes six times that size without
complaint, and the per-row rate is flat to within the noise of a single run
from 500k up. So whatever section 7 hit is **not** `insert_many` itself and
not the duplicate-key check as a matter of algorithm. It is in the SlateDB or
object-store half — which is what that section's leading hypothesis said, and
this is the first evidence separating the two halves rather than reasoning
about them. The disk under the in-process S3 server remains the other
candidate and is still not ruled out.

**The in-memory representation costs about 1,290 bytes a row**, dead flat
across a factor of thirty. These rows serialise to roughly 110 bytes, so the
store is holding **twelve times** what the data weighs.

> **Withdrawn on 2026-09-15.** Both halves of that sentence are wrong. The
> RSS column above is the whole process, and the process is holding the source
> `Vec<Row>` the store was built from — 474 bytes a row of `Value`s and their
> `String`s, never released, and nothing to do with the store. Counted with an
> allocator instead of RSS, the store holds **491 bytes a row**, over keys and
> values that total **86**, which is 5.7× and not 12×. See §7c.

**What this is not.** It is not a throughput benchmark: one run per size, no
cold/warm separation, no comparison to anything. It is not the S3 path. It
does not show the cost model is right at three million rows — only that the
rows can be loaded and analysed there.

### 7c. Where the bytes a row actually go

§7b's 1,290 was RSS divided by rows, and it was never investigated. Both of
those turned out to matter. `slate-slatedb`'s `row_footprint` example counts
allocations instead — `dhat` reports the bytes each allocation *requested*, so
the numbers are properties of the program rather than of the machine — and
stages the run so each figure is a difference between two readings:

| | total | per row |
|---|---:|---:|
| source rows, before the store exists | 45.2 MB | 474 |
| + `insert_many` | 91.9 MB | 964 |
| + `analyze` | 92.0 MB | 964 |
| **− the source rows: the store itself** | **46.8 MB** | **491** |
| − the commit's conflict history | 46.8 MB | 491 |

100,000 trips, release build. **Nearly half of the old number was the source
`Vec<Row>`**, which the harness held for the whole run and RSS duly counted:
474 bytes a row of `Value` enums and the `String` in each. It is not what the
store costs, and no amount of staring at `BTreeMap` would have found it.

Against that, the floor:

| | per row |
|---|---:|
| keys and values, logical | 86 |
| the same pairs in a bare `BTreeMap<Bytes, Bytes>` | 219 |
| — of which inline `Bytes` headers | 128 |
| the same pairs, allocated the way the writer allocates them | 369 |
| the store | 491 |

So the store is **5.7× its data, not 12×**, and the largest single component is
not overhead anyone chose: 128 bytes a row is four `Bytes` structs sitting
inline in the map's nodes, at 32 bytes each, for two entries per row. That is
the price of the representation and it does not move without changing it.

**A hypothesis, tested and rejected.** `Shared::history` keeps a `BTreeSet` of
every key a commit wrote, for conflict detection, and a bulk load of 100,000
rows puts 200,000 keys in one. It looked like the obvious culprit. It is worth
**two allocations a row and no measurable bytes** — the `Bytes` in the set
share the map's buffers — and `trim_history` drops it on the next transaction
drop anyway.

**What was worth 88 bytes a row.** The gap between the exact map (219) and the
writer-shaped one (369) is capacity slack. `slate_tuple::encode` reserves nine
bytes a value and `keys` a header plus the same; `Bytes::from(Vec)` then adopts
the vector's **capacity**, not its length, and holds it for the life of the
entry. Shrinking at the one place a buffer becomes stored —
`MemoryTransaction::put` — takes the store from **491 to 403 bytes a row**.

It also made the load *faster*, which was not the expected direction:

| | 100,000 rows |
|---|---:|
| without the shrink | 0.29 s, six runs, no spread |
| with it | 0.26–0.28 s, six runs |

Alternating runs of two prebuilt binaries, ranges non-overlapping — smaller
blocks and less memory touched, presumably, though that is an explanation and
not a measurement. So this is not the usual space-for-time trade; nothing was
given up.

**Not the SlateDB path.** `slatedb`'s `put` takes `AsRef<[u8]>` and copies into
its own write batch, so it never adopts a caller's capacity. This is
`MemoryStore`'s alone — which is the store the browser workbench runs on, and
the one §7b measured.

`slate-slatedb/tests/footprint.rs` holds it there: a ceiling of 450 bytes a row
over the same sample, which the shrink passes at 403 and its absence fails at
487.

### A cost-model limitation the workbench made visible

An unfiltered `GROUP BY` over an indexed column plans as a **table scan**, not
an index-only scan, and the reason is a tie:

```
SELECT author_id, count(*) FROM books GROUP BY author_id
  -> Table Scan on books  (rows=4824 cost=1.60 decodes=[1])

SELECT author_id, count(*) FROM books WHERE author_id < 50 GROUP BY author_id
  -> Index Only Scan using by_author on books  (rows=565 cost=1.07 decodes=[1])
```

With a predicate the index has a range and wins. Without one, scanning 4,824
index entries and scanning 4,824 whole rows both cost `1.0 + 4824 × 0.000125 =
1.603`, and the tie goes to the table scan.

The costs are equal and **the work is not**. The index entries are one column;
the rows are four. On object storage that is the difference between fetching a
few hundred kilobytes and a few megabytes, and the cost model — which charges
per row and has no notion of row width — cannot see it.

Recorded rather than fixed. Charging for bytes touches every plan in this
document and every calibration in section 8, so it is a change to make
deliberately with the oracle suites watching, not one to slip in beside a
dataset swap. `a_grouped_scan_can_answer_from_the_index_alone` in
`crates/slate-wasm/tests/sql.rs` pins the current behaviour *and* the
arithmetic, so the day the tie stops being a tie, a test says so.


### What was boring, and is reported as boring

These were measured, came back with no difference, and are worth as much as the
findings:

- **Tenant-affinity routing against round robin**: 76 ns apart, 0.06% of a
  request.
- **Proving freshness against a replica that is already caught up**: 1.0 µs,
  inside noise. The token costs nothing when it does not have to wait.
- **A replica read against a writer read**: 1.8 µs, inside noise.
- **Lease renewal against serving reads**: inside noise at 250× the real
  renewal rate — and still inside noise under eight concurrent clients with a
  15 ms conditional PUT, which is the version of the question with something in
  it to find.
- **A thousand open transactions against the read path**: 6% of throughput and
  15 µs of median latency, inside the run-to-run spread.
- **Concurrency against the head node's own cost**: `cpu/op` is flat from eight
  clients to a hundred and twenty-eight on all four workloads. Nothing in the
  head node gets more expensive as callers are added; the box runs out of cores
  first, and the identity `cores ÷ cpu/op = ops/s` holds to within 1%.
- **Write concurrency against the write path**: flat at ~22,000 inserts a
  second from sixteen clients to a hundred and twenty-eight, once the fixture
  stops growing under the benchmark.

And one constant was examined and left alone: `rows_per_message = 256` is
inside the measured range at both ends — 32 at the bottom, about 1,000 at the
top — and the one argument for lowering it turned out to be a socket option.

### What could not be measured here

- **Real object-store latency**, anywhere. The in-memory store removes the
  network, which is most of what a lease renewal, a durable commit and a replica
  read cost in a bucket. Every conclusion above that touches storage is about
  request *counts* and *shapes*, not their wall time in production.
- **A remote client.** Client and server share four cores and a loopback
  socket, so the 130 µs transport term includes the client stub and excludes the
  network.
- **A head node with the machine to itself.** Section 6 gets to 128 concurrent
  clients, but the load generator spends between a third and three-fifths of
  every core the process is given, and other builds were on the box throughout.
  The ceilings there are lower bounds on what the same head node does with a
  real client elsewhere, and nothing here says by how much.
- **Concurrency above 128 clients**, and concurrency against a *replica* fleet:
  every read in section 6 routes to the writer, because no replicas are
  configured. The slow-consumer finding in particular would look different with
  somewhere else for those snapshots to be pinned.
- **Whether the stalled-stream cost is the pinned snapshot.** Eight parked
  stream consumers halve write throughput; that is measured. The attribution to
  the snapshot each one holds on the writer is read off `service.rs` and
  `pool.rs` and is not measured separately.
- **The residual ~250 µs rise** in first-row latency across a batch of about
  125 once `TCP_NODELAY` is set. It is consistent with the per-row cost of a
  larger batch and it is at the edge of this harness's resolution, and neither
  of those is a demonstration.
- **The cost model above 200,000 rows**, which is what section 7 set out to
  measure. The fixture could not be built past about 500,000 rows on this
  machine, so the constants at a million rows remain unmeasured.
- **Why the fixture could not be built.** Four attempts at 600,000 rows or more
  stalled; the loader is linear and indifferent to machine load up to 500,000;
  and the disk under the in-process S3 server hit zero twice during the
  session. Scale and disk are not separated, and a machine with room would
  separate them in twenty minutes.

## The dependency defaults sweep

`TCP_NODELAY` was found by reading `tonic`'s documentation, not by profiling:
a library default that does not apply on the path this code takes. That is a
shape, not an incident, so the same question was put to every dependency this
project configures — `tonic`, `slatedb`, `object_store`, `tokio`, `hyper`,
`reqwest` and `prost`. Read what the repository sets; then read what the
library does with it on the path the repository takes.

Three answers came back. One is a capability switched off by a feature flag,
one is the same Nagle bug one layer down and inside the only wall-clock
numbers in this document that are not taken against a latency model, and one
is a setting the benchmark configures and the shipping daemon cannot.

```sh
cargo run --release -p slate-headbench --example cache_probe
cargo run --release -p slate-headbench --example s3_nodelay
HEADBENCH_POLL_MS=10000 cargo run --release -p slate-headbench \
    --example head_report -- routing
```

### 8. SlateDB's block cache is compiled out, and a point read pays three GETs

**Fixed.** `slate-slatedb` now has a `cache` feature (`slatedb/foyer`) and it is
on by default. Re-measured afterwards, on the same probe: the *as shipped*
arm moved from 603 cold GETs and 3.02 warm to 205 and **0.00**, which is where
the *cache on* arm already sat, while *cache off, said so* stayed at 603 and
3.02 as the control. The arm that was broken is now indistinguishable from the
arm that was correct, and the arm that asks for no cache still gets none — so
the change did what it claims and nothing more.

The original finding follows.

Every crate here declares `slatedb = { version = "0.16", default-features =
false }`, and `slate-slatedb` re-enables one feature, `aws`. SlateDB's own
`default` is `["aws", "foyer"]`. `cargo tree -i -p slatedb -e features`
confirms what is left: the `aws` feature and nothing else.

That matters because of how SlateDB installs its cache. `DbBuilder::new` calls
`default_db_cache()`, which builds a `SplitCache` over `default_block_cache()`
and `default_meta_cache()` — and **both of those return `None` unless `foyer`
or `moka` is compiled in**. A `SplitCache` with two empty halves is not an
absent cache, which would be obvious; it is a present one that answers
`Ok(None)` to every lookup and discards every insert. Nothing fails. The reads
just all go to object storage.

This is the readahead finding again, and worse in one respect: readahead was a
default we passed, and this is a capability a feature flag removed.

20,000 rows, written, closed and reopened over an in-memory object store
wrapped in a counting store, 21 runs, on a quiet box (load 1.3):

| phase | arm | cold GETs | GETs/op, warm | wall clock |
|---|---|---:|---:|---:|
| point read, spread | **as shipped (default)** | 603 | **3.02** | 117.52 µs [115.89 – 128.09] |
| point read, spread | cache off, said so | 603 | 3.02 | 116.32 µs [112.79 – 133.93] |
| point read, spread | **cache on** | 205 | **0.00** | **72.13 µs** [70.51 – 74.88] |
| point read, one key | as shipped (default) | 600 | 3.00 | 116.10 µs [113.09 – 148.49] |
| point read, one key | cache on | 3 | 0.00 | 69.34 µs [67.94 – 72.88] |
| full scan of 20,000 | as shipped (default) | 2 | 2.00 | 34.79 ms [34.12 – 36.17] |
| full scan of 20,000 | cache on | 2 | 1.00 | 34.90 ms [34.38 – 35.75] |

<!-- not a cost-model claim -->

**A point read costs three object-store GETs, every time, for ever.** With a
cache installed it costs none once the metadata is loaded. The GET column
does not care what else is on the machine: the same four arms re-run at load
6.8 give the same 3.02 / 3.02 / 0.00 / 0.00. The clock column does, which is
why it is the second column and not the first.

Two controls, and the first one is the whole argument. The `as shipped
(default)` arm calls `Db::builder(..).build()` with no cache method at all —
the exact path `SlateStore::open` takes — and the `cache off, said so` arm
calls `with_db_cache_disabled()`. They are indistinguishable: identical GET
counts, and 1.20 µs and 2.22 µs apart on the clock, inside noise both times.
That is the demonstration that the cache SlateDB installs by itself is doing
nothing in this build. The second control is the cold full scan: **2 GETs in
every one of the four arms**, because 20,000 small rows arrive in two 1 MiB
readahead fetches whatever the cache does.

The mechanism is the probe cache's own hit counters, and it says exactly which
three GETs those are. On the cold spread-out pass with a cache: **blocks 0/203
hit, metadata 398/400 hit**. Two hundred reads touched two hundred different
blocks — no block hit, and none should. But they consulted the *same two*
metadata objects four hundred times and missed twice: one SST index and one
filter, fetched once each. Without a cache those two objects are re-fetched on
every single read. That is two of the three GETs, and they are pure waste.

The wall-clock column is a lower bound and is stated as one: an avoided GET
here is a memcpy against an in-memory store, and in a bucket it is a round
trip. On this box removing three GETs was worth 1.63×; against S3 at 15 ms a
GET it is the difference between one round trip and three, warm, and between
nothing and three, hot.

**What this does not show.** The cache in the measurement is an unbounded
`HashMap` written for the experiment, so it is the *ceiling* on what a real
cache buys — no eviction ever costs it a hit. `foyer`'s default is 512 MiB of
blocks and 128 MiB of metadata, and this fixture is about 4 MB, so it fits in
either; on a working set that does not fit, the block half of this result
shrinks and the metadata half does not. The unbounded map was chosen over
enabling `foyer` for the measurement because it changes exactly one variable
and needs no second build.

**The fix**, and it is a manifest change rather than a code change: stop
passing `default-features = false` to SlateDB, or pass `features = ["foyer"]`
alongside it. `crates/slate-slatedb/Cargo.toml` is the place — it already
forwards `aws = ["slatedb/aws"]`, so a sibling `cache = ["slatedb/foyer"]` in
its `default` set gives every dependent the cache back without any of them
knowing. The same line appears in `crates/slate-serverd/Cargo.toml`,
`crates/slate-headbench/Cargo.toml` and `crates/slate-server`'s
dev-dependencies, all of which are pinned to the same version so that
`Arc<dyn ObjectStore>` stays one type; features unify across them, so one of
them enabling `foyer` is enough to turn the cache on everywhere.

### 8b. The same measurement with the cache on, at `--release` — and the first table here that says what built it

Finding 8 was measured on a build with SlateDB's block cache compiled out, and
the `3.0` it produced sat in `POINT_READ_COST` for nine tasks. #278 fixed the
manifest. Nothing had since re-run the calibration on a build with the cache
**on**, at `--release`, and recorded the result beside the build that produced
it. This is that run.

```
build: slate-slatedb 0.0.1 | features aws, cache | off dhat-heap | release (opt-level 3, debug false) | x86_64-unknown-linux-gnu | slatedb 0.16.0, foyer 0.22.3, object_store 0.14.1, tokio 1.53.1
```

`cargo run --release -p slate-slatedb --example cost_calibration`, three runs,
200,000 rows through SlateDB over the in-process `s3s` server. The spread is
across those three runs; where one figure is given it was identical in all
three.

| query | rows | predicted cost | GETs | rows per GET |
|---|---:|---:|---:|---:|
| point get by primary key | 1 | 1.0 | 19 – 25 | — (see below) |
| index equality, one bucket | 400 | 26.0 | 33 – 38 | — |
| narrow key range | 1,000 | 1.0 | 1 | 1,000 |
| wide key range | 50,000 | 7.1 | 10 | 5,000 |
| full scan | 200,000 | 26.0 | 20 – 21 | **9,524 – 10,000** |

<!-- not a cost-model claim -->

**Both constants hold.** The decisive arm is the forced index scan, which does
400 primary-key reads and served **408 GETs in every one of the three runs** —
1.02 requests per row, against `POINT_READ_COST = 1.0`. The full scan returns
9,524–10,000 rows per request, against `SCAN_ROW_COST`'s 8,000; that is inside
the 8,000–10,526 band already recorded above, and `SCAN_ROW_COST` is left
alone for the reason given there.

**The point-get row is warm-up, not the cost of a point read.** It is the
first query issued against a freshly loaded store, so it pays for the manifest
and the SST metadata the four rows under it then reuse — which is why it is
the only row that moves between runs (19, 25, 19) while every other GET count
is identical or within one. Read as a point read's cost it says 22; the
forced-index arm in the same process says 1.02, and the forced-index arm is
the one doing 400 of them. This is the confound #269 recorded, still present,
now bounded: it lives in one row and the run says which.

**The clock is not comparable with anything on this page.** 10.665, 10.930 and
10.860 ms per GET, against a loopback `s3s`; `docs/performance.md` uses 2.2 ms,
a wide-area figure. The GET counts are what transfers.

### 8c. Doubling the table does not double the requests — a scan gets cheaper per row as it grows

Every cost measurement on this page until now was taken at 200,000 rows, and
the README has carried "the constants at a million rows are still unmeasured"
for as long as that has been true. The loader cliff between 500,000 and 600,000
rows still blocks the million. **400,000 fits**, and it is the first second
point this model has ever had.

```
build: slate-slatedb 0.0.1 | features aws, cache | off dhat-heap | release (opt-level 3, debug false) | x86_64-unknown-linux-gnu | slatedb 0.16.0, foyer 0.22.3, object_store 0.14.1, tokio 1.53.1
```

`cargo run --release -p slate-slatedb --example cost_at_scale`, with
`SCALE_ROWS`. Two runs at 400,000; one at 200,000, which reproduces the
three-state table recorded above (53 / 205 / 369 there, 51 / 199 / 365 here).

| the store has already | 200,000 rows | 400,000 rows | requests, for 2× the rows |
|---|---:|---:|---:|
| read nothing at all | 51 GETs | 69 – 77 | **1.35 – 1.51×** |
| served 200 random point reads | 199 – 201 | 228 | **1.14×** |
| served `analyze` and 400 of them | 365 | 406 | **1.11×** |

Per row, in the same three states: 3,922 → 5,195 – 5,797 rows/GET; 1,005 →
1,754; 548 → 985.

**A scan gets more efficient per row as the table grows**, in every cache
state, and the effect is largest in the states a real deployment is actually
in. Nothing here predicted that — the cost model charges a scan strictly
linearly in rows, so it is the one direction the model cannot express.

The stability is worth stating because it is what makes the comparison
readable: `full scan, three times` printed `[228, 228, 228]` in both 400,000
runs, and the two probe-state rows are **identical across runs**. The pristine
scan is the only figure that moves at all (69 against 77), and it is the one
measured before anything has touched the store.

Cold point reads hold across the scale change: **1.14, 1.19 and 1.16 requests
per read** at the two sizes, against `POINT_READ_COST`'s 1.0 — an independent
confirmation of [§8b](#8b-the-same-measurement-with-the-cache-on-at-release--and-the-first-table-here-that-says-what-built-it)'s
1.02, from a different example, a different fixture size and a colder store.

**This does not move `SCAN_ROW_COST`.** It sharpens the reason not to: the
constant's docstring says there is no single value because the cost depends on
a cache state the model has no input for, and there is now a second axis it
has no input for either. A model that charged scans sub-linearly would fit
these six numbers better and would still be guessing at the cache.

### 9. The in-process S3 server had Nagle on too, and it is inside the readahead table

**Fixed**: `crates/slate-slatedb/tests/common/s3server.rs` sets `TCP_NODELAY`
on each accepted connection. The correction to the readahead table below
stands — that table's wall-clock ratios were measured through this socket and
the real figure is about 8x, not 158x-409x. The 31x request-count ratio, which
is what that section says is the number worth quoting, is unaffected: request
counts do not care about ACKs.

The original finding follows.

`crates/slate-slatedb/tests/common/s3server.rs` binds its own `TcpListener`,
accepts a `TcpStream`, and hands it to
`hyper_util::server::conn::auto::Builder::serve_connection`. Nothing on that
path sets `TCP_NODELAY`. Unlike `tonic::transport::Server` there is not even a
default being ignored: hyper does not own the socket and never touches its
options, and only the *client* side of `hyper-util` sets nodelay.

That server is under `scan_tuning`, `cost_calibration` and `cost_at_scale` —
which is to say under every wall-clock number in this document that is *not*
taken against a latency model.

Two servers, identical but for `set_nodelay(true)` on each accepted
connection, each with its own storage and its own SlateDB. Same 20,000-row
fixture as `scan_tuning`, scanned end to end, 5 runs, arms ordered
Nagle/nodelay/nodelay/Nagle so a machine getting busier cannot be read as a
slower socket:

| | S3 GETs | wall clock | delayed ACKs/scan |
|---|---:|---:|---:|
| Nagle on, 1 MiB readahead | 59 | 183.80 ms [150.01 – 218.71] | 11 |
| `TCP_NODELAY`, 1 MiB readahead | 59 | **103.02 ms** [99.84 – 105.12] | 4 |
| `TCP_NODELAY`, SlateDB defaults | 1231 | **827.25 ms** [674.25 – 829.30] | 5 |
| Nagle on, SlateDB defaults | 1377 | **47.14 s** [39.36 – 52.00] | **1127** |

**A scan at SlateDB's default readahead went from 47.14 s to 827 ms — 57×.**
At the readahead this project actually ships it is 183.80 ms against 103.02 ms,
1.78×, which is the same effect with two orders of magnitude fewer writes to
stall on.

The last column is the mechanism, and it is the same counter that settled the
head node's socket. 1,127 expiries of the kernel's delayed-acknowledgement
timer per scan, against 1,377 requests — call it one stall per request. Divide
the wall clock by the requests and the Nagled arm costs **34.2 ms per GET**,
which is not a number any code path produces; it is the delayed-ACK timer.
With the socket fixed the same counter reads 5 per scan.

**Run twice.** A second pass, three runs, on a different machine load:
44.66 s against 846.09 ms (52.8×) with 1,042 delayed ACKs against 6, and
169.31 ms against 86.29 ms (1.96×) at 1 MiB readahead with 59 GETs on both
sides. The effect is not a state the box was in.

**The control moved, and it should be said plainly.** The request count is
supposed to be the control — a socket option cannot change how many blocks a
scan reads — and it went from 1231 to 1377, 12% more. That is background work:
a scan that takes 47 seconds accumulates manifest polls and compactor checks
that a scan taking 0.8 seconds does not. It is an effect of the slowness, not
a cause, and 12% more requests does not make 57×.

**This retro-corrects the readahead table above.** Its "SlateDB defaults" row
reads 1372 GETs and 51,915 ms; this reproduces it (1377 GETs, 47.1 s) and shows
that ~34 ms of each of those requests was a socket. With `TCP_NODELAY` the same
comparison is 827 ms against 103 ms: **about 8×, not the 158×–409× that table
quotes.** The 31× *request-count* ratio is untouched, which is exactly what
that section said the number worth quoting was. Called correctly then, for the
right reason, and now there is a number behind the hedge.

**The fix** is one line in `crates/slate-slatedb/tests/common/s3server.rs`,
between `accept` and `TokioIo::new`:

```rust
let Ok((stream, _)) = listener.accept().await else { return };
let _ = stream.set_nodelay(true);
```

### 10. The daemon polls its replicas every ten seconds, and the harness never did

**Not fixed here.** The fix belongs in `crates/slate-serverd`, which another
stream had open when this landed; the finding was handed to it with the
measurement, including two things worth deciding rather than assuming — that
the daemon's default poll and `RoutingPolicy::catch_up` are coupled and neither
constant currently knows about the other, and that a configured poll at or
above `catch_up` is this same bug written down deliberately and is probably
worth refusing at startup.

The finding follows.

`slate-serverd` opens each configured replica with `SlateReader::open`
(`storage.rs`), which passes `DbReaderOptions::default()`. That default is
`manifest_poll_interval: Duration::from_secs(10)`. There is no key for it in
the `[[replicas]]` table, so a deployment cannot change it. Every other
construction site in the repository sets it: the replica tests and
`examples/replicas.rs` use 20 ms, and section 4 above was measured at 50 ms.

The same routing section, run at both intervals, 7 runs each:

| | poll 50 ms | poll 10 s (what ships) |
|---|---:|---:|
| `AtLeast(a sequence just committed)` | 26.98 ms [10.90 – 46.32] | **251.87 ms** [251.66 – 252.27] |
| fell back to the writer | **0 of 64** | **64 of 64** |
| `Freshness::Any` (a replica) | 232.41 µs | 206.40 µs |
| `Freshness::Latest` (the writer) | 227.30 µs | 205.30 µs |
| `pool.route`, tenant affinity | 97 ns | 96 ns |

The spread is the finding. At 50 ms the catch-up wait is 10.9–46.3 ms —
roughly uniform inside the polling cycle, which is what a wait looks like. At
10 s it is 251.87 ms with a range of **±0.2%**, which is not a wait at all: it
is `RoutingPolicy::catch_up`, 250 ms, expiring. Every one of the sixty-four
reads gave up and was served by the writer, against none of sixty-four at
50 ms.

So in the daemon that ships, read-your-writes through a replica is 9.3× slower
than the number in section 4 and **the replica fleet contributes nothing to
it** — every such read is a 250 ms delay followed by a read from the writer.
Section 4's closing line, that a deployment finding read-your-writes too slow
"should look at `DbReaderOptions::manifest_poll_interval` first", is right and
is currently impossible to act on through the configuration file.

The three reads that do not have to wait, and both routing costs, are the
controls and none of them move.

**The fix** is a key on the replica table — `poll_interval` in
`config::Replica`, threaded into `SlateReader::open_with` in
`storage.rs::open_replicas` — and a default well under
`RoutingPolicy::catch_up` rather than forty times over it. The two constants
are coupled and neither knows about the other: a poll interval above the
catch-up budget turns every freshness-proving read into a timeout, and that
relationship deserves a line in the configuration documentation whatever
default is chosen.

### What the sweep checked and found correct

Reported because it is worth as much as the findings, and so nobody sweeps it
again:

- **`tonic`'s client-side `TCP_NODELAY`.** `Endpoint::new_uri` sets
  `tcp_nodelay: true` and `connect` passes it to `HttpConnector::set_nodelay`.
  Unlike the server side there is no path that ignores it, and
  `RecordsClient::connect` — which every client in this repository uses — takes
  it. The Nagle bug was server-side only.
- **Every server that binds its own listener.** `slate-serverd`'s `serve.rs`,
  the harness's `harness::serve`, `slate-server`'s test fixture and
  `clients/python/testserver` all call `set_nodelay(true)` on each accepted
  connection now. The one that did not was the S3 test server, above.
- **HTTP/2 flow-control windows.** `tonic` leaves `init_stream_window_size` and
  `init_connection_window_size` unset, so hyper's defaults apply: 1 MiB
  connection and 1 MiB stream on the server, 5 MiB and 2 MiB on the client. A
  server-streamed response is governed by the client's 2 MiB stream window,
  which is eight times the largest batch this head node will send. Nothing to
  tune, and nothing silently small.
- **`object_store`'s client defaults.** 30 s request timeout, 5 s connect
  timeout, HTTP/1.1 rather than HTTP/2 (deliberate upstream, and documented
  there as the faster choice), and `reqwest` sets `tcp_nodelay` on by default.
  No `ClientOptions` are set anywhere here, and none of the defaults are wrong
  for this workload.
- **`ScanTuning`.** 1 MiB readahead and four fetch tasks are already chosen and
  already measured. `cache_blocks: false` is the third field and was checked
  here for the first time: with a cache installed, letting scans populate it
  takes the repeated-scan cost from 1 GET to 0, which is real and is still the
  wrong default — a table scan that evicts the working set of every point read
  is the reason SlateDB has the switch. Left as it is, deliberately, and now
  measured rather than assumed.
- **`DbReaderOptions` everywhere except the daemon.** The replica tests and
  `examples/replicas.rs` set `manifest_poll_interval` to 20 ms and the harness
  takes it as an argument. Only `slate-serverd` takes the default.
- **`prost` and `tonic` codegen.** No compression, no custom codec, nothing
  configured that a later call overrides. `build.rs` is a plain `configure()`
  with `build_client` and `build_server`.
- **`tokio`.** Both binaries use the multi-threaded runtime; no
  `current_thread` runtime is hiding under a server, and nothing on the read
  path blocks a runtime thread.
- **SlateDB's compression codecs are compiled out too**, by the same
  `default-features = false`. It changes no behaviour today, because
  `Settings::default().compression_codec` is `None` regardless — but a
  deployment that wanted compression could not turn it on without a manifest
  change, and that is worth knowing before someone tries.

### Suspected, not demonstrated

- **`bytes` fields cross the wire by copy, twice.** `slate-server`'s `build.rs`
  does not call `tonic_prost_build`'s `.bytes([".."])`, so prost generates
  `Vec<u8>` for the proto's `bytes_value` and `uuid_value` rather than
  `bytes::Bytes` sliced out of the receive buffer. On top of that `convert.rs`
  copies again in both directions — `Kind::BytesValue(bytes.to_vec())` going
  out, `Value::Bytes(bytes::Bytes::from(bytes.clone()))` coming back — while
  the kernel's `Value::Bytes` is already a `bytes::Bytes` that could have been
  handed over for nothing. Every uuid column pays a 16-byte allocation per
  value per direction, and a table keyed by uuid — the shape the README's own
  example uses — pays it on every key of every row of every batch. The
  mechanism is certain; the size of it is not measured, because measuring it
  means regenerating `slate-server`'s protobuf types and that crate was not
  this sweep's to edit. The fixture here is keyed by `u64` and would show
  nothing.

## Paging by key, and loading relations in one read

Two measurements from the record layer rather than the storage engine. Both are
about work that is *skipped*, so both are counted rather than timed: a counting
wrapper around the key-value store, and an assertion on the count. Timing them
would have measured an in-memory fixture and reported noise.

```sh
cargo test -p slate-kernel --test pagination a_page_costs_the_same -- --nocapture
cargo test -p slate-orm --test relations loading_relations_costs_one_read -- --nocapture
```

### `OFFSET` reads what it discards

500 rows, pages of five, the last page fetched two ways:

```
PAGE 99 OF 100: offset read 495 pairs, cursor read 5
```

The rows returned are identical — `[491, 492, 493, 494, 495]` both ways — which
is exactly why no assertion about the rows could have caught this. `OFFSET n`
walks and throws away `n` rows; a cursor narrows the scan's key range so they
are never read.

The test's assertion is deliberately `cursor_pairs * 10 < offset_pairs` rather
than the ratio. A ratio would pin the prefetch depth and the row encoding, and
this test is about neither. The honest lower bound is asserted too: a page of
five rows cannot cost fewer than five pairs.

The second-order cost is worse than the first and is not a performance
property at all — `OFFSET` counts rows, so deleting one ahead of the cursor
between two pages makes the reader skip a row it has never seen. That is
`a_row_deleted_ahead_of_the_cursor_does_not_skip_one`, and it asserts the wrong
answer beside the right one: showing only that the cursor works would not
establish there was anything to fix.

### One read for every parent's children

```
3 parents: batched 1 scan(s), per-parent loop 3
```

`load_related` collects the parents' keys, deduplicates them and issues one
`Expr::In` — which the planner already turns into point gets or an index range.
The loop it replaces is measured in the same test, on the same fixture, rather
than described.

Three parents is a small number and the point is the shape: the batched arm
does not grow with the parent count and the loop does. The assertion is
`cost <= 1` rather than `== 1`, so that a planner which later chooses a point
get over a scan — a *better* plan, and one that opens no cursor — does not fail
a test about costing less.

What is not measured: anything at a size where the `IN` list itself is the
expensive part. A thousand parents means a thousand values in one predicate,
and where that stops being cheaper than a thousand point gets has not been
found.
