# date_trunc reaches a month and a year, a stale binary is refused in all three harnesses, and one performance concern is withdrawn as measured

- **Date:** 2026-09-15
- **Author:** Claude (Opus 5), with jacob.beck.018@gmail.com
- **Touches:** `slate-kernel` (`scalar.rs`, `lib.rs`, new `examples/flatten_cost.rs`,
  `tests/calendar.rs`), `slate-server` (`convert.rs`, `tests/wire.rs`),
  `records.proto` (both copies), all three clients, `slate-wasm`
  (`lib.rs`, `sql.rs`, `tests/datetime.rs`), the three client harnesses
- **Kind:** feature, process, and one withdrawn hypothesis

## What changed

Three of the recorded gaps, in one commit because they share no code and would
each be a thin one.

**`date_trunc` to a month and a year.** It stopped at the day, and the reason
was real: truncating to a fixed unit is `seconds / 86_400 * 86_400`, and a month
has no fixed length. So `CalendarUnit` joins `CalendarPart` as the second thing
`TimeUnit` cannot express, `days_from_civil` joins `civil_from_days` as its
inverse, and `Scalar::CalendarTrunc` decodes the date, drops the fields below
the boundary and encodes it again. It reaches the wire, all three clients, and
the SQL front end as `month_start()` and `year_start()`.

**A stale prebuilt binary is refused.** `SLATE_TESTSERVER` and `SLATE_SERVERD`
let CI build the server once and hand it to three client suites, and nothing
checked that the binary was newer than the source. All three harnesses now
compare modification times and fail with the file that moved.

**The grouped path's double flatten is not a cost.** Measured, and the
hypothesis withdrawn.

## Why

**The stale-binary guard exists because it happened twice, to me, in one
session.** A full Python run reported 153 passing tests against a
`slate-testserver` built before that session's server changes — so every test of
the new behaviour was exercising the *old* server and passing, because the client
asked for something the old binary politely ignored. It surfaced when three new
tests failed after a rebuild, which is luck. Then it happened again an hour
later with `date_trunc`, in exactly the same way. A guard that costs one
directory walk is cheaper than noticing.

**The double flatten was written up as "a real regression" and it is not one.**
The previous entry recorded that a grouped join with a computed column flattens
each row twice and that this was unmeasured. Keeping the cursor's flat row on
`JoinedRow` and reusing it gave medians of 325.3 and 310.3 ms over two runs,
against 320.9 and 342.2 ms rebuilding it, with a within-variant spread of 290 to
363 ms. The difference is well inside the noise: both paths clone every value
exactly once and only the bookkeeping differs. The change was reverted rather
than kept, because a field and a per-row `Row` are a real cost for no gain.

## Alternatives rejected

**Adding `Month` to `TimeUnit`.** One enum instead of two, and a lie: `TimeUnit`
is *defined* by `seconds()`, which every `date_trunc` and `extract` divides by.
A month member would need a number, 30 days is wrong for seven months of the
year, and the wrongness would be silent. This is the same argument
`CalendarPart` was split out on, and it applies to truncation for the same
reason.

**Offering `CALENDAR_UNIT_DAY` as well.** A day *is* a fixed number of seconds,
so `date_trunc(TIME_UNIT_DAY, t)` already means it. Two spellings of one
operation leave no way to tell which a caller meant, and no way to explain why
they might differ.

**Truncating toward zero rather than flooring.** What `/` does in Rust, and
wrong below the epoch: a December 1969 instant would truncate *forward* into
1970 and land in the wrong year. `div_euclid` throughout, as everywhere else
here.

**Comparing a binary's hash rather than its modification time.** Exact, and it
needs the hash of a binary built from the current tree, which means building it
— which is the thing the environment variable exists to avoid. Modification time
is crude and catches the whole of the real failure: a binary CI just handed over
is minutes old, and one built last week is not.

**Making the stale check a warning.** It would be ignored, and the failure it
guards against is a suite that reports success having tested the wrong server.
That is the same class as "a skip is green", which CLAUDE.md already names as
the worst kind.

**Keeping the `flat: Option<Row>` change anyway**, on the grounds that it is
"more correct" to reuse work. It is not more correct, it is more code: the
measurement says the two are indistinguishable, and the field would have to be
explained forever.

## Evidence

**Every constant in the truncation test was checked against `date -u -d @N`**,
not against the kernel: 1709214300 is 2024-02-29T13:45:00Z, 1706745600 is
2024-02-01, -2678400 is 1969-12-01, -2206310400 is 1900-02-01 — the century that
is not a leap year — and 951868800 is 2000-03-01, the century that is.

**The round trip covers four centuries.** `days_from_civil` is transcribed
arithmetic, and a round trip through its own inverse would agree with itself if
both were wrong by the same day. So
`truncating_and_reading_the_date_back_agree_over_four_centuries` steps a day at
a time from 1700 to 2100 — the full Gregorian cycle, half of it before the epoch
— and requires the truncated instant to read back as the same year and month
with day 1, to be at or before the original, and to be within 31 days of it. It
counts 4,800 month boundaries and 400 year boundaries, which is what says the
loop covered the span rather than exiting early.

**Eight mutations on the new kernel arithmetic, each caught by a named test:**
skipping the March-based year shift; mis-rotating the month; being one day out;
forgetting the epoch shift; dropping the divisible-by-100 rule; truncating
always to the year; returning days rather than seconds; and using `/` instead of
`div_euclid` so a pre-epoch instant rounds the wrong way.

**The stale guard fires, checked in all three harnesses** against a binary dated
2020: Python, Go and TypeScript each refuse with
"`SLATE_SERVERD=… was built before crates/slate-kernel/src/lib.rs was last
changed`". It also caught a genuine case twice during this change, which is how
the release `slate-testserver` came to be rebuilt.

**`slate-server`'s exhaustive `Scalar` coverage check did its job.** Adding a
kernel variant failed to compile `tests/wire.rs`, which is deliberate — the
match is exhaustive "so a new kernel variant fails to compile here rather than
going untested" — and the round-trip property now generates `CalendarTrunc` too.

**Suites:** the kernel's 40-odd binaries, `slate-server`, `slate-wasm` (28 in
`datetime`), 158 Python tests (up from 156), 63 TypeScript, the Go suite,
`cargo clippy --workspace --all-targets`, and the pre-commit hook's 14.

## What this does not do

**`date_trunc` still stops at the year.** A week boundary is missing and is the
one a reader might reach for next: it is not a calendar field, it is a day
offset from an arbitrary weekday, so it needs a convention — ISO's Monday or
SQL's Sunday — and picking one silently is how `day_of_week` nearly went wrong.

**The truncation is UTC, like everything else here.** `month_start` in another
zone is `month_start(t + offset)` shifted back, which is not what a caller
means and is not offered.

**The stale check walks two directory trees per run.** Measured at well under a
second on this tree, and it happens once per suite, not once per test. It does
*not* notice a binary built from uncommitted changes that were then reverted —
modification time cannot.

**Nothing checks the `slate-wasm` bundle's freshness the same way.** The browser
check loads whatever `site/` holds, and `site/build-wasm.sh` is a separate step
a person can forget. That is the same class of gap in a place this commit did
not touch.
