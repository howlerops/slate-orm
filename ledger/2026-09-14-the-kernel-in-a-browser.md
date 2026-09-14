# The record layer, compiled for a browser

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-wasm/` (new), `Cargo.toml`
- **Kind:** feature

## What changed

`slate-wasm`: `slate-kernel` and `slate-schema` compiled to
`wasm32-unknown-unknown`, driven over the kernel's own `MemoryStore` and
exposed through `wasm-bindgen`. A `Playground` seeds two tables, answers a
query described as JSON, and returns the rows together with the real
`Explanation`.

Verified in headless Chromium, not only as a Rust crate:

```
scanAccess:      "Table Scan"                        cost 1.603
coveringAccess:  "Index Only Scan using by_author"   cost 1.0015
```

## Why

The site could show code and not run it. Every claim it makes about planning —
that an index-only scan avoids reading rows, that the plan responds to the
query — was something a reader had to take on trust or clone the repository to
check.

A browser build makes the claim checkable on the reader's own query, which is
worth more than another paragraph asserting it.

## Alternatives rejected

**A JavaScript reimplementation of the query model.** Far less work and
completely worthless: it would agree with whatever its author believed the
kernel did, and drift silently from it. That is precisely the failure the
three-SDK conformance runner exists to catch, and shipping a fourth
unverifiable client to a landing page would be the worst instance of it.

**Hosting the explorer demo instead.** Shows more — a real head node, three
SDKs, writer leadership — and needs a server somebody pays for and keeps
running. A wasm bundle is a static file on the Pages host that already exists.
The explorer is still the better demo of the *distributed* system; this is a
better demo of the *kernel*, and they are different claims.

**Returning promises through `wasm-bindgen-futures`.** The kernel is async, so
this looks like the correct shape. It buys nothing here: `MemoryStore` never
yields on I/O, so every future is ready on first poll, and `block_on` completes
without parking. Making callers `await` a value that is already computed would
add ceremony for no concurrency. Written down in the module docs because it
stops being true the moment a backend that really awaits is swapped in — on a
browser main thread that would be a hang, not a slowdown.

**Keeping the small curated fixture.** See below; the tests refused it.

## What building it found

**The kernel had quietly taken on a binary's runtime.** `tokio = { workspace =
true, features = ["time"] }` is additive, so the kernel was getting
`rt-multi-thread` too. wasm refuses it outright. Fixed in the kernel's own
manifest and written up separately — it is a portability bug that had nothing
to do with browsers.

**`Action::ALL` does not grant `EXPLAIN`.** The playground's first test run
failed with `no role grants explain on table books`, because `Explain` is
deliberately not a data action: it reads statistics describing rows a row
policy may hide. The playground grants `Action::EVERYTHING` explicitly. The
refusal working on the first thing to ask for it is the control for that whole
design.

**A twenty-four row fixture cannot demonstrate planning.** The first version
had six authors and twenty-four books, and `author_id = 1` planned as a *table
scan*. The test asserting an index scan failed, and the planner was right: with
`SCAN_ROW_COST` at 0.000125 and `POINT_READ_COST` at 3.0, **one point read
costs as much as scanning twenty-four thousand rows**. Five matching rows means
five point reads — about 15 — against 1.60 to scan the entire table.

Growing the fixture to 4,824 books does not change that, and cannot: an index
scan that still fetches rows loses at any size a browser tab should hold. What
the larger fixture does buy is statistics worth having, `ANALYZE` actually
running, and estimates that track the data.

So the test was rewritten to assert what is true, and it is a better test than
the one intended. The playground's most useful lesson is not "indexes are
faster" — it is that on object storage an index only pays when it avoids the
row reads entirely, which is exactly what the covering-scan case shows at cost
1.0015 against 1.603.

## Evidence

Nine tests in `crates/slate-wasm/tests/playground.rs`, on the host: the fixture
seeds and reads back, the access path is a table scan where a scan is cheaper,
a projection onto the index is index-only and the covering plan costs less, a
limit applies to rows and not only to the plan, a typed literal against a `u64`
column is refused rather than silently matching nothing, a malformed regular
expression is refused rather than panicking, and the schema comes from the
catalog rather than a copy.

Each plan assertion carries a control, because "this is an index scan" passes
against a planner that says index scan for everything.

In Chromium: module loads, `Playground` constructs, both queries answer, plans
as quoted above.

Sizes: 1,715 KiB of wasm, **596 KiB gzipped**, which is what a browser
downloads.

## What this does not do

No head node, so nothing about leadership, replicas, freshness, the wire
protocol or the three SDKs is exercised. This is the kernel alone, and the
explorer demo remains the only thing that shows the rest.

No joins, aggregates or grouped reads through the binding yet, though the
kernel underneath supports all of them — the surface is filters, sort,
projection, limit and offset.

Writes are not exposed. The store is seeded and then read; a reader cannot
insert a row and watch an index be maintained, which is the other half of what
makes this a record layer rather than a query engine.

596 KiB gzipped is not free, and nothing lazy-loads it yet.

The wasm build is not in CI as of this commit, so nothing stops the kernel
reacquiring a non-wasm dependency tomorrow. That guard arrives with the site
integration.
