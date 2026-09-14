# The workbench's timer now excludes rendering, covers writes, and has tests that would catch it lying

- **Date:** 2026-09-14
- **Author:** Claude Opus 5, at Jacob's direction
- **Touches:** `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/timing.rs`, `site/workbench.js`, `site/check/workbench.py`, `site/README.md`
- **Kind:** fix

## What changed

Three gaps in the number the workbench prints beside every query. **One:** the
clock ran until after each `Row` had been turned into display strings, so a
hundred thousand rows times eleven columns of `format!` were charged to the
database; `render` now happens after the clock stops, on every path. **Two:**
writes were not timed at all — `INSERT`, `UPDATE` and `DELETE` printed no
number, which is the one a record layer most wants to show, because the row and
its index entry go in the same transaction. `write`, `remove` and `patch` now
return `(message, elapsed_ms)`, `UPDATE` charges for the read it has to do
first, and the Log tab prints a time per statement. **Three:** nothing asserted
the number was a measurement. `tests/timing.rs` is six tests that a binding
returning a plausible constant fails.

## Why

The reported time was the reason to trust the page, and it was overstated by
roughly a factor of two on the query a reader is most likely to run:
`SELECT * FROM trips` read 130 ms when the kernel's share was 56. Worse, the
overstatement was *proportional to the result size*, so the page's own headline
— that a narrow projection turns a table scan into an index-only scan — was
measured with a thumb on the scale.

The tests matter more than either fix. Every check on this page passed against
a binding that returned `kernel_ms: 1.0` for everything: the browser check
asserted the number was displayed and separated from the JSON overhead, never
that it was true. A timing you cannot falsify is decoration.

## Alternatives rejected

**Report one number, the round trip, and drop the distinction.** Honest and much
simpler. Rejected because it is 197 ms against 56 for `SELECT *` — three
quarters of it `serde_json` building an 8.4 MB string and `JSON.parse` taking it
apart, neither of which the database would do over a socket. One number would
say slate is slow at `SELECT *` when what is slow is the demo's transport.

**Time the kernel with a counter of rows and a cost model instead of a clock.**
Deterministic, testable exactly, no clock-resolution problem. Rejected because
it would be the planner's own estimate reported back as if it were a
measurement — a number that agrees with the cost model by construction and
cannot ever contradict it. The whole value of the display is that it *can*.

**Test the timer by asserting a range — "an index-only scan takes under 10 ms".**
The obvious approach, and what a first draft did. Rejected: it asserts something
about this container, not about the code, and CI runs on a shared box. The
tests assert *orderings* instead, which survive a machine three times slower.

**For the render-exclusion test, compare a narrow projection against a wide
one.** This is what was written first, and it is wrong: late materialization
couples output width to kernel work, so the wide query genuinely does eleven
times the decoding and the threshold had to be loose enough (200×) to swallow
the rendering it was meant to detect. The mutation sailed through. Replaced with
a `GROUP BY` against the plain `SELECT` of its key over the same predicate —
same access path, same entries read, and the grouping then does strictly more
per row, so it can never be the faster of the two unless the select is paying
for output.

## Evidence

Re-measured in headless Chromium against the built bytes, medians of seven,
spread in brackets. The kernel column moved because rendering left it:

| query | kernel, before | kernel, now | round trip |
|---|---:|---:|---:|
| `pickup_zone = 132`, index-only | 3.6 ms | 2.9 ms [2.7–4.5] | 9.9 ms |
| the same rows, every column | 43.5 ms | 31.0 ms [27.6–32.3] | 40.0 ms |
| `SELECT * FROM trips` | 130 ms | 56.2 ms [52.6–64.9] | 197.2 ms |
| 200 `INSERT`s in one buffer | not timed | 0.7 ms [0.6–1.3] | 3.8 ms |

Four mutations, each restored and re-verified:

| mutation | caught by |
|---|---|
| `now_ms()` returns a monotonic counter, not a clock | `the_reported_time_never_exceeds_an_independent_clock`, `the_reported_time_grows_with_the_work`, and both write tests |
| `render` moved back inside the timed region | `the_clock_stops_before_the_rows_are_rendered` — *after* it was rewritten; the first version passed |
| `wrote()` reports `0.0` instead of the elapsed | `a_write_reports_what_the_index_maintenance_cost`, and the browser check's `200 writes report what the index maintenance cost` |
| `UPDATE` reports its write and not its read | `an_update_charges_for_the_read_it_has_to_do` |

The second row is the useful one: the mutation survived the test written to
catch it, which is the only reason the test got rewritten. Measured margins for
the replacement, best of seven: clean, the select is *faster* than its grouping
(11.2 vs 12.9 ms at 4,837 rows; 274 vs 285 ms at 100,000). Mutated, the ordering
flips (16.3 vs 12.9; 372 vs 297). The threshold sits at 1.15× — the mutation is
worth 1.26× and 1.31×.

**A browser clock cannot resolve one write.** `performance.now()` is clamped to
0.1 ms in a page that is not cross-origin isolated — measured, not assumed, by
taking the minimum non-zero difference over 200,000 consecutive reads. One
`INSERT` into `trips` costs about 3.5 µs, arrived at by dividing the 200-row
buffer, which is 25 times under the grain: a lone insert reads `0.00 ms` **293
times out of 300**. The first version of the browser assertion checked that one
insert reported a non-zero time. It passed. It would have failed 98% of the
time, and it is now a check on the buffer, which is stable over three
consecutive runs.

## What this does not do

The wasm arm of `now_ms()` — `performance.now()` through `wasm_bindgen` — is
not exercised by `tests/timing.rs`, which is a native test and takes the
`SystemTime` arm. What covers the real arm is the browser check, and only
coarsely: that the numbers are present, ordered plausibly and non-zero in
aggregate. Nothing compares the wasm clock against a second source in the
browser.

Nothing catches a clock wrong by a constant factor *inside* the bound — one
reporting 0.6× the truth would pass all six tests. Only a second implementation
would, and there is not one.

The `+ N ms JSON` figure the status bar shows is still a subtraction, round trip
minus kernel, so it absorbs anything else that happens between them. It is
labelled as marshalling and is mostly `serde_json`, but "mostly" is doing work
in that sentence and it has not been profiled.
