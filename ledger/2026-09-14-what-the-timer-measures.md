# The timing on screen was the round trip, not the query, and a hundred thousand rows froze the tab for half a minute

- **Date:** 2026-09-14
- **Author:** Claude (agent session), prompted by the owner asking whether the speed shown is real
- **Touches:** `crates/slate-wasm/src/lib.rs`, `site/workbench.js`, `site/check/workbench.py`,
  `site/README.md`
- **Kind:** fix

## What changed

The kernel now clocks itself. `Answer` and `JoinAnswer` carry `kernel_ms` —
milliseconds spent planning and executing, measured around the `block_on` and
nothing else — and the status bar reports that, appending `+ N ms JSON` only
when marshalling is a material share.

And the grid renders at most 1,000 rows, saying so when it truncates.

## Why

The question was whether the number on screen is real. It was real in the sense
that mattered least: a true `performance.now()` around a true call. What it
measured was **the round trip** — parse, plan, execute, `serde_json` serialise,
`JSON.parse` — and for a query returning many rows most of that is not the
database.

Measured, seven runs each, medians:

| query | kernel | round trip | JSON share |
|---|---:|---:|---:|
| `WHERE id = 500` | 0.1 ms | 0.1 ms | 0% |
| `pickup_zone = 132`, index-only | 3.6 ms | 10.6 ms | 66% |
| the same rows, every column | 43.5 ms | 49.7 ms | 12% |
| full scan, grouped to 226 | 38.1 ms | 38.5 ms | 1% |
| `SELECT * FROM trips` | 130 ms | 316 ms | 59% |

So the page was overstating by up to 3×, and worst exactly where it mattered
most: the index-only scan that the whole site is built to show off was
reporting 10.6 ms for 3.6 ms of work, because 66% of its round trip was JSON.
Correcting it makes the comparison **better** — 3.6 ms against 43.5 ms is a
12× difference where the old numbers showed 4.7×.

Chasing that turned up something worse. Asserting on `SELECT * FROM trips` made
the browser check time out, which was not a flake: the query takes 114 ms and
the page then spends **29.5 seconds** building 100,000 `<tr>` into the DOM. A
page that locks up for half a minute after a 114 ms query is not demonstrating
speed, it is hiding it.

## Alternatives rejected

**Leave it and document what the number includes.** The cheapest honest option,
and it leaves a number on screen that most readers will read as query time.
Prose next to a number loses to the number.

**Report only the round trip, and call it that.** Truthful and useless: nobody
comparing two databases cares how fast `serde_json` is, and the page's one
job is to show what the planner did.

**Time it with `std::time::Instant`.** Panics on `wasm32-unknown-unknown` —
there is no clock source. `performance.now()` is imported through
`wasm_bindgen(inline_js)` instead, with a native arm for the tests.

**Return rows lazily, or a cursor, instead of one JSON blob.** That would cut
the marshalling rather than measure around it, and is the right answer if this
were a product. It is a browser demo whose largest table is 100,000 rows; a
paging protocol between JS and wasm is a lot of machinery to make a number
smaller that is now reported correctly.

**Cap the query at 1,000 rows instead of the table.** Then `SELECT * FROM
trips` would report 1,000 rows and be lying about the database. The query
returns everything and the status bar says 100,000; the *table* is what is
capped, and it says so.

**Virtualise the grid.** The right fix for a client people use daily, and
several hundred lines of scroll maths for a page where the cap costs one
sentence of honesty.

## Evidence

Every number above is a median of seven runs in headless Chromium against the
built bytes, with the spread recorded: the kernel column ranged 3.5–6.6 ms for
the index-only scan, 127–150 ms for the 100,000-row scan, so treat the last
digit loosely.

The render measurement is one observation, not seven — 29,535 ms wall clock
from clicking Run to 100,000 rows being present in the DOM, against a query
the binding clocked at 114 ms. One run is enough for a number that large.

Four new browser assertions, and the suite goes from 36 to 40:

- a grouped query reports its time with no JSON breakdown (~1% overhead)
- a 100,000-row result separates the marshalling from the query
- a 100,000-row result does not build 100,000 rows into the page, and returns
  in under 8 seconds
- and it says what it truncated

**A bug of my own, caught by the check.** The new driver step wrote
`out.grouped`, which was already the grouped *join*'s result several steps
above. The join check then read a trips group-by and failed on its headers.
Renamed, with a comment saying why, because the next person adding a step will
reach for the same obvious name.

## What this does not do

**`kernel_ms` still includes building the `Vec<Vec<String>>`** that the rows
are rendered from — `render()` runs inside the timed region, so a query
returning 100,000 rows pays 100,000 string formats inside the number. That is
closer to the truth than the round trip and is not the truth: the honest
number would time only the cursor loop. Splitting it further means timing
inside the kernel rather than around it.

**The write paths are not timed.** `INSERT`, `UPDATE` and `DELETE` report no
milliseconds at all, so a reader cannot see what index maintenance costs —
which is the one number a record layer should most want to show.

**Nothing asserts the reported time is *accurate***, only that it is reported
and separated. A binding that returned a plausible constant would pass every
check here.

**The cap is 1,000 rows and is not configurable.** No paging through a large
result, no "show me the next thousand" — the pager built for the keyspace
viewer was not reused here, and should be.
