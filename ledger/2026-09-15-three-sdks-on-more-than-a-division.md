# The three-SDK comparison ran on one arithmetic expression; it now runs on eight kinds of scalar and on nearest-neighbour order

- **Date:** 2026-09-15
- **Author:** Claude, working from "I don't want any gaps. Please address those"
- **Touches:** `examples/explorer/head.toml`, the three adapters, `examples/explorer/CONTRACT.md`, `conformance/conformance.py`, `web/src/panels.tsx`, `web/e2e/explorer.mjs`
- **Kind:** feature

## What changed

`books` gains two columns — `released` (seconds since the epoch) and
`embedding` (a four-element vector) — and the conformance corpus gains the
cases they make possible:

| `groupBy` | the expression | what it compares |
| --- | --- | --- |
| `decade` | `books.year / 10 * 10` | arithmetic (this was the only one) |
| `shout` | `upper(authors.name)` | a string function |
| `era` | `CASE WHEN books.year < 1970 …` | a conditional |
| `tidy` | `regexp_replace(lower(books.title), '[^a-z]+', '-')` | a regular expression |
| `releasedYear` | `year(books.released)` | a calendar field |
| `releasedMonth` | `month_start(books.released)` | a calendar truncation |
| `releasedHourNY` | `hour(books.released` in `America/New_York)` | a named timezone |
| `label` | `country ‖ '/' ‖ title ‖ '/' ‖ year` | concatenation, across both inputs and over an integer |

Plus a new endpoint, `POST /api/nearest`: books ranked by cosine distance from
a fixed query vector, returning the titles **in order**. 38 cases became 51,
and the grouped-join panel's dropdown offers every one of the computed keys.

## Why

The corpus compared the three SDKs on exactly one expression, and integer
division is the worst possible choice for that: it is the one operation Go,
Python and TypeScript all spell `/` and all mean the same thing by. Three
clients agreeing about `year / 10 * 10` says close to nothing about the ones
where they might not — a `CASE` whose branches are a different type from its
test, a regex dialect, a calendar field over a negative instant, a timezone
name, a vector metric. Every one of those is a place a client could quietly
build a different request, and none of them was covered.

The dates are chosen to make the calendar cases sharp rather than decorative:
seven of the eleven books were released before 1970, so `year(released)` and
`month_start(released)` run on **negative** epoch seconds, which is where a
naive `/ 86400` is wrong; some are in daylight saving and some are not, so
`releasedHourNY` is not a constant shift; and *The Player of Games* at 02:10
UTC is the previous day in New York.

## Alternatives rejected

**Keep the corpus and add a general expression language to the contract.** The
adapters would then have three implementations of a second query language, and
the contract already refuses that for the join builder with the same
reasoning. Named groupings keep each expression written once per SDK, in the
SDK's own idiom, which is exactly what the comparison is about.

**Reuse `authors.born` as a timestamp** rather than adding `released`. It is
already an `i64`, so no schema change — and it holds a *year* (1929), so
`year(born)` would be 1970 for everybody, and rewriting the seed to make it an
epoch second would make the demo's author table say something false to a
reader. Two appended columns cost one line each and keep every existing
ordinal, including the row policy's, where it was.

**Return the distances from `/api/nearest`.** They are the obvious thing to
compare and they cannot be: a distance is an f64, the three languages print
floats differently, and `/api/explain` already excludes `estimatedCost` for
exactly that reason. The *order* is comparable, is total once the sort breaks
ties on the primary key, and is the whole answer a nearest-neighbour query is
for.

**Take the query vector from the request body.** It would make the endpoint
more useful and the comparison meaningless: a caller could ask one adapter a
question the other two were not asked.

**`upper(authors.country)` for the string case** — which is what it was for an
hour. Every country in the fixture is already upper case, so the `upper` was
an identity and an adapter that dropped it would have passed. Changed to the
author's *name*, where it is visibly not the identity (and where `Stanisław`
exercises a non-ASCII upper-casing through all three clients).

**`CASE WHEN books.year < 2000`** — likewise. Every book predates 2000, so the
`otherwise` branch was never taken and the conditional was a constant. 1970
splits the fixture five and five.

## Evidence

**A mutation caught and named.** The Go adapter's `shout` had its `Upper`
removed; the runner reported

```
a grouped join by a computed upper-cased author (app): the adapters disagree
    go      ... {"str": "Iain M. Banks"} ...
    node    ... {"str": "IAIN M. BANKS"} ...
    python  ... {"str": "IAIN M. BANKS"} ...
```

which is the whole claim of this change: a client that builds a subtly
different expression is now caught, where before only arithmetic was.

**The nearest-neighbour order checked against an independent computation.**
Cosine distance folded in Python, outside every client and the kernel, gives
`Solaris, Parable of the Sower, Consider Phlebas, The Astronauts, Kindred, The
Left Hand of Darkness, Author Unknown, The Player of Games, The Dispossessed,
The Cyberiad, A Wizard of Earthsea` — the order all three adapters return,
element for element.

**The calendar cases checked against the seed by hand.** `releasedYear` gives
1951, 1961, 1965, 1968, 1969, 1974 …, matching the dates written into the
seed; `releasedMonth`'s first key is −596937600, which is 1951-02-01T00:00:00Z;
`releasedHourNY` gives the multiset {0, 2, 5, 7, 7, 9, 12, 13, 16, 19, 22},
which is what `zoneinfo` says those eleven instants are locally.

**`label` demonstrates the `concat` fix end to end**: `PL/Solaris/1961`, where
until this morning the kernel spliced Rust's `Debug` form and it would have
read `PL/Solaris/I64(1961)`.

**Two defects the runner found on its first run with the new columns**, both
the same shape — a case nobody had ever produced was a case nobody had ever
checked:

1. **Go and Python's value encoders had no `vector` arm.** They sent
   `{"unknown": "slate.Vector"}` and `{"unknown": "Vector"}` while Node sent
   the real thing. `books` had never had a vector column, so the tag had never
   been exercised. Fixed in both, with elements as formatted strings for the
   reason floats are.
2. **All three transaction handlers inserted a five-column row** into a
   now-seven-column `books`, and the server refused it — with three different
   messages, since two clients quote the server and Python checks locally. All
   three now send every column.

**Suites:** `./run.sh --conformance` 51/51 agree, `./run.sh --e2e` 18/18 pass
(one new check: a reader picks a computed grouping and the chart draws, with
the labels asserted by shape — four-digit years, hours in 0-23, two eras,
upper-cased names — so a grouping that silently returned author names fails).

## What this does not do

**`/api/nearest` has no UI.** Its consumer is the conformance runner. A
nearest-neighbour panel is a design question about the demo rather than a gap
in the comparison, and adding an endpoint with no surface was the cheaper half
of an honest trade.

**No index answers the distance.** There is no vector index here, so
`/api/nearest` is an exhaustive scan and `/api/explain` on the same query says
so. That is the kernel's position — nearness is a scalar and search is an
`ORDER BY` with a `LIMIT` — not a gap in this change.

**The embeddings are four-dimensional and hand-written.** A realistic 768
would make the seed unreadable and the ordering uncheckable by eye, which is
the opposite of what a demo fixture is for.

**`L2` and the other three metrics are untested from the clients.** Only
cosine is in the corpus. The metric is a single enum field, so the risk is
that a client maps the enum wrongly, and one metric does not catch that —
`tests/wire.rs` does, exhaustively, in Rust.

**Nothing compares the adapters on a *filter* over a computed value, or on a
computed value in `/api/query`'s `sort`.** `/api/nearest` sorts by one, so the
sort path is covered for one shape; `HAVING` over a computed key and a
`WHERE` over one are not.

**The schema change is silent for a reader of the demo's tables.** The two new
columns appear in `/api/query` output and nothing in the UI explains them; the
contract does.
