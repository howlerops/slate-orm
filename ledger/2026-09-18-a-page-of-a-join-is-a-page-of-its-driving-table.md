# A page of a join is a page of its driving table

- **Date:** 2026-09-18
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `docs/paging-a-join.md` (new), `crates/slate-kernel/src/{join,chain,read}.rs`, `crates/slate-kernel/tests/paged_join.rs` (new), `crates/slate-server/proto/slate/v1/records.proto` (+ the TypeScript copy), `crates/slate-server/src/{convert,service,session}.rs`, all three clients and their suites, `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

`JoinQuery` grew `after` and `paged`; `JoinResponse` grew `next_cursor`. The
kernel grew `Join::after`/`Chain::after`. Three clients grew `page_join`,
`PageJoin` and `pageJoin`, and a `JoinPage` to hand back.

**A page of a join is a page of its driving table.** The cursor is input 0's
primary key, `limit` counts input-0 rows, and every joined row those rows
produce comes back with them.

18 kernel tests, 9 Python, 7 Go, 6 TypeScript. 83 conformance cases still
agree.

## Why

`Query` had `after` and `paged` from P3; `JoinQuery` had `offset` and no
cursor. Paging a join was therefore offset paging — the thing P3's cursor
exists to replace. A row inserted between pages shifts every later page by one,
so a caller walking a join sees a row twice or not at all, and nothing reports
either.

## The design, which was the item

`docs/orm-comparison.md` said to write the design down before building either
option, and that the item might legitimately end as a named refusal:

> An undesigned cursor that is subtly wrong under concurrency is worse than an
> offset that is obviously wrong under concurrency.

`docs/paging-a-join.md` is that note, written first. Its conclusion is one
line, and the property falls out of it in one more: **every joined row derives
from exactly one input-0 row**, because the join is left-deep — so "every
input-0 row is read by exactly one page" gives "every joined row is returned by
exactly one page", and the first is what `Query::after` already holds on that
table's own key range.

So paging is a *rewrite*, not an execution mode: the left input is given the
cursor, the page size and `paging`, and the join runs as it always did.

## Three measurements, taken before anything was designed

**1. Only two of three algorithms keep the driving side's key order**, and none
does for a right or full outer join, whose preserved rows belong to no left row
and are drained after the probe side ends. This is why the cursor is *not* a
position in the output: one that was would page differently depending on what
the cost model picked that day.

**2. A side's `limit` and `offset` are honoured, not ignored** — and the three
algorithms agree *exactly* on which rows a windowed side yields. `left.limit(3)`
gives the same 8 rows over authors {0,1,2} on all three. This design is built on
that agreement rather than on the disagreement in (1).

**3. A side's `sort` is honoured where that side streams and silently dropped
where it is hashed.** Worse than either being ignored or honoured, because what
it does depends on a costing decision.

## Two comments that were wrong, and had been

`Join`'s doc comment and `JoinInput.query`'s field comment both said the kernel
*ignores* a side's `limit`, `offset` and `sort`. It does not: `SecuredReads::execute`
ends in `with_window`. Nothing was user-visibly broken, because the wire refuses
all three on an input — but it refused them citing a reason that was false, and
this design depends on the true behaviour. Both are corrected, and the Go
client's copy of the same sentence with them.

## Alternatives rejected

**A composite cursor over `(left.pk, right.pk)`.** The design that bounds a
page at exactly `limit` rows, and what a database with an ordered join would
do. It needs the output ordered by that pair — measurement 1 says two of three
algorithms — and each left row's matches ordered by the right key, which holds
only when the right side is walked by an index whose suffix is its primary key.
So it pins the plan to index-nested-loop, refuses joins whose right join key
has no index, and needs a third kind of cursor state to resume *inside* a left
row's match set. Bounding the page is worth something; it is not worth pinning
the plan, refusing joins that page fine under this design, and writing
mid-group resumption only one algorithm can honour.

**Count joined rows and cut at the next left-row boundary.** Keeps `limit`
closer to its usual meaning and bounds the overshoot by one row's fan-out. Still
needs the output ordered by the left key, so it pins the algorithm for a
cosmetic gain, and makes `limit` mean "at least this many, then up to one group
more" — harder to explain than "this many left rows".

**Page by counting distinct left rows seen in the output.** Same ordering
requirement, and it cannot tell a left row that matched nothing from one never
read. Under an inner join those rows are consumed and emit nothing, so a page
whose left rows all matched nothing returns no rows *and* no advanced cursor:
the caller loops forever. The boundary has to come from the scan, which is why
`page_end` tracks the left rows *read*.

**Refuse, and say paging a join needs a unique `ORDER BY`.** The outcome the
item explicitly allowed. Rejected because the refusal would be permanent: a
sort the primary key does not give is exactly what the single-table cursor
already refuses, so "page by a unique ordering" and "page by the driving key"
are the same request — and the second one works.

**A separate `ChainQuery` on the wire.** A chain is already a `JoinQuery` with
more inputs, so one pair of fields covers both and the server routes on the
input count as it already did.

## Evidence

21 `slate-server` test binaries, 66 across `slate-kernel` and `slate-orm`, 238
Python, the Go suite, 128 TypeScript. `cargo clippy --workspace --all-targets`
clean under `-D warnings`; `cargo fmt --all -- --check` clean. **83 conformance
cases: the three SDKs agree on all of them.**

**Sixteen mutations, two survivors, both now killed.**

| mutation | result |
| --- | --- |
| the cursor is dropped: every page is page one | killed |
| the join's own limit is left on, truncating joined rows too | killed |
| a right or full outer join may page | killed |
| the build side is not pinned when paging | killed |
| a forced hash-build-left is allowed to page | killed |
| an offset alongside a cursor is allowed | killed |
| a page with no size is allowed | killed |
| the left query is not told it is paging | killed |
| the boundary is never recorded on the hash path | killed |
| the boundary is never recorded on the nested-loop path | **survived** → killed |
| a chain's page size is not applied to the first step | **survived** → killed |
| a chain's cursor is dropped | killed |
| a right-outer chain step may page | killed |
| the chain boundary is taken after the steps | killed |

**A mutation found a defect in the test suite, not in the code.** Dropping the
cursor made `a_page_boundary_falls_between_left_rows` **hang** rather than
fail: it walked pages in an unbounded `loop` and the cursor came back non-empty
forever. A test that hangs under a mutation is a test that hangs CI, and that
is strictly worse than one that fails — a failure names itself in seconds, a
hang is a timeout nobody can attribute. Every walk in the file is bounded now
and the bound is an assertion, not a quiet `break`.

**The cost model always picked the hash join on this fixture**, so the nested
loop's boundary tracking was dead code as far as the suite knew — deleting it
survived every test. Two tests close it: one forces the loop and checks its
boundary directly, and one runs both streaming algorithms over the same walk
and asserts they agree about the rows *and* about where each page ends. That
second one is the paged version of what `join_oracle.rs` establishes for rows.

**A union over pages says nothing about where the pages divide.** Never
applying the page size to a chain's first step survived
`paging_a_chain_visits_every_row_exactly_once`, because page one then returns
the whole chain, its cursor is the last first-table row, page two is empty, and
the union is still exactly right. Counting the first table's distinct rows in
one page is what catches it.

**The concurrency property is tested through a real head node**, not only in
the kernel: the Python suite inserts rows behind the cursor between pages and
asserts every row that existed throughout is seen exactly once.

## What this does not do

**A page's row count is not bounded by `limit`.** Ten driving rows with a
hundred matches each is a thousand-row page. Survivable only because a read is
streamed — which is exactly why N2's `max_returned_rows` cap does not apply and
should not: that cap exists because a `WriteResponse` is one message. A caller
who needs a bounded *response* needs the composite cursor rejected above.

**No per-level predicate, ordering or limit.** Unchanged from the join itself.

**Grouped reads still refuse a cursor**, and keep refusing it:
`no_cursor_on_groups` is right that a cursor names a row and a grouped read
returns groups. Paging groups needs the grouping key as the cursor, which is a
different mechanism with a different refusal set.

**`Related` paths are not paged.** A page of a path would be a page of its
roots, reachable today by paging the query that produces the roots and passing
those keys in. Nothing blocks the server doing the walking; there is no
evidence anyone wants it to.

**Nothing measures the cost.** Every page costs what the first costs, because
the cursor narrows the driving scan's key range — that is the mechanism
`Plan::resume_after` already provides and its reasoning is quoted rather than
re-measured. No number here says what a paged join costs against an offset one
at depth.
