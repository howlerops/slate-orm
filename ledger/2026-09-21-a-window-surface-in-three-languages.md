# Windows in the Python, Go and TypeScript clients, and the two tests a mutation run said were missing

- **Date:** 2026-09-21
- **Author:** Claude Code, working on F2a
- **Touches:** `clients/python`, `clients/go`, `clients/typescript`, `scripts/mutate.py`, `scripts/test_mutate.py`, `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

Each of the three SDKs gained a window surface over the protocol F2a put on the
wire: a builder for `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD` and any
ordinary aggregate `OVER` a partition, an `OVER (…)` clause carrying a partition
and the window's own order, a way to name a window from a sort key, and a third
per-row list to read the values out of. Each got a live suite against a real
head node, checked against the `Aggregate` RPC as an oracle.

Then all three were mutation-tested, which produced two findings and one
rewrite. `mutate.py` gained a `go` dialect so the Go client could be tested at
all; the Python window suite was rewritten to seed its own rows; and both Go and
TypeScript gained a `LAG`/`LEAD` test that neither had.

## Why

The kernel has had windows since F2, and the wire has carried them since the
commit before this one, but nothing a user writes could ask for one. A feature
reachable only from a `.proto` file is a feature with one caller — the Rust
tests — and the question a first client always answers is which of the
protocol's assumptions were never written down. Two turned up:

- **Go's zero value is a real aggregate.** `Window` is a struct with one field
  per function and Go has no sum type, so an unset `Aggregate` is `COUNT(*)`,
  not an absence. Sending every field unconditionally would make every `RANK`
  a request the server refuses, and the refusal would read as a server bug.
  `toProto` sends only the field the function uses, and
  `TestARankDoesNotCarryTheZeroAggregate` is that rule.
- **A window value has no declared type**, exactly as a computed value has
  none, so `q.windowed(0).eq(1)` is refused by the *client* before the request
  exists. The test that asserts the server's own "a filter cannot name a
  window" therefore has to write `u64(1)`, or it passes on the wrong refusal.

The three findings from mutation testing are in **Evidence**.

## Alternatives rejected

**Refusing the impossible combinations client-side.** Each client could reject
an unordered `RANK`, a running `COUNT(DISTINCT)` or an offset of zero without a
round trip, and the error would be faster and in the caller's own language.
Rejected: that is four statements of one rule — kernel, Python, Go, TypeScript
— and the three copies drift the first time the kernel's list changes, in the
direction that matters most (a client refusing something the server now allows
is a feature nobody can reach, and it fails silently in the sense that nothing
tests for the absence). The factories' *signatures* still make the common
mistakes hard to write: `lag(column, offset=1)` cannot be spelled without a
column, and `aggregate_over` takes an `Agg` rather than repeating its seven
factories. One statement of each rule, on the server, and a fourth client gets
them free.

**Folding window values into `computed`.** One list is less protocol and less
client code. Rejected in F2a for a reason this work confirmed from the other
side: with one list, "the second computed value" means a different position
depending on how many windows were asked for, so adding a window to a query
silently moves every computed value the caller was reading. The row-shape
assertion in each of the three suites — a `docs` row has four stored columns and
no more, whatever the query computed — is the check that catches a server
folding them together, and it is one line in each because the lists are
separate.

**Not adding a `go` dialect to `mutate.py`, and mutating the Go client by
hand.** Rejected for the reason the script's own docstring gives: doing it by
hand fails in three ways that all look like success. That is not hypothetical
here — the `go` dialect's report marker was written `^(?:ok|FAIL)\s+\S+` first,
and `\s` matches the newline, so a bare `FAIL` from a package that would not
build plus the compiler's next line parsed as a package verdict. A build error
scored as a surviving mutation. The two-case test added beside the dialect is
what caught it, within a minute of the dialect existing.

**Leaving the Python suite on the seeded `docs` rows.** It passed that way, and
`pytest clients/python/tests/test_windows.py` still would. Rejected because the
full suite failed four of its tests: `docs` is shared and half a dozen other
modules write into it, so the table holds 687 rows by the time pytest reaches
this file rather than the 7 the assertions were written against. Seeding eight
rows of its own at an id range nothing else claims also bought the ties the
seeded data lacks, so `RANK` and `DENSE_RANK` can actually disagree.

## Evidence

**Three findings, all from `scripts/mutate.py`.**

1. *The Go client had no `LAG`/`LEAD` test.* Sending `out.Column = nil` for
   both survived the whole Go suite. Every other Go window test uses a ranking
   function or an aggregate, neither of which carries a column, so the one line
   in `toProto` that fills it was exercised only by the two refusal tests —
   which pass whatever the reason for the refusal.
   `TestLagAndLeadStepThroughThePartition` is the missing test; with it, both
   that mutation and `Offset: w.Offset * 2` are caught, and named.

2. *The TypeScript client had the same hole, and the mutation run hid it.*
   `String(w.offset + 1)` was **caught** — by `lag at offset zero is refused`,
   which under that mutation asks for offset 1 and is accepted. That proves the
   offset reaches the server and says nothing about the value coming back. With
   `lag and lead step through the partition` added, `w.offset * 2` and a `lead`
   sent with no column are both caught by it. A mutation caught for the wrong
   reason reads exactly like coverage.

3. *`mutate.py`'s own new dialect was wrong.* Described under **Alternatives
   rejected**; `a go build error is not read as a suite that reported` is the
   case, and it fails against the first version.

**Every other mutation was caught, by a named test:**

| surface | mutation | caught by |
| --- | --- | --- |
| `query.py` | partition dropped; window order dropped; `lag` defaults to 2; windows not sent | 4 named Python tests each |
| `rows.py` | window list decoded from `computed`; `windowed(i)` reads `_computed` | the Python window suite |
| Go `query.go` | rank carries the zero aggregate; partition dropped; order dropped; offset + 1; windows not sent; `LAG` column nil; offset × 2 | 3–4 named Go tests each |
| TS `join.ts` / `query.ts` / `client.ts` | partition dropped; order dropped; offset × 2; `lead` column dropped; windows not sent; row decoded from `computed` | 1–4 named TypeScript tests each |

**Suites, in full rather than one file:** `clients/python` 318 passed
(was 312 passed and 4 failed before the rewrite, 316 before this work);
`clients/go` `go test ./slate/` green with the new case; `clients/typescript`
182 tests, up from 181; `scripts/test_mutate.py` 15 passed, up from 13;
`sh scripts/check.sh` 34 of 34; `python3 site/check/docs.py` green.

## What this does not do

**The SQL front end still refuses `OVER` by name.** That is the last third of
F2a and is untouched here: `slate-wasm`'s `QuerySpec` — the shape the parser
compiles to, which is *not* the gRPC `Query` — has no window field, so a parse
would have nowhere to put the result. The refusal message says so and is
accurate as of this commit.

**The three clients are not compared against each other.** Each new suite runs
against a real head node and agrees with that node's `Aggregate` RPC, which
catches one client being wrong. It cannot catch all three being wrong the same
way, which is what `examples/explorer/conformance` exists for — and no window
case was added there, because the contract and all three demo adapters would
need an endpoint first.

**No client refuses anything locally**, deliberately (see above), so every
refusal costs a round trip. For a mistake in a query builder that is the right
trade; for a hot loop building windows it would not be, and nothing here
measures it.

**Nothing measures the client-side cost of a window at all.** The kernel's
shared-permutation measurement is in F2a's entry; the clients only encode and
decode, and the decode is one extra list per row, which is too small to have
been worth the harness.
