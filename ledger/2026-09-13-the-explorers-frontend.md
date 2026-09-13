# The explorer's frontend

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `examples/explorer/web/` (new)
- **Kind:** feature

## What changed

A SolidJS app with TanStack Query, over the three adapters from the previous
commit. Five panels — rows with a plan, joins, a grouped join drawn as bars,
transactions, and an agreement check — and two switchers in the header.

## Why

The interactive half of the demo. The backend proves the three SDKs agree; the
UI is what makes that visible to someone who has not read the code, and what
makes the *database's* behaviour visible rather than the client's.

## What it is actually demonstrating

The **SDK switch** is the headline and the least interesting technically: it
changes a base URL. Its value is negative space — nothing else on screen
changes, which is the claim.

The **identity switch** is the one worth building. `app`, `reader` and
`stranger` differ only in what the database grants them:

- `reader` sees 9 of 11 books, because a row policy hides everything published
  before 1960. Verified in the browser.
- `reader` is refused `EXPLAIN` while still being allowed to read — the payoff
  of making `EXPLAIN` its own action earlier today, now visible as two panels
  answering differently on one screen.
- `stranger` gets `permission-denied` and the UI renders the refusal, with a
  line saying the database refused it rather than the adapter.

Rendering a refusal as a result rather than as an error state is the one real
design decision here: a UI that shows a denied query as an empty table teaches
that the row is missing, when the truth is that the caller may not ask.

## Alternatives rejected

**A charting library.** The chart is one horizontal bar per group. A dependency
for that is a dependency a reader has to trust before they can see a count;
`<div>`s with a width do it.

**Rendering values as bare JavaScript values.** Every cell shows its wire type
in the column header — `u64`, `i64`, `f64`. An `i64` and a `u64` of the same
magnitude are different values to this database, and a grid rendering both as
`1` hides the most surprising thing about the value model.

**Numbers for 64-bit integers.** Values arrive as tagged strings and are
rendered as strings. `JSON.parse` rounds above 2^53.

**Retrying failed queries.** TanStack's default. Turned off: a refusal is a
result the demo is deliberately showing, and retrying it three times only
delays the point.

**A general query builder.** The filter row exposes one predicate. A full
expression builder over HTTP would be a second query language with three
implementations behind it, which is the thing `CONTRACT.md` argues against for
the same reason.

## Evidence

Checked in a real browser via Playwright, not by reasoning about the code: five
screenshots across the tabs and both switchers, with the console watched for
errors. `app` shows 9 rows under a `year >= 1960` filter and `reader` shows the
same query refused nothing but the two hidden books; `stranger` shows two
`permission-denied` panels; the grouped join draws four bars with the counts
the fixture implies; the agreement panel reports all three SDKs returning 11
rows, identical.

`tsc --noEmit` clean under `strict` and `noUncheckedIndexedAccess`. The build
produces 76 kB of JavaScript, 25 kB gzipped.

One console 404 — the favicon — is now an inline SVG. A demo with a red error
in the console teaches the wrong thing about whether it is working.

## What this does not do

No tests. The panels are checked by having been driven in a browser, once, by
me. A component test suite would be worth having and is not here, and the
conformance runner covers the layer below rather than this one.

No `groupBy: "decade"` — the UI offers it and the adapters refuse it, which
surfaces the missing computed-column support rather than hiding it. That is
deliberate but it does mean one dropdown option always errors.

The frontend talks to three hard-coded localhost ports. Fine for
`./run.sh`, useless anywhere else.
