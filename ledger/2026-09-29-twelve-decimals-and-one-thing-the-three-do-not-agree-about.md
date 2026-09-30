# Twelve decimals the three clients agree on, and one thing they do not

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `examples/explorer/backends/{go,node,python}`,
  `examples/explorer/conformance/conformance.py`,
  `examples/explorer/CONTRACT.md`, `scripts/check_demo_surface.py`
- **Kind:** feature

## What changed

`POST /api/render-decimals` in all three adapters: one shared table of twelve
`(units, scale)` pairs, run through each client's own decimal renderer, with no
server in it at all. One conformance case compares the three. 139 cases to 140.

**They agree on every row**, including both i64 extremes — the null result, and
the one worth having, because negating the magnitude of `i64::MIN` overflows in
two of the three languages and only Go's renderer has a comment saying so.

**And they disagree about something else, demonstrated on the way.** Given a
*negative* scale, Python raises `ValueError` while Go and TypeScript return the
value with no point at all. Both of those two say in their own comments that
they clamp on purpose — "a caller who got a scale wrong wants a number they can
see is wrong, not a crash in a render path" — and Python's says a scale is not
negative. Two thirds of the clients made one decision and one made the other,
and nothing had ever put them side by side. Not fixed here, for the reason
below; recorded as an open caveat.

## Why

> **The corpus compares renderers at one scale.** Every adapter renders against
> 2, because that is what `books.price` declares. A disagreement showing only at
> scale 0 or a negative value would not appear; each client's own suite has its
> edge-case table, written independently rather than shared.
> — `ledger/2026-09-18-the-three-sdks-compared-on-a-decimal.md`

The second sentence is the whole argument. Three edge-case tables written by
three authors agreeing is not three renderers agreeing: the tables can differ,
and where they differ neither renderer is compared against anything. A decimal
on the wire is a count of the column's smallest unit and the scale never
travels, so formatting is the one part of the decimal story that is entirely
the client's — the server cannot be wrong about it and cannot be the oracle
for it.

`/api/conditional-update` did compare the three, on `12.50`. Scale 2, one
positive value, one whole part above one. That leaves out every branch where
the implementations actually differ.

## Alternatives rejected

**Declare a second decimal column at another scale on `books`.** The obvious
way to get scale variety through the *database* rather than beside it, and the
one that would also exercise the wire and the planner. It is a schema change
that every generated declaration in three languages is pinned to, plus the
fingerprint, plus the seed, plus every case that reads a whole `books` row with
no projection — which is deliberately most of them. A renderer is a pure
function and comparing it needs none of that.

**Take the table from the request body.** Then the corpus could grow cases
without touching three adapters. Rejected for the reason `/api/nearest` gives
about its fixed vector: a table from the caller lets one adapter be asked a
question the other two were not, and the runner sends each case to all three
independently. Fixed in all three, compared as data, and a table that drifted
in one adapter is a disagreement — which is what two of the mutations below
demonstrate.

**Put it in each client's own suite instead, with a shared fixture file.** A
JSON fixture read by three suites would compare three renderers to one
recording. That is what the clients already do, and it is what the caveat
called out: agreeing with a recording is not agreeing with each other, and a
recording is a fourth thing that can be wrong. The runner compares the three
live answers.

**Fix the negative-scale divergence while here.** Two clients clamp, one
raises, and the honest options are to make Python clamp or to make the other
two raise. Neither is reachable from a corpus of scales 0 to 4: no column
declares a negative scale, so nothing on the wire can produce one, and this
endpoint's table deliberately does not include one — a case comparing three
behaviours that are three by design would fail on the first run and say
nothing new. Changing a shipped client's error behaviour is a decision with
its own alternatives, and it is not this entry's.

## Evidence

- `examples/explorer/run.sh --conformance`: **140 cases, the three SDKs agree
  on all of them. 140 passed, 0 failed.** Up from 139.
- The twelve, as Python renders them and as all three agreed:

  | units | scale | text |
  | --- | --- | --- |
  | 0 | 0 | `0` |
  | 0 | 2 | `0.00` |
  | 7 | 1 | `0.7` |
  | 5 | 4 | `0.0005` |
  | -1 | 2 | `-0.01` |
  | -75 | 2 | `-0.75` |
  | 1250 | 0 | `1250` |
  | 1250 | 1 | `125.0` |
  | 1250 | 2 | `12.50` |
  | -1250 | 3 | `-1.250` |
  | 9223372036854775807 | 2 | `92233720368547758.07` |
  | -9223372036854775808 | 2 | `-92233720368547758.08` |

- The divergence, run rather than read. `Units(1250)` at scales -1, 0 and 2:
  - Python `to_string_with_scale`: raises `ValueError: a scale is not
    negative, got -1`, then `'1250'`, `'12.50'`.
  - TypeScript `unitsToString`: `"1250"`, `"1250"`, `"12.50"`.
  - Go `StringWithScale`: `"1250"`, `"1250"`, `"12.50"` — from a throwaway
    `go test` in `clients/go/slate`, removed afterwards.
- **Three mutations, all caught**, each against a different failure the shared
  table exists for:
  - `ledger/mutations/20260929T023736-examples-explorer-backends-node-src-main-ts.json`,
    two cases: the Node adapter rendering every row at scale 2 — which is
    exactly the blindness the caveat named — and its table losing the negative
    below one whole unit. Both → *twelve decimals rendered: the adapters
    disagree*.
  - `ledger/mutations/20260929T023813-examples-explorer-backends-go-renderdecimals-go.json`:
    the Go table's `math.MinInt64` moved by one. Same finding, which is the
    point — three hand-kept copies of one list are three chances to drift, and
    the `units` and `scale` are in the answer so a drifted table is a
    disagreement rather than a quieter comparison.
- `python3 scripts/check_demo_surface.py`: `15 of 26 adapter endpoints in the
  UI, 11 left out on purpose`.
- `python3 scripts/caveats.py`: 1642 caveats, 114 open, 72 narrowed, 406
  closed, 917 deliberate, 0 untriaged.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**It compares rendering, not parsing.** No client here reads `"12.50"` back
into units, because none of them offers it — a decimal arrives as a count and
leaves as a string, and the round trip does not exist to be compared.

**Twelve rows is not a property test.** An oracle over random `(units, scale)`
pairs would catch the thirteenth case nobody thought of, and this is a
hand-written differential: it tests what somebody thought of, which
`CLAUDE.md` says to know rather than to pretend otherwise. What makes it worth
more than each client's own table is that all three are held to one list.

**Scale above 4 is untested.** Nothing in the demo declares one and no row here
uses one, so a renderer that broke at scale 18 — where `10**scale` leaves the
range of an `int64` divisor in Go — is invisible. Go computes its divisor in an
`int64` loop and would overflow silently; Python and TypeScript would not.
Named because it is the next row somebody should add, and it is not added here
because the three would then genuinely disagree and the case would be a bug
report rather than a comparison.

**The negative-scale divergence is recorded, not resolved.** Open, with the
three behaviours measured. Whoever picks a side has to change a shipped
client's contract on an argument, not on a failing case.

**No guard holds the three tables to each other except the runner.** They are
three literals in three languages and nothing parses them. The conformance case
catches a drift because `units` and `scale` are in the answer — which is enough
and is not the same as a guard that would fail without a running stack.
