# The node's summary reports a p50, p90 and p99, read out of a histogram that says by how much it might be over

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-serverd/src/observe.rs`, `crates/slate-serverd/tests/observing.rs`, `docs/orm-comparison.md`, `site/docs/deployment.html`
- **Kind:** feature

## What changed

Each method's counters gained a log-linear histogram of head durations — eight
buckets to the octave, 496 of them, a fixed 4 KiB — and the summary line grew
three fields:

```
slate-serverd: /slate.v1.Records/Query calls=2 failed=1 mean_head=1.4ms \
  p50_head<=1.0ms p90_head<=2.0ms p99_head<=2.0ms slowest_head=1.9ms
```

`<=` rather than `=` because a bucketed quantile is an upper bound: within an
eighth of the truth, never under it. And clamped to the exact `slowest`, so a
quantile can never print above a maximum on its own line.

## Why

The gap table has said since the observability item shipped that what it
reports "is enough to notice a problem and not enough to characterise one".
That is precise, and it is the whole of this change.

A mean and a maximum do not describe a latency. The mean of a bimodal
distribution is a value nothing produced — a node where 95% of calls take 1 ms
and 5% take 200 ms has a mean of 11 ms, which is both alarming and wrong about
every request it served. The maximum is one sample, and on a node that has
been up a week it is almost always a cold start nobody will ever see again. The
one number that answers "is this slow for the people using it" is a high
quantile, and there was none.

## Alternatives rejected

**A `/metrics` endpoint and let something else compute the quantiles.** The
right long-term answer and a much larger one: a second listener, a second port
to configure and firewall, an exposition format to get right, and a decision
about whether it is authenticated — on a process whose whole observability
surface is currently "lines on stderr". It also does not remove this work, it
moves it: an exporter still has to keep a histogram, because a scraper cannot
compute a p99 from a mean. Left recorded as still-open.

**Keep every sample and sort at summary time.** Exact quantiles, about fifteen
lines. Rejected on memory and on the lock: a node serving ten thousand requests
a second accumulates 288 MB of `u64` an hour with no bound, and the `Vec` would
need a mutex taken on the path that every request already takes — where today
there are two relaxed atomic adds. The histogram is 4 KiB a method for ever.

**`hdrhistogram` or a t-digest crate.** Better precision and somebody else's
correctness. Rejected because the arithmetic here is four lines and its error
bound is a property this file can state and test, where a dependency's is a
property it would have to trust. `slate-serverd` is the binary an operator
runs; a crate in its tree is a thing to audit and upgrade for ever.

**Two sub-bits per octave (25% error) or four (6.25%).** Two cannot tell 80 ms
from 100 ms, which is most of the distinctions somebody reading a p99 is making.
Four doubles the array to report a precision that a node serving a few hundred
requests an interval does not have the samples to support.

**Report the bucket's floor.** Symmetric-looking and wrong in the direction
that matters: a latency reported under the truth is the one that reads as "this
is fine". The ceiling is an honest over-estimate, and the field name says so.

**Reset the histogram on each summary, so the numbers are per-interval.**
Rejected for consistency: `calls` and `failures` are cumulative, and a line
mixing lifetime counts with interval quantiles is a line that will be misread.
The cost of that choice is real and is in *What this does not do*.

## Evidence

Seven mutations, each restored. **Two survived the first pass and both were
the finding**, which is the point of doing it:

| # | mutation | outcome |
|---|---|---|
| H1 | a quantile reports the bucket's floor, not its ceiling | killed — three tests |
| H2 | the rank sweep uses `>` instead of `>=` | **survived** |
| H3 | the rank floors instead of ceiling | killed — `a_quantile_never_exceeds_the_observed_slowest` |
| H4 | the clamp to `slowest` is dropped | killed — same |
| H5 | `record` never increments a bucket | **survived** |
| H6 | `bucket_of` drops the sub-bucket bits, so an octave is one bucket | killed — three tests |
| H7 | the p90 field prints the p50 | killed — `a_quantile_is_the_nearest_rank_and_never_under_the_truth` |

**H5 is the worse of the two.** The whole feature was unwired — nothing ever
filled a bucket — and every test passed. Two things conspired. The quantile
test filled buckets *itself* and then called `Method::quantile`, so it measured
arithmetic and never the path a request takes; and `quantile`'s fallback arm,
which exists for the race between reading `calls` and sweeping the buckets,
answered `slowest` for every empty histogram — a plausible number, in the right
units, on the right line. Both tests now go through `Counters::record` and read
the rendered summary, and the note about why is in the file.

**H2 needed a different distribution, not a tighter assertion.** An off-by-one
in the rank moves the answer by one sample, and across a hundred samples evenly
spread from 1 ms to 100 ms that is 1%, well inside the 12.5% the bucketing
already costs — so no tolerance that admits the bucketing can also catch the
mutation. `a_quantile_falls_on_the_rank_and_not_one_past_it` uses a cliff
instead: ninety-nine samples at 1 ms and one at a second. The true p99 is the
99th sample, so it is 1 ms; one sample later it is 1000 ms. The mutation moves
the answer by a factor of a thousand.

**The bucketing's own claims are tested rather than asserted.**
`the_buckets_are_monotone_and_gapless` probes every value to 255 and every
value within eight of every power of two to 2^63, and checks the pair that
together mean "this is the one bucket holding this value":
`bucket_ceiling(i) >= v` and `bucket_ceiling(i - 1) < v`.
`a_bucket_is_never_more_than_an_eighth_wider_than_its_floor` checks the
`+12.5%` that the doc comment, the log field name and the gap table all now
claim — a number in three documents and nothing checking it is a number that
drifts the day somebody edits `SUB_BITS`.

Two of those tests were red on their first run and the tests were what was
wrong, not the code: the monotone check asserted a step of at most one bucket
between *probes*, which sparse probes above 256 cannot satisfy, and the width
check compared against the floor with the wrong constant and did not exempt the
exact single-value buckets below 8.

`cargo test -p slate-serverd --no-fail-fast`, `cargo clippy --workspace
--all-targets`, `cargo fmt -p slate-serverd`: green.

## What this does not do

- **The quantiles are over the process's whole life, not the last interval.**
  This is the sharpest limitation and it gets worse the longer a node runs: an
  incident an hour ago is still in the p99 an hour later, and a node that has
  been up a week has a p99 that has stopped moving. A scraper solves it by
  taking a delta between two reads of the bucket counts — which needs the
  buckets exposed, which is the `/metrics` endpoint that is still open. The two
  items are one item; recorded here so the next person does not build the
  second without the first.
- **No scrape endpoint and no `tracing`.** Unchanged, still stderr, and the
  reasoning for both is in the module's own header.
- **Still timed to the response head.** A streamed read's rows are not in any
  of these numbers, including the quantiles, for the reason the module doc
  already gives.
- **A failure raised in a trailer still counts as a success**, so `failed=`
  can read low on a streamed read that died part way. Named in `head_status`'s
  doc comment, unchanged here, and the next item on this list.
- **Nothing else is bucketed.** Rows returned, bytes written and queue depth
  have no histogram and no counter; the only distribution this node can
  describe is the time to a response head.
- **The client's view is still the client's.** These are server-side numbers
  and exclude everything between the two — a client's p99 will be higher, and
  the difference is exactly what an operator wants and cannot get from here.
