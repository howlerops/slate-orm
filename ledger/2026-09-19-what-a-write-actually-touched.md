# What a write actually touched

## What changed

`slate-server` gained a `WriteObserver` trait, told the row count of every
standalone write. `slate-serverd` implements it over a counter and exposes
`slate_rows_written_total{statement,table}`.

## Why

The purge entry ended: *"No metric. `affected` comes back to the caller and
nothing counts purged rows server-side, so a scheduled purge's effect is
invisible to `/metrics`."*

That is the difference between two states an operator has to tell apart: a
nightly retention sweep working, and a nightly retention sweep running and
erasing nothing. Every counter that existed measured **calls** — requests,
failures, latency — and in both states those are identical.

## Why the general hook, not a purge counter

`slate_rows_purged_total` would have been half the code. It would also have
been a special case of exactly this, and building the special case first is how
a general one never arrives.

Every standalone write already funnels through `Head::autocommit`, which has
the statement, its table, and the committed `Written { affected }` in one
place. One call there gives inserts, updates, deletes, predicate writes and
purges at once. The purge is what made the gap visible, not what the hook is
for.

## Alternatives rejected

**Read the count in the metrics layer.** Where the other counters live, and
impossible: the layer sees a method, a status and a duration. `affected` is in
a protobuf body it never decodes.

**A field on `HeadConfig`.** The natural place, and it breaks every caller —
that struct is built literally in seven places for something six of them want
unset. `Head::observing_writes` is consuming, so it reads as part of building a
node rather than as a mutation of one already serving, and "the counters
started late" is not a state anybody has to reason about.

**Count before the transaction, or on failure.** A write that conflicts is
retried and applies more than once while committing once, so counting attempts
reports a number no row ever had. The observer is told after `Ok`, and a test
asserts a refused purge contributes nothing — a counter that moved on a
permission error would make a misconfiguration look like data loss.

**Skip a zero.** The tempting optimisation and the wrong one: zero is the
number worth watching. A series that only appears once it is non-zero cannot be
told from a node that never swept.

**A label taken from the request.** `MAX_METHODS` exists because method names
arrive from the wire. Neither of these labels does — the statement comes from a
`match` on an enum, the table from the catalog — so this map needs no overflow
bucket, and the `&'static str` means the compiler refuses to build until a new
`Write` variant is named.

## Evidence

Six mutations, all caught:

| mutation | caught by |
| --- | --- |
| the observer is never told | `…reports_what_it_erased_to_the_observer` |
| a zero count is skipped | `…that_erased_nothing_still_reports` |
| the statement label is always the same | `…reports_what_it_erased…` |
| a refused write is counted anyway | `a_refused_purge_reports_nothing` |
| the counter sums into one series | `rows_written_are_counted_by_statement_and_table` |
| an empty write map still emits a header | `…exposes_no_write_family` |

12 tests in `purge_wire.rs`, 28 in `observe`. `cargo clippy` clean over both
crates — it caught the mistake below.

## A mistake worth recording

Inserting the trait above `HeadConfig` put it *between* that struct's doc
comment and the struct, so `HeadConfig` lost its documentation and the trait
gained a paragraph about catalogs. `missing documentation for a struct` is how
I found out, from clippy, after the tests were already green.

Scripted edits that anchor on a `pub struct` line land after its doc block,
which is almost never where a new item belongs. The repair then ate the `#` off
a `#[derive]` — a second slicing error in the fix for the first, found the same
way.

## What this does not do

**Writes inside a transaction are not counted.** `autocommit` is the funnel for
standalone writes; a transaction commits through the session machinery and
never passes this hook. So the metric undercounts on any deployment that uses
transactions, and nothing says so at the scrape. That is a real gap and the
honest reason it is not closed here is that the session path has no equivalent
single funnel — every command applies separately and the commit is elsewhere.

**Nothing scrapes it in a test.** The counter is asserted through
`Counters::prometheus` and the observer through a recording double; no test
starts a daemon, runs a purge and reads `/metrics` over a socket. The three
pieces are each covered and their composition is one line in `serve.rs`.

No alert or dashboard ships with it, and there is still no scheduler — an
operator wanting a nightly purge writes their own cron against the RPC.
