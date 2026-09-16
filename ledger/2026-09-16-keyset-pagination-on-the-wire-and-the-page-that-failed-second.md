# Keyset pagination on the wire, and the unpageable read that only failed on page two

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-kernel/src/{query,read}.rs`, `crates/slate-server/{proto,src/{convert,service,status}.rs}`, `crates/slate-orm/src/ext.rs`, all three clients, `examples/explorer/`, `README.md`, `site/docs/`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`Query.after` carries a cursor on the wire, `Query.paged` asks for the next one,
and `QueryResponse.next_cursor` returns it. All three clients have `page`, which
reads one page and hands back the cursor. Eight cases in the three-SDK
conformance corpus, ten tests in `crates/slate-server/tests/keyset.rs`, and
seven to ten in each client.

The kernel gains `Query::paging`: the intent to resume, separate from carrying a
cursor, because the first page carries none. Two defects fixed on the way, both
described below.

## Why

`Query::after` had been in the kernel since keyset pagination was built and the
proto had no field for it, so every remote caller paged by `offset`. `OFFSET`
counts rows, and is only correct while nothing changes: delete a row ahead of
the cursor between two pages and the reader silently skips one, insert one and
they see a row twice, and nothing reports either.
`paging_visits_every_row_exactly_once_under_concurrent_writes`, in four
languages, is the test that would fail if the server quietly translated the
cursor into an offset.

The **server** builds the cursor. `Records::page_records` already argues for
that at the record layer — paging by hand means knowing which columns are the
primary key and in what order, and a cursor built from the wrong column still
pages, just through the wrong sequence — and the argument is stronger across a
wire, where the client may not hold a schema at all.

`paged` exists because of what that costs. A cursor is the last row's key, so a
projection that drops a key column cannot produce one, and the choice is then
between refusing by name and serving the page with no cursor — the silent
version, where the caller loops until the cursor is absent, gets none on the
first page, and reads ten rows of a large table as the whole answer. Refusing is
only available if the server knows a cursor was wanted, and `LIMIT 10` does not
say that. So the caller does.

## Alternatives rejected

**Let the client extract the key.** No `paged`, no `next_cursor`, one field
instead of three. Every client already declares its schema through
`SchemaCheck`, so it *could* find the key columns. Rejected because the
declaration is optional — a client without one could not page at all — and
because it puts the rule that `page_records` exists to remove back into every
caller, in three languages rather than one.

**Return `next_cursor` whenever `limit` is set and the page was full.** No
`paged` field. It makes the projection case silent instead of refused, which is
the failure this is built to avoid, and it starts attaching a cursor to every
`LIMIT 10` that was never paging.

**Refuse a `sort` on a paged read in the server.** It is what the corpus caught
(below) and it would have worked. It states a kernel rule a second time, in a
place that cannot see the plan: `ORDER BY id` needs no sort and must page fine,
and only the planner knows which sorts survive. The server would have had to
approximate, and an approximation of a correctness rule is a bug waiting.

**A kernel `Query::paging` flag** — chosen. The kernel owns the rule and already
states it; it only needed to be told that a read *will* be resumed.

**`bool paged` rather than the file's `Unit` idiom.** `Unit` exists because a
`bool` inside a `oneof` has two spellings for one choice and the false one is
what a zeroed struct sends. `paged` is not in a `oneof`, and its false spelling
is exactly what a client that has not heard of paging means.

## Evidence

Ten tests in `keyset.rs`. slate-server 212 passing, slate-orm 79, slate-kernel
green across every binary. `cargo fmt --all` and `clippy --workspace
--all-targets` with `-D warnings` clean. Go 7 paging tests in a suite that
passes in 5.3s, TypeScript 91 total, Python 192 total. Conformance:
**72 cases, the three SDKs agree on all of them**.

Mutation run over the server, nine mutations, two rounds:

| mutation | first run | after |
| --- | --- | --- |
| the cursor is dropped on the way in | killed | killed |
| a short page returns a cursor too | killed | killed |
| a full page returns no cursor | killed | killed |
| the cursor is built even when unpaged | **survived** | killed |
| the paged check is skipped | killed | killed |
| a cursor on a join input is silently dropped | killed | killed |
| the projection check accepts a missing key column | killed | killed |
| a paged read with no limit is allowed | killed | killed |
| an invalid cursor is an internal error again | killed | killed |

**Three findings, and none of them came from a test I wrote on purpose.**

*`InvalidCursor` was an `INTERNAL` error.* It fell to the wildcard in
`code_for`, because until the wire had a cursor field no remote caller could
provoke one and the mapping was unreachable. A caller cannot tell "I sent a bad
request" from "the server broke" at `INTERNAL`, and the second invites a retry
that will fail identically forever. Now `INVALID_ARGUMENT`, with an
`INVALID_CURSOR` reason token.

*The survivor was real redundancy, and my comment described an optimisation I
had not made.* Forcing `paged` to `true` in the final cursor decision changed
nothing, because a second guard already stopped the key being collected. Two
checks of one rule, the second unobservable. Collapsing them let the loop hold
the last `Row` by moving it rather than extracting its key on every row — which
is what the comment beside it had claimed all along and was not doing, at two
wasted allocations per row.

*The conformance corpus found a bug the whole design was supposed to prevent.*
"a page sorted into an order the key does not give" was listed as a refusal and
all three SDKs answered it. The kernel's sort refusal sits inside the branch
reached only when `after` is `Some`, so the *first* page of a sorted read came
back happily, with a cursor, and page two was where it fell over — the caller
learning on the second request that the first was never resumable. That is
precisely what `paged` was introduced to move earlier, and my own proto comment
claimed it did. `Query::paging` makes the claim true;
`the_first_page_of_a_sorted_read_is_refused_not_the_second` pins it.

*A mutation hung a mutation run for thirty minutes.* "A short page returns a
cursor too" makes paging never terminate, and every paging loop in the suite was
`loop`/`while True`. A test that hangs on a bug reports nothing, and it took the
whole run down with it. All four loops are bounded now and fail by name;
the harness also has a per-test timeout that reports a hang as a kill rather
than dying.

*My own tests deleted the same row on every iteration.* Caught by reading, not
running: the concurrent-writes loops all deleted `seen[0]`, which exists once.
The second delete would have refused. They delete `seen[deleted]` now and assert
the churn happened at all, because a loop that never churned would agree with
itself perfectly.

## What this does not do

`Related`, `Join` and `Aggregate` have no cursor. A relationship load is bounded
by its keys, a grouped read is refused a cursor by the kernel and should be, and
a join's rows have no single key to resume from — that last one is a real gap
and not a decision, and paging a join is not designed.

The cursor is the primary key and nothing else. Paging in an index's order would
need the index entry as the cursor, and the kernel refuses that by name rather
than approximating it.

Nothing measures the *cost* on the wire. The README's "page 99 read 495
key-value pairs by offset and 5 by cursor" is a kernel measurement and is still
the only one; the same claim through a head node is untested, and I have not
made it.

`slate-tuple`, `slate-schema`, `slate-derive` and `slate-slatedb` were not
re-run after the kernel field was added — the disk in this container could not
hold a full `cargo test --workspace` alongside the build, and I ran the four
crates the change touches instead. CI runs the workspace; if one of those four
breaks, it breaks there and not here, which is worth saying rather than
implying a green I did not see.
