# A Scalar surface for Go and TypeScript, the stale generated stubs it uncovered, and a conformance case that was never checked

- **Date:** 2026-09-15
- **Author:** Claude (Opus 5), with jacob.beck.018@gmail.com
- **Touches:** `clients/go` (new `scalar.go`, `query.go`, `join.go`, `client.go`,
  regenerated `internal/pb`, new `scripts/generate_proto.py`), `clients/typescript`
  (new `scalar.ts`, `query.ts`, `join.ts`, `client.ts`, `index.ts`),
  `clients/python` (`expr.py`, `query.py`, `scalar.py`, `__init__.py`,
  regenerated `_proto`), `examples/explorer` (three adapters, `CONTRACT.md`,
  `conformance.py`)
- **Kind:** feature, and three fixes for things that were quietly not checked

## What changed

Go and TypeScript can build computed values. Both had **no `Scalar` surface at
all** — they could not construct one, could not name one, and dropped the ones a
row carried — which is why the previous entry recorded that "the conformance
runner therefore cannot compare the three SDKs on any of this".

Each gets: the expression builders, three reference kinds (`ComputedAt` /
`computedAt` for an input's own, `JoinComputed` / `joinComputed` for the join's,
alongside the existing column form), `Compute` on a query, an input and a join,
a comparison family that takes any reference, a sort key that takes any
reference, and a way to read the computed values back beside the row rather than
as a tail of it. Python, which had the single-table half already, gains
`JoinQuery.compute` and `JoinQuery.computed(i)`.

And the demo's `"decade"` grouping, which all three adapters refused with
"grouping by decade needs a computed column, which this demo does not declare",
now works in all three — so the conformance runner compares three independent
`Scalar` surfaces rather than three ways of naming a column.

Three things turned up that were not the task:

1. **The committed Go protobuf stubs were stale.** They predate `Scalar.round`
   and `Scalar.calendar_part`, both added two rounds ago, and nothing noticed.
2. **The conformance runner passed on three identical errors.** Agreement was
   the only check.
3. **Two doc comments described a refusal that never happened** — Go's
   `rowFromProto` and TypeScript's `rowFromWire` both said they refused a row
   carrying computed values; both silently omitted them, which is the right
   behaviour described as a different one.

## Why

The previous entry left this as the reason the three SDKs could not be compared
on any of the date-and-time work: "Go and TypeScript still have no computed
columns at all." That is a gap in the clients and never was one in the database
— the kernel has had scalar expressions throughout, and the wire has carried
them since `Query.compute` existed.

**The stale stubs** are the more interesting find. Python regenerates its stubs
into a temporary directory and requires the result to be byte-identical to what
is committed (`test_generated.py`); Go had the same committed artifact and no
such test. So it drifted, and the drift was invisible until something needed a
field that was not there — at which point it reads as `undefined:
pb.Scalar_Round`, which looks like a typo rather than like a protocol that has
moved. `scripts/generate_proto.py` and `TestStubsAreFresh` close it, mirroring
Python's arrangement including its reasoning.

**The conformance guard** matters for the same reason "a skip is green" does.
Three adapters returning the identical error agree, so a case that quietly
became unserveable would keep passing forever. That is not hypothetical here:
`{"groupBy": "decade"}` was in the corpus *as a refusal case*, and the four new
cases that group by a computed decade would have passed exactly as happily if
the feature had never worked. `EXPECTED_REFUSALS` names the cases whose right
answer is a refusal; every other case must come back without an `error`.

## Alternatives rejected

**A general expression language in the demo's HTTP contract.** `CONTRACT.md`
already refuses this for joins — "a general join builder over HTTP would be a
second query language to keep three implementations of" — and the argument is
the same one. `"decade"` is a fixed shape whose *implementation* is a computed
value, which exercises all three `Scalar` surfaces without the contract growing
an expression grammar.

**Hand-bucketing the decade in each adapter.** What the three adapters' comments
already said they would not do: "the adapter doing the database's job", and then
three adapters have to bucket identically for no reason.

**Changing `Eq`/`eq` and friends to take a qualified reference.** It would have
been one family instead of two, and it breaks every existing caller of a
published client for a case most callers do not have. `Compare(ref, op, value)`
adds one function and one operator type instead of six more names, and the
existing shorthands keep working unchanged.

**A new `SortKey` type for qualified references.** `SortKey { column, ref }`
with `ref` winning is uglier than a sum type and keeps `SortKey{Column: 2}`
meaning column 2. A client that has not adopted computed values sees no change.

**Returning computed values as a tail of the row.** The shape the kernel uses
internally, and wrong at a client boundary: an ordinal would stop meaning a
column, and a caller indexing past the table's width would silently get a
computed value instead of nothing. Both clients keep them beside the row —
`RowStream.Computed()` in Go, `withComputed()` in TypeScript.

**Making TypeScript's `withComputed` an addition to the plain iterator rather
than an alternative.** A gRPC stream is consumed once. Offering both and having
the second silently yield nothing would be worse than documenting the choice.

**Generating the Go stubs at build time.** Rejected for the reason
`clients/python/scripts/generate_proto.py` gives at length: a build that depends
on an external code generator fails differently on every machine, and a
committed file shows up in a diff so a protocol change is reviewed rather than
absorbed. The answer to drift is the freshness test, not removing the artifact.

## Evidence

**The stale stubs, measured.** `Scalar_Round`: 0 occurrences in the committed
file, 5 in the regenerated one. `CalendarPart`: 0 against 32. `Scalar_Distance`,
`Scalar_RegexpReplace` and `Scalar_Case`: 5 each in both, so the drift is
exactly the two features added since the file was last written.

**21 mutations, each caught by a named test.** Ten on Go: `ComputedAt` emitting
a plain column, `JoinComputed` emitting an input's, the query and the join
dropping `compute`, both streams dropping the values, `MonthOf` sending `YEAR`,
`DayOfMonthOf` sending `DAY_OF_WEEK`, `Hour` sending `DAY`, `Round` sending
`Length`, and the sort ignoring a qualified reference. Ten on TypeScript, the
same list. Plus the guard itself, below.

**One mutation survived the first pass**, in both clients at once: no test
sorted by a computed value, so ignoring `SortKey.ref` and sorting by `column`
— which defaults to 0 — went unnoticed. The fixture's years are deliberately a
different permutation from its ids, so the two orders cannot coincide.

**The conformance guard is load-bearing**, checked by breaking it: renaming the
`decade` grouping to one that does not exist makes all three adapters return the
identical `{"error": ..., "message": "no such grouping: decayed"}`, and the
runner now fails with "all three refused it, and this case is supposed to return
an answer" rather than passing. It also immediately found a case I had not
listed — "a reader may not explain a grouping" — which is the shape of evidence
worth more than the test passing.

**The oracles are each language's own calendar.** Go's `time` and JavaScript's
`Date`, over five instants including a leap day and one before the epoch, which
are the two cases a naive `days / 365` or an unsigned division gets wrong.
Computing the expected year by doing what the kernel does would prove only that
the arithmetic is reproducible.

**The decade groups are real, not three identical failures.** Checked by hand
against all three adapters on a live stack: five decades, 1950 through 1990,
counts 1/4/2/2/1, summing to the fixture's ten books, with the key coming back
as `i64` from all three.

**Suites:** 38 conformance cases across three SDKs, 156 Python tests (up from
153), the Go suite including `TestStubsAreFresh`, 62 TypeScript tests, the
workspace's Rust suites, `cargo clippy --workspace --all-targets` clean, and the
pre-commit hook's own 14.

## What this does not do

**The Python suite was running against a stale binary and I nearly missed it.**
`SLATE_TESTSERVER` pointed at a `target/release/slate-testserver` built before
this session's server changes, so the first full run — 153 passing — exercised
none of the new server behaviour. The three new Python tests failed once the
binary was rebuilt, which is what surfaced it. Nothing in the harness checks
that a prebuilt binary is newer than the source it was built from, and that is
still true.

**No client can express a `Chain::compute` over three or more tables.** The wire
carries it — `JoinQuery.compute` is one field for both shapes — and all three
clients can build a chain, so this is reachable today by writing the compute on
a three-input `JoinQuery`. It has no test in any client suite.

**The demo's computed value is one expression.** `books.year / 10 * 10` is
arithmetic; nothing in the conformance corpus exercises a calendar function, a
`CASE`, a regex replace or a vector distance across the three SDKs. The
per-client suites cover the calendar functions; the cross-client comparison does
not.

**Go and TypeScript still cannot read an *input's* own computed values back.**
The wire carries them in each input's `Row.computed` and both clients drop them
on the join path, as they always have. Only the join's own values come back.
Python reads both.

**The `decade` grouping changes what the demo's chart can draw**, and the web
front end has not been touched: it still offers author and country. The
capability is reachable through the contract and not through the UI.
