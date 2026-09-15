# The four gaps the time-functions entry left open, and the wrong claim it left standing

- **Date:** 2026-09-15
- **Author:** Claude (Opus 5), with jacob.beck.018@gmail.com
- **Touches:** `slate-kernel` (`join.rs`, `read.rs`), `slate-wasm` (`sql.rs`,
  `lib.rs`, `taxi.rs`), `clients/python` (`scalar.py`), `slate-slatedb`
  (`examples/bucket_layout.rs`), `site/` (workbench, docs, `data/bucket.json`),
  `records.proto`
- **Kind:** feature, and one fix for a silent wrong answer

## What changed

Four recorded gaps closed, and one recorded claim withdrawn as wrong.

A fixed-offset timezone argument on the SQL time functions
(`hour(pickup_time, '-05:00')`), which needs no kernel and no wire change
because a zone conversion *is* an addition. `Join::compute` in the kernel —
values computed over the joined row, appended after every table's columns —
and the SQL front end that reaches it, so `SELECT hour(pickup_time), count(*)
FROM trips JOIN zones ... GROUP BY hour(pickup_time)` runs where it used to be
refused. Python builders for `CalendarPart` and `Round`. A provenance block in
the committed bucket listing, and a test that fails when it stops describing
what the site ships.

Along the way, a defect: **grouping** a join whose side computed a value of its
own was accepted and silently answered a different question. It is now refused
on that path, and the refusal names the field that works.

## Why

The last entry's "what this does not do" section listed these as open, and one
of the things it said was untrue.

**The timezone claim.** It recorded that "trips per hour of day for New York is
therefore off by five", reasoning that the column is epoch seconds and the
extraction is UTC. Both premises hold; the conclusion does not.
`site/data/make-trips.py` stores each pickup as an offset from
`2024-01-01 00:00:00` **local**, and `taxi.rs` adds back the epoch second of
`2024-01-01 00:00:00` **UTC**. The two conversions cancel, and a UTC extraction
reads the New York wall clock straight out. Every hour-of-day number the site
has ever shown was right, and the entry told readers it was five hours wrong.

That left a real gap underneath it, though: for a column that genuinely holds
UTC instants there was no way to ask about any other zone.

**The join.** `JoinSpec` had no computed columns, so the parser refused. Going
to add one turned up why: `JoinedRow::flatten` truncates each side to its
*table's* declared width, so a value a side's `Query` appended was dropped, and
the ordinal it should have occupied read the right table's first column
instead. `review_grouped_join.rs` already had a test pinning that as behaviour
— written on the grounds that "the fix is a refusal, which is a decision rather
than an arithmetic error". The decision is made here.

The first attempt at that refusal was too broad, and the suite said so. It went
into `Join::validate`, which covers every path, on the reasoning that the value
"is dropped on every path — the truncation is in `flatten` and in the cursor
alike". Only the first half is true: the ungrouped path never flattens, and
hands each input's computed values back beside its columns in `Row.computed`,
split per input on the wire. That is a documented feature with its own test,
`a_join_inputs_computed_values_come_back_beside_its_columns`, which went red.
The rule is about flattening, not about joins, so it now lives in
`read::no_side_computes` and applies to the grouped join and the grouped chain.
I had written a kernel test asserting the broad version, and it passed, because
I wrote it to match the code I had just written rather than the behaviour that
existed.

## Alternatives rejected

**A timezone database, or `AT TIME ZONE 'America/New_York'`.** This is what
ClickHouse does and it is what a reader will reach for first. It needs the IANA
database — megabytes, in a binding whose whole point is that it fits in a
browser tab — and the failure mode of approximating it is not an error but a
wrong hour for a third of the year, in a histogram that still looks completely
reasonable. Refused with that reason, in the parser, where the reader sees it.

**An `offset` field on `Scalar::Extract`, `CalendarPart` and `DateTrunc`.** The
obvious shape, and it would have meant three kernel variants changed, three
proto messages changed, `convert.rs` changed, and every client's builders
changed — to express something `Scalar::Add` already expresses exactly.
`compute_scalar` wraps the column instead, so the planner, the wire, the
covering scan and the round-trip property all handle it with no change at all.
The cost is that the offset is invisible in the kernel's own types: a `Scalar`
tree cannot tell you it came from a zone. Nothing needs that yet.

**Widening `JoinSchema` so each side carries its own computed slots.** Would
have made a side's `Query::compute` work rather than refusing it. Rejected
because the right table's ordinals would shift by the left side's computed
count, silently re-pointing every existing caller's ordinals — the exact
failure `ColumnRef` exists to prevent, reintroduced one level down. Putting the
values on the *join*, after every table, is the one place an ordinal can be
added without moving one that already exists. It is also strictly more
expressive: a computed value over the joined row can read both sides, which a
per-side one could not.

**Leaving the side-computed case as documented behaviour.** It had a test
saying it was known. But a wrong answer that looks like a right one is the
worst thing this can do, and "known" is not "acceptable" — the same file's
`having` validation refuses the identical mistake two hundred lines up. The
inconsistency was the argument.

**Regenerating the bucket listing in CI.** The honest check, and still not
worth it: it needs SlateDB, an object store and a 100,000-row load to produce a
picture, and it *still* could not compare — the SST names are ULIDs and the
sizes move with block packing. The provenance block checks what the listing is
a listing *of*, which is what actually goes stale, in about a millisecond with
no SlateDB at all.

**A hash for the schema fingerprint.** Two hex strings tell a reader that
something changed. `duration:I64` next to `duration:F64` tells them what. Same
line count.

**Adding a `Scalar` surface to the Go and TypeScript clients.** They have none
at all — they refuse a row carrying computed values — so this was never the
two-variant gap the last entry described for them. It is a feature, not a
follow-up, and smuggling it in here would have made this change unreviewable.
Left open and stated.

## Evidence

**The timezone claim is wrong, and the curve says so.** Grouping the sample by
`hour(pickup_time)` gives its minimum at hour 4 (549 trips) and its maximum at
hour 18 (7,278) — the New York diurnal curve, trough before dawn and peak at
the evening rush. Genuine UTC instants would put those at 09:00 and 23:00.
`the_hours_are_new_york_local_which_is_what_the_curve_says` asserts both
extremes and the 8-versus-4 ratio, so a fixture whose two endpoints happened to
land right could not pass.

**The join defect, before the fix.** Grouping a two-column `authors` joined to
`books` by `Ordinal(2)` — the slot a left-side computed `id * 100` would occupy
— returned 24 groups keyed 0..23. That is `books.id`. The computed value has
six distinct values. No error anywhere.

**The over-broad refusal was caught by a test I did not write.** Putting it in
`Join::validate` broke the ungrouped path's per-input computed values, and
`slate-server`'s `multi.rs` failed within the same run. My own kernel test for
that case asserted the wrong thing and passed — worth recording, because it is
the failure mode CLAUDE.md warns about: a test written against the code instead
of against the behaviour proves only that the code is what it is.

**Ten mutations on the new Rust, each caught by a named test:** dropping the
minutes from an offset (`a_half_hour_offset_is_not_rounded_to_the_hour`);
flipping its sign, and ignoring it entirely
(`a_fixed_offset_rotates_the_hours_and_keeps_every_trip`); `Sub` for `Add` in
the shift; landing a join's computed column after the left table only, which is
the original bug (`the_join_spec_puts_the_computed_column_past_both_tables`);
`flatten_computing` dropping the values; `narrowed_join` no longer reading the
computed inputs; re-allowing a side's computed column; dropping the
grouping-ordinal validation; and leaving the computed slots out of
`JoinSchema::width`.

**Three on the Python builders**, each caught: `day_of_month` → `DAY_OF_WEEK`,
`year` → `MONTH`, and `round` → identity. The oracle is Python's own
`datetime`, which knows the Gregorian calendar independently of the kernel's
transcribed `civil_from_days`.

**Three on the bucket provenance**, each caught: dropping `passengers`'
nullability (`the_listing_describes_the_current_schema`), an object in a
directory the page cannot label, and a WAL entry restored to 11 MB
(`the_total_is_not_twice_the_data`).

**The regenerated listing reproduces the committed one byte for byte** apart
from the two ULIDs — same twelve objects, same 11,538,479-byte SST, same 11.0
MB total. So the provenance block was added to a listing that is still true,
not to a stale one.

**Suites:** the kernel's 40-odd binaries, `slate-wasm` (21 in `datetime`, 4 in
the new `bucket_provenance`, 2 in `examples`), 153 Python tests against a live
head node, and the browser check at 60 assertions including six new ones.

## What this does not do

**The gRPC protocol does not carry a join's computed column.** `Join::compute`
is reachable from the browser binding and not over the wire, because
`ColumnRef` has no way to name a slot belonging to the join rather than to an
input, and giving it one means reworking the joined ordinal space in
`convert.rs`. The proto comment that used to say a computed value has no slot
in a joined row now says which layer that is true of. This is the largest thing
left open here.

**Go and TypeScript still have no computed columns at all**, so they cannot
build a time function, a zone shift, or anything else in `Scalar`. Python can.
The conformance runner therefore cannot compare the three SDKs on any of this.

**Chains still have no computed column of their own.** Only two-table joins got
one. A grouped chain now *refuses* a step's computed value rather than dropping
it — the same flatten, the same truncation — but there is nowhere to redirect
it to, so the message names a missing feature instead of an alternative.

**A join's computed column reads the left table and its aggregates read the
right.** So "group by the hour and average the fare" is not expressible over
`trips JOIN zones` — the workbench example filters on the right side and counts
instead. That is a `JoinSpec` limitation older than this change and untouched
by it.

**`Join::having` cannot see a computed value**, deliberately: `having` decides
whether a pair is admitted and the computed values are produced from the
admitted pair, so it would be reading a value that does not exist yet.

**Fixed offsets do not know about daylight saving**, which is the whole reason
region names are refused. `hour(t, '-05:00')` over a range spanning March will
be an hour out for part of it, and nothing warns.

**The bucket listing's byte counts are still unchecked.** The provenance block
catches a schema or sample change; it cannot catch SlateDB changing its block
packing, which would make the sizes stale while every test passed.

**`date_trunc` still stops at the day.** Truncating to a month needs a
`days_from_civil` to match `civil_from_days`, and nothing asked for it.
