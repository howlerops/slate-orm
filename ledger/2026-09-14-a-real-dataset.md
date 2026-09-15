# The workbench stops querying a fixture and starts querying a real month of New York taxi trips

- **Date:** 2026-09-14
- **Author:** Claude (agent session), at the request of the repository owner
- **Touches:** `crates/slate-wasm/src/{taxi.rs,taxi_zones.csv,lib.rs,sql.rs}` (first two new),
  `crates/slate-wasm/tests/{taxi.rs,sql.rs,playground.rs}`, `site/data/{make-trips.py,trips.bin.gz}` (new),
  `site/{index.html,workbench.js,README.md,docs.html}`, `site/check/workbench.py`,
  `docs/performance.md`, `README.md`
- **Kind:** feature

## What changed

The workbench now opens on **100,000 real New York yellow-taxi trips** from
January 2024 — the TLC's published month, the same corpus ClickHouse and DuckDB
benchmark on — joined to the TLC's own 265-zone lookup table. The books and
authors fixture stays as the small-table contrast.

Three things had to be built to make that worth having:

1. **Single-table `GROUP BY`**, with aggregates, `HAVING`-shaped refusals and
   `count(distinct)`. The kernel had it; the SQL front end refused it unless
   there was a join, which made ClickHouse's taxi queries inexpressible.
2. **`ORDER BY` and `LIMIT` over groups**, routed to the kernel's `Grouping`
   rather than to the query. This is what makes `SELECT pickup_zone, count(*)
   ... GROUP BY pickup_zone ORDER BY count(*) DESC LIMIT 10` mean what it says.
3. **A general join.** The parser and the binding hard-coded `authors.id =
   books.author_id`. A zone lookup nobody can join is a list of names nobody
   can reach.

## Why

The ask was for a dataset that shows real capability, with ClickHouse's NYC
taxi benchmark named. The honest answer has two halves, and the first is a
refusal: **the full dataset cannot go in a browser.** ClickHouse's taxi corpus
is ~1.3 billion rows; one month is 2,964,619. A `wasm32` tab has 4 GB of
address space in theory and about 2 GB in practice, and this store costs
~1,290 bytes a row (measured, below), so the month alone wants ~3.8 GB. No
amount of engineering moves that.

The second half is that a *real* 100,000 rows is worth more than a generated
million. Generated data is a claim about the generator. "The busiest pickup
zone in January 2024 was JFK, with 4,837 of 100,000 sampled trips" is a claim
about New York, and a reader can check it against anyone else's copy of the
same file.

## Alternatives rejected

**Generate taxi-shaped data instead, at 250k–500k rows.** Free to download and
bigger. Rejected: `avg(total) = 28.19 for credit card` would then be a
made-up number that looks exactly like a real one, and the page would have to
carry a disclaimer loud enough to undo the benefit. Fabricated data on a page
whose selling point is "check it yourself" is the wrong trade at any size.

**Ship the whole month (2.96M rows, ~30 MB).** Measured rather than assumed: it
loads in 17.8 s and holds 3.8 GB. Both numbers rule it out for a tab. It is in
`docs/performance.md` §7b so the limit is documented as the browser's, not the
record layer's.

**250,000 rows (2.59 MB gzipped).** Tempting — still real, still comfortable on
a desktop. Rejected on the mobile case: 2.6 MB over a phone connection before
the page answers anything, for a fourth of the rows the story needs. 100,000 is
1.26 MB and first paint measured 1.4 s.

**Titanic.** Offered and not taken up. 891 rows is real and famous and makes
every plan a table scan, so it would teach "your index does nothing on a small
table" — which the books fixture already does at 4,824 rows, with the same
lesson and no extra schema.

**Build the sample in CI instead of committing it.** Would keep a 1.26 MB
binary out of git, and cost a 50 MB parquet download on every run, a pyarrow
dependency, and a sampling step that has to be deterministic to the byte or
the file changes under you. The wasm is rebuilt every deploy because it *can*
drift from the kernel; a fixed sample of a published dataset cannot drift from
anything.

**A CSV or JSON payload.** 100,000 rows of CSV is ~8 MB, ~2.5 MB gzipped, and
needs a parser. The packed 24-byte record is 2.4 MB raw and 1.26 MB gzipped,
and `DecompressionStream` is in the platform.

**Keep the hard-coded join and give `zones` its own query path.** Cheaper by an
afternoon and dishonest: the page would be claiming a join engine while
special-casing the only two joins it had.

## Evidence

**Scale, measured rather than argued** (release, 4 vCPU / 15 GB, one run per
size — an order of magnitude, not a benchmark):

| rows | `insert_many` | rate | `analyze` | RSS | bytes/row |
|---:|---:|---:|---:|---:|---:|
| 100,000 | 0.30 s | 329k/s | 0.16 s | 132 MB | 1,282 |
| 1,000,000 | 6.89 s | 145k/s | 1.39 s | 1,330 MB | 1,326 |
| 2,964,619 | 17.77 s | 167k/s | 4.08 s | 3,839 MB | 1,293 |

Two things fall out. **Nothing stops at 600,000** — the wall `docs/performance.md`
§7 recorded on the SlateDB-over-S3 loader is not `insert_many` as an algorithm,
which is the first evidence separating the write path from the storage half
rather than reasoning about it. And **the store holds twelve times what the
data weighs**: these rows serialise to ~110 bytes and cost ~1,290 in memory.
That factor had never been measured and is what decides the browser's ceiling.
*(The twelve is withdrawn — see the note at the end of this entry. It is 5.7×,
now 4.7×.)*

**In the browser**, headless Chromium at 1280×900, against the built bytes:
first paint 1.4 s — wasm fetched and compiled, 1.26 MB fetched and decompressed,
100,000 rows seeded, analysed, and a grouped query answered. `SELECT
pickup_zone FROM trips WHERE pickup_zone = 132` is 2.7 ms index-only; the same
rows with every column is 22 ms as a table scan.

**Real answers.** Busiest pickup zones: 132 (JFK, 4,837), 161 (Midtown Center),
237 (Upper East Side South). Payment split 78,335 card / 14,756 cash, average
totals $28.19 and $22.69.

25 browser assertions and 65 Rust tests, all passing. Mutations, each failing a
*named* test:

| mutation | check that failed |
| --- | --- |
| fill in the missing passenger counts | `count_of_a_nullable_column_is_smaller_than_count_of_rows`, `the_real_nulls_survive_the_round_trip` |
| ignore `ORDER BY` over groups | `the_clickhouse_taxi_queries_run` |
| ignore `LIMIT` over groups | `the_clickhouse_taxi_queries_run` |
| skip re-analysing after the bulk load | `a_filter_on_the_indexed_zone_is_a_real_planner_decision` |
| `avg` computed as `sum` | `every_aggregate_agrees_with_folding_by_hand` |
| allow a column outside the group key | `grouped_refusals_name_what_was_wrong` |
| `count(distinct x)` becomes `count(x)` | `count_distinct_counts_values_not_rows` |
| explain only the first of two group keys | `grouping_by_two_columns_keys_on_the_pair` |

**Two mutations survived and both became tests.** Reading the *dropoff* zone
into the pickup column changed nothing — the join test filters on pickup_zone
and joins on pickup_zone, so a consistently wrong column still finds rows and
still resolves 132 to JFK. Only an independent reading of the packed bytes
tells one field from another, which is now
`every_column_comes_from_the_field_the_format_says_it_does`. Explaining a
narrower grouping than the one executed also survived, and exposed a comment of
mine claiming the two shared one `Grouping` value when they did not; the
binding now goes through `grouped`, which takes the whole thing, and the
comment says what is actually true.

**Three real bugs, all found by machinery.**

1. **My sampler took a prefix of the file.** It sorted candidate indices and
   stopped at the sample size — New Year's Day, not the month — while the
   comment above it claimed to avoid exactly that. Caught because the sample
   had *zero* rows with a missing passenger count in a month where 4.73% have
   one.
2. **`ORDER BY` beside `GROUP BY` was silently discarded.** It was lowered onto
   the query, so it sorted the rows going *into* the grouping, which the
   grouping then re-ordered by key. A test asserting the refusal found it
   accepted instead.
3. **Reset emptied the taxi table and left it empty**, with no way back but a
   page reload. The browser check reported zero rows where 72 were expected.

**A cost-model limitation, recorded not fixed.** An unfiltered `GROUP BY` on an
indexed column plans as a table scan: scanning 4,824 index entries and 4,824
whole rows both cost `1.0 + 4824 × 0.000125 = 1.603`, and the tie goes to the
scan. The costs are equal and the work is not — one column against four. The
model charges per row and has no notion of width. Written up in
`docs/performance.md`; the test pins the arithmetic so the day it stops being a
tie, something says so.

## What this does not do

**It is not a benchmark and must not be read as one.** No cold/warm
separation, one run per size, a container on shared hardware, and no comparison
to any other system. ClickHouse would beat this badly on these queries — it is
a column store built for them and slate is a row-oriented record layer — and
the four queries here are run to show they *work*, not to race.

**ClickHouse's Q3 and Q4 are adapted, not replicated.** Both key on
`toYear(pickup_datetime)`. There is no date type in the kernel and one month has
one year in it, so Q3 substitutes a second key that varies. Saying so is the
point; a replicated-looking query that answers a different question would be
worse than an honest substitution.

**No date or time handling at all.** `pickup_time` is an integer of seconds.
"Trips per hour of day" is not expressible, which is the most obvious question
a reader will have about this dataset and the largest gap the page now has.

> **Both closed on 2026-09-15** by `the-year-there-was-no-year-to-key-on`.
> `hour()`, `day_of_week()`, `year()` and the rest are computed columns in the
> SQL front end now, and Q3 and Q4 are written as ClickHouse writes them. The
> year is still constant, because the sample is still one month — but that is a
> property of the sample and no longer of the grammar.

**The full month is not reachable from the browser**, and the workbench does
not say so on screen — only `site/README.md` and the docs do.

**`trips` has one secondary index**, on `pickup_zone`. Queries filtering on
time, distance or fare are table scans, correctly but unremarkably.

**The 1,290 bytes a row was measured and not investigated.** No attempt was
made to find where it goes or to reduce it, and the obvious suspects — a
`String` per row for the payment type, two `Vec<u8>` per row once the index
entry is counted, `BTreeMap` node overhead — are hypotheses, not findings.

> **Closed on 2026-09-15** by `where-the-bytes-a-row-went`, and the number was
> wrong: 474 of the 1,290 was the source rows, which RSS counted and the store
> never held. The store is 491 bytes a row, now 403 after a two-line fix, over
> keys and values totalling 86.
