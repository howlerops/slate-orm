# A helpdesk, written against the Rust surface

A small multi-tenant helpdesk built on `slate-orm` the way somebody building
on it would build one: four `#[derive(Record)]` tables, a service layer that
takes a `SecurityContext` and hands back domain types, and a runner that puts
the whole thing on SlateDB over an object store.

```sh
cargo test -p slate-helpdesk           # the service, over MemoryStore
sh examples/helpdesk/run.sh --smoke    # the same service, over SlateDB
sh examples/helpdesk/run.sh --s3       # and over a real signed S3 server
```

## Why it exists

Everything else in this repository exercises the record layer the way a
*test* does. The demo goes through the head node and three SDKs; the deployed
harness loads a hundred thousand taxi trips and folds them; the oracles
compare a plan against a full scan. None of them is somebody sitting down to
write an application against the Rust surface and finding out what that is
like — and a surface is only as good as the thing you cannot say in it.

So the point of this crate is not the helpdesk. It is the list below.

## What it could not say

Each of these is a database feature reachable from the TOML schema or the
wire and **not** from `#[derive(Record)]`, and each has a test that fails if
the surface changes.

**One of the five is closed, and it closed the way the table said it would.**
Row 1's second test was written asserting that the store accepted a bad
status, with a comment saying *"if the derive ever grows `check`, this test
starts failing and that is the signal to delete it."* The derive grew it, the
test failed, and it was inverted rather than deleted — a test that the rule is
*below* the service is worth more than one that it is not. The row is struck
rather than removed because the list is the point of this crate and a gap
that closed is the most useful kind of entry in it.

| | The gap | Demonstrated by |
|---|---|---|
| 1 | ~~**No `CHECK`.**~~ **Closed 2026-10-01.** `#[record(check(...))]` exists, and `Ticket::status` uses it: the four-status rule is in the schema, carries the sentence a form shows, and refuses a write that goes around `Helpdesk::open_ticket`. The derive declares a **foreign key** too, and `Comment` has two — one cascading, one restricting. `ledger/2026-10-01-a-check-the-derive-could-not-declare.md` and `ledger/2026-10-01-a-foreign-key-the-derive-could-not-declare.md`. | `the_same_status_written_past_the_service_is_refused_by_the_schema` (which used to assert the opposite and says so), `the_statuses_and_the_check_agree`, `a_comment_on_a_ticket_that_does_not_exist_is_refused`, `deleting_a_ticket_takes_its_comments_and_deleting_an_author_does_not` |
| 2 | **No text index.** Index options are `name`, `id`, `unique`, `desc`, `columns`, `only_where`. The search box needed one, so `TICKETS_TABLE` restates the whole table by hand. | `the_hand_written_tickets_table_matches_the_derived_one`, which is the cost made into a guard |
| 3 | **Soft delete has no read.** `#[record(soft_delete)]` works and the close stamps rather than erases, but `Deleted` appears nowhere in `crates/slate-orm/src`, `visible_row_with` is private, and there is no `restore`. An application can close a ticket and cannot reopen it. | `a_closed_ticket_is_hidden_and_cannot_be_reopened_from_here` |
| 4 | **A paged read cannot be a sorted one.** `Query::after` pins the access path to the table's key range, so the inbox pages in primary-key order and `most_urgent` is a separate, unpaged read. | `a_sorted_inbox_page_is_refused_rather_than_silently_misordered` |
| 5 | **A write a policy forbids says two different things.** An insert says "row-level security forbids writing this row"; an update says "table `tickets` has no row with this primary key", which is indistinguishable from a row that is gone. | `a_write_a_policy_forbids_says_two_different_things` |

Number 4 is a *good* refusal — a cursor into an order the primary key does
not describe would skip and repeat rows — and it is still something an
application has to design around, which is why it is in the table.

Number 5 is very likely deliberate: it is the same disclosure decision
`Helpdesk::by_reference` makes on purpose, because telling a caller that a
row they may not touch *exists* is a leak. The read path's version is
documented and this one is not.

Two smaller things, recorded because they cost time rather than because they
are defects: `Records` is implemented on `RecordTransaction` and not on
`RecordStore`, so every operation opens and commits a transaction; and
`begin`/`commit` raise `KernelError` while every `Records` method raises
`OrmError`, so an application's error enum carries both.

## What it does

`src/lib.rs` is the whole application. The tables are `tenants`, `agents`,
`tickets` and `comments`, all tenant-prefixed except `tenants` itself. The
service does: raise a ticket, read one by the reference a customer quotes,
take one, log billable time in hundredths of an hour, comment, read a thread,
close, page the queue, list the most urgent, roll work up per assignee, and
search.

The interesting parts, in the order somebody reading it should meet them:

- **`TICKETS_TABLE`** — the table written out by hand so it can carry two
  text indexes. `Indexed` wraps `Ticket` so the derive still does the row
  codec; only the schema is duplicated, and a test holds the two together.
- **`security()`** — three roles and eight policies. The write rule is *two*
  policies rather than one `Expr::Or`, because policies are permissive and
  combine with `OR` (as in PostgreSQL) and a named half gives a better
  refusal. The tenant boundary is deliberately **not** here: it is the key
  prefix, below this layer.
- **`Helpdesk::inbox`** — keyset paging, and the comment explaining why it
  cannot also be sorted.
- **`Helpdesk::workload`** — one grouped read rather than a scan and a fold,
  and the null group that is the unassigned pile.
- **`read_workload`** — where a grouped read loses the derive's help, because
  the aggregates come back positionally.

## The runner

`examples/over_slatedb.rs` is the part that makes the crate's central claim
true rather than asserted. `Helpdesk<S>` is generic over `KvStore`; the tests
run it over `MemoryStore`, which is a `BTreeMap`, and until something drove
it against an LSM on a bucket that was a claim about a type parameter.

It plans and applies the migration, checks that a second apply has nothing
to do, seeds two tenants, runs every read and every refusal above, then
**closes the store and opens a fresh one over the same prefix** and re-asks
each question. The reopen is the point: a run that writes and reads in one
process proves the memtable works. The index is the sharpest of the re-asked
questions, because `ledger/2026-09-15-a-new-index-returns-nothing.md` is
about an index that existed and was empty.

It collects every failed expectation rather than stopping at the first, for
the same reason `scripts/check.sh` does: a run over an object store is the
expensive kind, and one defect per run is a slow way to find three.

`--s3` runs the identical binary against `s3s` speaking actual signed S3,
and it has been run — sixteen checks, same result. Nothing re-runs it.
The default is a `LocalFileSystem` object store, which is the same LSM with
the same SSTs, WAL and manifest —
`crates/slate-slatedb/examples/bucket_layout.rs` makes the same choice and
gives the same reason. What the filesystem does not reproduce is S3's
latency, conditional-write semantics and error shapes, and this runner
asserts none of those.

## What is not here

- **No benchmark.** Nothing in this crate is timed and no number it prints is
  about speed. Four benchmark crates measure that and CI runs them at
  `--smoke`.
- **No head node, no SDK.** This is the Rust surface directly on SlateDB,
  which is the layer that had no application over it. The other path is
  proved three ways already — `examples/explorer`, the conformance runner and
  `examples/deployed`.
- **Not an audit of the derive.** The five findings are what a handful of
  ordinary requirements happened to hit. Enumerating everything the derive
  cannot say means reading `crates/slate-derive` rather than using it, which
  is a different and weaker exercise.

`ledger/2026-09-30-an-application-written-against-the-rust-surface.md` and
`ledger/2026-09-30-the-helpdesk-on-slatedb.md` are the entries.
