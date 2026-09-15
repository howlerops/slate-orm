# Time functions in the SQL front end, and the two ClickHouse queries they unblock

- **Date:** 2026-09-15
- **Author:** Claude Opus 5, at Jacob's direction
- **Touches:** `crates/slate-kernel/src/scalar.rs`, `crates/slate-kernel/tests/calendar.rs`, `crates/slate-wasm/src/{sql,lib}.rs`, `crates/slate-wasm/tests/datetime.rs`, `crates/slate-server/{proto,src/convert.rs,tests/wire.rs}`, `clients/{python,typescript}`, `site/`
- **Kind:** feature

## What changed

`hour()`, `minute()`, `second()`, `year()`, `month()`, `day()`,
`day_of_week()`, `date()` and `round()` in the SQL front end, as **computed
columns**: a value evaluated per row and appended after the table's own, so it
can be a group key, a sort key or a `HAVING` subject exactly as a column can.

`CalendarPart` is new in the kernel — year, month, day-of-month, day-of-week —
because `TimeUnit` could not carry them. `Round` is new too, and is here
because ClickHouse's Q4 buckets by `round(trip_distance)`.

The two queries that motivated this now run **as ClickHouse writes them**
rather than adapted.

## Why

The ledger recorded this as "the largest gap the page now has": `pickup_time`
is an integer of seconds, so "trips per hour of day" and "by day of week" —
the first two questions anyone asks of a taxi dataset — were not expressible.
It is also why the site's ClickHouse comparisons were adapted: Q3 and Q4 both
key on `toYear(pickup_datetime)`, and there was no year to key on, so Q3
substituted a column that varied and said so.

Most of the capability already existed. `Scalar::Extract` and
`Scalar::DateTrunc` have been in the kernel since scalars arrived, and
`Query::compute` has been appending computed values at ordinals downstream
addresses normally for just as long. Nothing could reach any of it from SQL.

## Alternatives rejected

**A real date type.** The right answer for a database, and the wrong size for
this change: a `ValueType::Timestamp` touches the tuple codec's ordering, the
schema, every client's type mapping and the wire, to make queries that already
work read slightly better. The kernel's own comment on `TimeUnit` says this —
"a real date type would make these calendar operations rather than arithmetic;
that is a type-system change, and this is not pretending to be one" — and it
was right when it was written.

**More variants on `TimeUnit`.** `Month` beside `Hour` is the obvious move and
it is a trap: `TimeUnit::seconds()` is how *both* `Extract` and `DateTrunc` are
implemented, and a month has no fixed number of seconds. `TimeUnit::Month`
would need a `seconds()` that lies, and `date_trunc(month, t)` would compile
and return nonsense. `CalendarPart` is a separate enum for that one reason.

**A date-time crate.** `chrono` or `time` would supply `civil_from_days` and a
great deal else — a parser, a formatter, a timezone database — for four integer
fields. Hinnant's algorithm is a dozen lines, exact, and short enough to read
in the file that uses it.

**An expression grammar, so `hour(x) + 1` parses.** The kernel's `Scalar` is a
tree and would carry it. Rejected because the SQL front end has no expression
grammar anywhere else — `WHERE` takes `col <op> literal` and nothing more — so
a spec able to express more than the parser can produce would be a shape
nobody generates and nothing tests. `ComputeSpec` is deliberately a function
name and a column.

**Naming `day()` after `TimeUnit::Day`.** That would make it the day of the
*epoch*. SQL's `EXTRACT(DAY FROM t)` is the day of the month, and both return
an integer that looks plausible in a column, so the wrong choice here would be
silent. A mutation swapping them is in the table below.

**Leaving the wire protocol alone.** The workspace stops compiling without
handling the new `Scalar` variants, and the cheap way out is an
`unimplemented` arm. Rejected: `tests/wire.rs` is deliberately exhaustive over
`Scalar` so that a new kernel variant fails to compile rather than going
untested, and a kernel that can do something the wire cannot carry is how the
clients quietly fall behind. Two proto fields and an enum.

## Evidence

**The calendar arithmetic, against hand-checked instants.** Transcribed
arithmetic is exactly what is subtly wrong in a way no self-consistency check
catches, so `tests/calendar.rs` uses dates produced with `date -u -d @<seconds>`:
the epoch and the second before it, both ends of the sample, 2024-02-29,
2000-02-29 (a leap year, divisible by 400) and 1900-02-28 (not one, divisible
by 100). Plus a walk over all 366 days of 2024 and 365 weekdays either side of
the epoch.

Six mutations, each restored and re-verified:

| mutation | caught by |
|---|---|
| truncating division instead of floor | `it_agrees_with_dates_worked_out_by_hand` |
| truncating remainder for the weekday | that, and `the_week_advances_one_day_at_a_time_and_wraps` |
| the epoch is a Wednesday | both of those |
| day of month off by one | that, and the leap-year walk |
| the March-based year never fixed up | both |
| the 400-year leap rule dropped | `it_agrees_with_dates_worked_out_by_hand` alone |

The last row is why 1900 and 2000 are in there: nothing else distinguishes the
three leap rules.

**The SQL layer, against an oracle.** `tests/datetime.rs` decodes the committed
sample directly, folds it by hand, and checks the kernel agrees group for
group — 24 hours, 7 weekdays, 31 days, all summing to 100,000. Five more
mutations:

| mutation | caught by |
|---|---|
| `day()` means day-of-epoch | 2 tests |
| `hour()` returns the day of the month | 4 tests |
| find-or-add always adds | all 9 |
| computed ordinals collide with the table's columns | 8 of 9 |
| a computed group key typed from the source column | **nothing — see below** |

**One mutation survives, and it is not a missing test.** `group_value_type`
returns `I64` for a computed key rather than reading the source column's type,
and replacing that branch with `if false` passes everything. It cannot be
caught: `compute_scalar` refuses a non-integer source, so both readings land on
`I64` or `U64`, which share a class rank and compare identically. The branch is
there for the first function that returns a double, where the two stop agreeing
and — per the rank hazard written up in yesterday's `HAVING` entry — the
disagreement is silent. Written down in the code rather than covered by a test
that does not exist.

**The queries.** ClickHouse's taxi Q4, unchanged apart from column names:

```sql
SELECT passengers, year(pickup_time), round(distance), count(*) FROM trips
  GROUP BY passengers, year(pickup_time), round(distance)
  ORDER BY year(pickup_time), count(*) DESC
```

Five browser assertions cover the wasm boundary, which the Rust tests on either
side of it cannot: 24 hours in order summing to 100,000, `compute` surviving
serde in the Spec tab, Q4's four headers, and the refusal for a time function
on a text column. 53 checks in that file now.

## What this does not do

**No timezone handling, at all.** Every one of these is UTC, because a
timestamp here is a count of seconds and nothing records an offset. "Trips per
hour of day" for New York is therefore off by five, and the page does not say
so on screen — this entry and the module docs do. Fixing it properly is the
date type this deliberately did not build.

> **Wrong, and withdrawn on 2026-09-15** by
> [`the-hours-were-already-local`](2026-09-15-the-hours-were-already-local.md).
> The sample stores New York wall clock as if it were UTC —
> `make-trips.py` counts from local midnight and `taxi.rs` adds back the epoch
> second of UTC midnight, so the two conversions cancel. A UTC extraction reads
> the local hour straight out and is **not** off by five. The diurnal curve says
> so: the trough is at 04:00 and the peak at 18:00, where genuine UTC instants
> would put them at 09:00 and 23:00. A fixed-offset argument now exists for
> columns that really are UTC, and the page says which it is.

**No `date_trunc` to a month or a year.** `DateTrunc` takes a `TimeUnit`, which
is fixed-length by construction, so `date()` stops at the day. Truncating to a
month needs the calendar path and a `days_from_civil` to go with
`civil_from_days`; nothing asked for it.

**No time functions on a join or a chain.** `JoinSpec` has one group key and no
computed columns — no `compute` field and no ordinal space to put one in — so
the parser refuses with that reason rather than ignoring the call. Widening the
join spec is a larger change than this one.

> **Closed for joins on 2026-09-15** by
> [`the-hours-were-already-local`](2026-09-15-the-hours-were-already-local.md),
> which added `Join::compute` over the joined row. Chains still have none.

**`year()` is constant on this sample**, because the sample is one month. The
grouping is real and would spread over a longer one; a test asserts the
constant so that swapping the sample fails loudly instead of quietly answering
a different question.

**The clients cannot yet *build* one.** The wire carries `CalendarPart` and
`Round` in both directions and the round-trip property covers them, but no
Python, Go or TypeScript client exposes a builder for either. A client that
wants one today writes the proto by hand.

> **Closed for Python on 2026-09-15** by
> [`the-hours-were-already-local`](2026-09-15-the-hours-were-already-local.md).
> Go and TypeScript turn out to have no `Scalar` surface *at all* — they refuse
> a row carrying computed values — so this was never the two-variant gap it is
> described as here for those two. That is a larger piece of work and is still
> open.

**`round()` is the only non-time function in a list called `TIME_FUNCTIONS`,**
which is now a name that lies slightly. It is really "names that are computed
columns rather than aggregates", and one arithmetic function did not seem worth
a second list to be the only member of. Said in the code, said here.
