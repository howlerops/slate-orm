# 600,000 rows load in 12.6 seconds, against a recorded "did not finish in five minutes, on three attempts" — and the knob my own last entry cleared turns out to place the bump it missed.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #293 (F6m)
- **Touches:** `crates/slate-slatedb/examples/loader_cliff.rs`, `crates/slate-slatedb/tests/common/s3server.rs`, `docs/performance.md` §8e, `README.md`
- **Kind:** a measurement that closes one open candidate, reopens another, and corrects an entry from three hours ago

## What changed

The fake S3 server can say how much disk it is holding, and `loader_cliff`
reports it per chunk. That one column changes both answers.

**The cliff does not reproduce.** 600,000 rows in 12.6 / 12.8 / 12.6 s, a
million in 22.0 / 23.2 / 22.5 s, identical PUT counts and identical bytes
across runs.

**The disk candidate is closed.** `performance.md` §7 left "the disk under the
in-process S3 server" explicitly not ruled out, and nothing had weighed it,
because the counters count requests and a 120,000-row load issues under thirty.
The store wants 238 bytes a row, flat over a factor of eight, so a million rows
is 227 MiB — never near this container's free space.

**And `l0_sst_size_bytes` does place something, which #292 said it did not.**

| `l0_sst_size_bytes` | on-disk jumps at | interval |
|---|---|---:|
| 8 MiB | 70k, 130k, 190k | ~60,000 rows |
| 16 MiB | 140k | ~130,000 rows |
| 64 MiB (stock) | 510k | ~510,000 rows |

Double the knob, double the interval. At each jump the directory nearly doubles
and PUTs leap from 2 a chunk to 9 — a compaction — for about 0.3 s. The first
one at the stock setting lands at ~510,000 rows: **inside the 500,000–600,000
band the cliff was recorded in.**

## Why

#292 closed with "the cliff is above 400,000 rows and is not set in proportion
to either backpressure knob", and I wrote the README bullet that said **Not
`l0_sst_size_bytes`**. That bullet was mine and it was too strong. #292 tested
one mechanism — L0 filling and writers pausing — refuted it correctly, and then
generalised from the mechanism to the knob. The knob does have a part; it is
compaction scheduling, not backpressure.

What made the difference is the disk column, which #292 did not have. Reading
only the timing column, a 0.3-second compaction is one chunk slightly over a
1.5x threshold — indistinguishable from noise, and #292 duly called it noise.
The doubling directory and the PUT burst are unambiguous.

## Alternatives rejected

**Trust the "still reproducible" in the README.** It had been carried since
#118 and restated by me yesterday. Nobody had re-run it; four recorded failures
on some machine are not a property of this code, and the cheapest thing
available was to run the size the bullet said could not be run.

**Extrapolate from the bytes-a-row figure instead of loading a million rows.**
The first plan was to measure 120,000, divide, and argue about where it would
cross the free space. That is the recorded-literal defect #292 refused to
commit — and the load takes 22 seconds, so the argument cost more than the
experiment.

**Report the first 600,000-row run.** It showed three slow chunks at
510k–530k and I had left a second instance of the example running in the
background, which overlapped exactly that region. Two processes, one
measurement, and the contaminated part was the interesting part. Re-run alone,
three times. The bump is real and survives, but that run could not have shown
it — and its "observed cliff" line moved between clean runs (350k, 510k, 510k),
which is what told me the *timing* signal alone is noise.

**Leave the assertion inline in `main`.** The first draft put
`assert!(on_disk > 0)` there. `cargo test --example` never runs `main`, and the
runner that does — `run_examples.sh --smoke` — cannot be built on this
container: nine debug example binaries exhaust the disk and the link dies at
`ld terminated with signal 7 [Bus error]`, tried twice, once after reclaiming
to 1.91 GB. So the one check guarding every reported number would have been the
one thing no suite here could reach. It is a named function now, and tested.

**`#[should_panic]` for that test.** It scores as **unreadable** rather than
caught: libtest prints a failing should-panic case as
`test NAME - should panic ... FAILED` and `mutate.py` reads `test NAME ...
FAILED`, so the extra words make a caught mutation look like a broken run. Hit
it, then switched to `catch_unwind`. Recorded below as its own finding.

## Evidence

[`performance.md` §8e](../docs/performance.md), two tables, both carrying the
build line, three runs at each of the two sizes that matter.

Five mutations, **four caught and one survived as recorded**, in
[`ledger/mutations/20260923T132547-crates-slate-slatedb-examples-loader-cliff-rs.json`](mutations/20260923T132547-crates-slate-slatedb-examples-loader-cliff-rs.json):
the walk not recursing, files counted rather than weighed, a missing directory
panicking, and the empty-store condition inverted. The survivor is the *call
site* in `main`, for the reason above; the record carries that reason rather
than the entry alone asserting it.

`sh scripts/check.sh`: **48 passed, all of them.**

## What this does not do

**It does not explain the original observation.** A 0.3-second compaction is
not five minutes. Either that machine turned the same event into something far
worse — plausible, untested, and unrecoverable now because nothing recorded
what else was running — or it was something this build no longer does. "Not
reproducible here" is the honest claim; "it never happened" is not, and four
recorded attempts are evidence about a machine I cannot inspect.

**It does not measure the cost model at a million rows.** It only establishes
that the size is reachable, in 22 seconds. The calibration is a separate run
and nobody has done it.

**The compaction's cost is measured on an idle container with 16 GB of
memory.** How that 0.3 s behaves under real memory pressure — the condition
most likely to explain the original five minutes — is exactly what is not
tested here.

**One fixture, ~110 bytes a row serialised, one loopback `s3s`.** Every row
count here is a property of that fixture.

**`mutate.py` cannot read a `should_panic` failure**, so a correctly caught
mutation of any such test scores as unreadable. Found by hitting it; worked
around in this file rather than fixed in the tool, which leaves the next
`should_panic` test in this repository with the same trap.
