# Paging a join, and what a join's cursor is

`docs/orm-comparison.md`'s N3 asks for keyset paging over a join or a chain and
says, correctly, that the design is the hard part:

> The driving side's key is not unique after a fan-out, so a cursor over it
> either skips the rest of a group or repeats it. … **Write that down before
> building either.** An undesigned cursor that is subtly wrong under
> concurrency is worse than an offset that is obviously wrong under
> concurrency.

This is that note. It is written before the code and its measurements are
reproducible from `crates/slate-kernel/tests/paged_join.rs`.

## What has to be true

Keyset paging exists to make one property hold that `OFFSET` does not:

> **Paging through a result with a cursor, while rows are inserted and deleted
> concurrently, visits every row that existed throughout exactly once.**

`OFFSET` fails it because it counts: a row inserted before the cursor shifts
every later page by one, so a row is served twice or skipped, and nothing
reports either. The single-table cursor holds the property because a key does
not move when its neighbours change.

A join's cursor has to hold the same property. Everything below follows from
that, and the property is the test.

## Three measurements first

The design turns on what the executor actually does, so it was measured before
anything was designed. All three are reproducible and all three are in
`paged_join.rs`; the first two contradicted the documentation that was there.

### 1. Only two of the three algorithms keep the driving side's order

A join's output order is not specified — `join_oracle.rs` says so in its header
and compares multisets for that reason. Measured, over an eight-author,
twenty-book fixture with fan-out:

| join type | nested loop | hash, build left | hash, build right |
| --- | --- | --- | --- |
| inner | **ordered** | not ordered | **ordered** |
| left  | **ordered** | not ordered | **ordered** |
| right | *refused* | not ordered | not ordered |
| full  | *refused* | not ordered | not ordered |

Building the left side destroys its order, because the left rows come back out
of hash buckets. A right- or full-outer join is never ordered by the left key
under any algorithm, because the rows it preserves from the right belong to no
left row at all and are drained after the probe side is finished. A nested loop
refuses a right-preserving join outright, which `join_oracle.rs` already pins.

### 2. A side's `limit` and `offset` are honoured, not ignored

Both `Join`'s doc comment and `JoinInput.query`'s field comment said a side's
`limit`, `offset` and `sort` are *ignored by the kernel*. They are not.
`SecuredReads::execute` ends in `cursor.with_window(query.limit, query.offset)`,
which is the same windowing a single-table read gets. Measured on the same
fixture — 8 authors, 20 books, 18 joined rows with no window:

| side window | nested loop | hash, build left | hash, build right |
| --- | --- | --- | --- |
| `left.limit(3)` | 8 rows, authors {0,1,2} | 8 rows, authors {0,1,2} | 8 rows, authors {0,1,2} |
| `left.offset(5)` | 6 rows, authors {5,6,7} | 6 rows, authors {5,6,7} | 6 rows, authors {5,6,7} |

Honoured, and **the three algorithms agree exactly** on which rows come back.
Nothing was user-visibly broken by the wrong comment, because the wire refuses
all three fields on an input — but it refused them citing a reason that was
false, and this note's design depends on the true behaviour, so both comments
are corrected as part of this work.

### 3. A side's `sort` is worse than either

| side window | nested loop | hash, build left | hash, build right |
| --- | --- | --- | --- |
| `left.sort_by(desc(id))` | descending | **unchanged** | descending |

Not ignored, and not honoured: honoured on the algorithms where the left side
streams, and silently dropped on the one where it is hashed. That is the worst
of the three possibilities and is exactly why the wire refuses it. It stays
refused, and the comment now says this rather than "ignored".

## The design

**A page of a join is a page of its driving table.** The cursor is the *left*
input's primary key, `limit` counts left rows, and every joined row those left
rows produce comes back with them.

It is implemented by giving the left input the window it already honours:

```rust
left.limit  = join.limit      // the page size, in left rows
left.after  = join.after      // the cursor, a left primary key
left.paging = true            // so the left's own refusals fire
```

and nothing else. The page is therefore a contiguous key range of the left
table, and the next page's cursor is the primary key of the last left row this
page read.

### Why this holds the property

Every joined row derives from exactly one left row — that is what left-deep
means. So "each left row is visited by exactly one page" implies "each joined
row is visited by exactly one page", and the first is the single-table cursor's
property, already held, on the left table's own key range. The proof is one
line because the design was chosen so that it would be.

### Why it needs no ordering guarantee

The first measurement says the output order is algorithm-dependent, and this
design does not care, because a page is not defined by a position in the
output. It is defined by *which left rows were read*, which is a key range, and
the left is read in key order because `paging` pins it to the table's own scan
— the same pin `Query::after` already makes for the same reason.

That is why measurement 2 matters more than measurement 1: the three algorithms
disagree about order and agree exactly about *which rows a windowed side
yields*, and this design is built on the agreement rather than on the
disagreement. A paged join can therefore keep the cost model's choice between
the two algorithms that stream the left, and the oracle can keep checking that
they agree.

### What it refuses, and why

- **A right or full outer join.** Its preserved right rows belong to no left
  row, so they are in no left page. Serving them on every page would repeat
  them and serving them on none would drop them. Refused by name.
- **Hash with the left as the build side.** The design does not need the
  output ordered, but it does need to know which left row was read last, and a
  built side is consumed into buckets. Pinning to "the left streams" leaves
  both remaining algorithms available and costs one refusal instead of a second
  way to compute the boundary.
- **`offset` together with a cursor.** Counting and keying are the two ways to
  say where a page starts, and a request carrying both is a caller who has not
  decided. The single-table path has the same refusal.
- **A page with no size.** `limit` is required, as it is on a single-table
  paged read: a page with no size is the whole join.
- Everything `Query::after` already refuses on the left input — an index access
  path, a sort the key does not give — fires from the left's own planning,
  unchanged and with the kernel's own wording.

### What it costs, honestly

**A page's row count is not bounded by `limit`.** Ten left rows with a hundred
matches each is a thousand-row page. This is survivable only because a read is
*streamed* — `rows_per_message` frames it, and `QueryResponse` is a stream
rather than one message — so a large page is slower rather than undeliverable.
It is the reason N2's `max_returned_rows` cap does not apply here and should
not: that cap exists because a `WriteResponse` is a single message.

The alternative bounds the page and costs more than it is worth; see below.

## Alternatives rejected

**A composite cursor over `(left.pk, right.pk)`.** This is the design that
bounds a page at exactly `limit` rows, and it is what a database with an
ordered join would do. It needs the output ordered by that pair, which
measurement 1 says only two algorithms give, and it needs each left row's
matches ordered by the right key, which is only true when the right side is
walked by an index whose suffix is its primary key. So it pins the plan to
index-nested-loop, refuses when the right join key has no index — where the
fallback is a full right scan per left row — and has to resume *inside* a left
row's match set, which is a third kind of cursor state. Bounding the page is
worth something; it is not worth pinning the plan to one algorithm, refusing
joins that page fine under this design, and writing mid-group resumption that
only one algorithm can honour.

**Count joined rows and cut at the next left-row boundary.** `limit` keeps
something closer to its usual meaning and the overshoot is bounded by one left
row's fan-out rather than by the page's. It needs the output ordered by the
left key — measurement 1 again — so it pins the algorithm for a cosmetic gain,
and it makes `limit` mean "at least this many, then up to one group more",
which is harder to explain than "this many left rows".

**Page by counting distinct left rows seen in the output.** Same ordering
requirement, plus it cannot tell a left row that matched nothing from one that
was never read: under an inner join those rows are consumed and emit nothing,
so a page whose left rows all matched nothing would return no rows *and* no
advanced cursor, and the caller would loop forever. The boundary has to come
from the scan, not from the output.

**Refuse, and say paging a join needs a unique `ORDER BY`.** The outcome N3
explicitly allows. Rejected because the refusal would be permanent: a sort the
primary key does not give is exactly what the single-table cursor already
refuses, so "page by a unique ordering" and "page by the driving key" are the
same request, and the second one works.

## Chains

Identical, and simpler: `ChainCursor` materialises each step from the first
table's rows, so the first step *is* the driving scan. `chain.first` takes the
same three fields, the cursor is the last row the first step read, and the
`limit` that used to truncate the finished chain is applied to the first read
instead — which is also the only version that makes paging a chain cheaper
rather than just differently spelled.

A chain refuses the same things, plus one more: `limit` must be applied to the
first step, so a paged chain is a page of the first table and a chain step's
own window is as refused as a join input's.

## What is still not paged

**A grouped read** — `no_cursor_on_groups` refuses a cursor on an aggregate and
keeps refusing it. A page of groups needs the grouping key to be the cursor,
which is a different mechanism with a different refusal set.

**A `Related` path.** Its levels are already batched by key, and a page of a
path would be a page of its roots — reachable by paging the query that produces
the roots and passing that page's keys in. Nothing here blocks it; there is
just no evidence yet that anyone wants the server to do the walking.
