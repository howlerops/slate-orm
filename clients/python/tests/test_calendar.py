"""`calendar_part` and `round`, which the wire has carried since they existed
and no client could build.

The oracle is Python's own `datetime`, which knows the Gregorian calendar
independently of the kernel's transcribed `civil_from_days`. That is the point
of using it: a hand-written expectation would be this module's author agreeing
with themselves, and the leap rules are exactly where that goes wrong.

`docs.size` is 5..65, so it is multiplied up into a real spread of instants
rather than asserted about the first minute of 1970.
"""

from __future__ import annotations

import datetime

from slate import Agg, AggregateQuery, Client, Query, asc
from slate.scalar import (
    CalendarPart,
    calendar_part,
    month_start,
    day_of_month,
    day_of_week,
    lit,
    month,
    round_,
    year,
    year_start,
)
from slate.values import i64

from .fixture import DOCS

#: Turns `size` (5, 15, ... 65) into instants from 1977 to 2072, which spans
#: enough years, months and weekdays that a wrong calendar cannot agree by
#: coincidence — and crosses 2000, the century leap year that the every-100
#: rule alone gets wrong.
SPREAD = 50_000_000

SIZES = [5, 15, 25, 35, 45, 55, 65]


def _instants() -> list[int]:
    return [size * SPREAD for size in SIZES]


def _utc(seconds: int) -> datetime.datetime:
    return datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc)


def test_calendar_parts_agree_with_pythons_own_calendar(client: Client) -> None:
    q = Query(DOCS)
    scaled = q.c.size * i64(SPREAD)
    q.compute(
        year(scaled),
        month(scaled),
        day_of_month(scaled),
        day_of_week(scaled),
    )
    q.sort(asc(q.c.id))
    rows = list(client.query(q))

    got = [[row.computed(i) for i in range(4)] for row in rows]
    want = []
    for seconds in _instants():
        when = _utc(seconds)
        want.append(
            [
                when.year,
                when.month,
                when.day,
                # Python's Monday-is-0 to the wire's Sunday-is-0, which is
                # ClickHouse, MySQL and SQLite's convention.
                (when.weekday() + 1) % 7,
            ]
        )
    assert got == want


def test_day_of_month_is_not_the_day_of_the_epoch(client: Client) -> None:
    """The two are both integers and both plausible in a column.

    `day_of_month` is `EXTRACT(DAY FROM t)`; `extract(TimeUnit.DAY, t)` counts
    days since 1970. Confusing them is silent, so this pins the difference
    rather than trusting the name.
    """
    q = Query(DOCS)
    q.compute(day_of_month(q.c.size * i64(SPREAD)))
    q.sort(asc(q.c.id))
    days = [row.computed(0) for row in client.query(q)]
    assert all(1 <= day <= 31 for day in days), days
    assert days == [_utc(s).day for s in _instants()]


def test_round_returns_an_integer_and_goes_away_from_zero(client: Client) -> None:
    """`round` on a value that is genuinely fractional.

    Dividing by a float literal is what makes it one: two integers divide to an
    integer in the kernel, so `size / i64(4)` would already be rounded and this
    would assert nothing.
    """
    q = Query(DOCS)
    q.compute(round_(q.c.size / lit(4.0)))
    q.sort(asc(q.c.id))
    got = [row.computed(0) for row in client.query(q)]
    # 1.25 -> 1, 3.75 -> 4, 6.25 -> 6, 8.75 -> 9, 11.25 -> 11, 13.75 -> 14,
    # 16.25 -> 16. Halves are not involved here; the point is that a float
    # comes back as an integer at all, so it can be a group key.
    assert got == [1, 4, 6, 9, 11, 14, 16]
    assert all(isinstance(v, int) for v in got), got


def test_a_calendar_part_can_be_a_group_key(client: Client) -> None:
    """Which is the whole reason to have it: `GROUP BY year(t)`.

    A computed group key is the case that made `CalendarPart` worth adding —
    ClickHouse's own taxi queries all key on `toYear(pickup_datetime)`.
    """
    a = AggregateQuery(DOCS)
    a.compute(calendar_part(CalendarPart.YEAR, a.c.size * i64(SPREAD)))
    a.group_by(a.computed(0))
    a.aggregate(Agg.count())
    groups = list(client.aggregate(a))
    assert sorted(g.key[0] for g in groups) == sorted({_utc(s).year for s in _instants()})
    assert sum(g.aggregate(0) for g in groups) == len(SIZES)


def test_a_fixed_offset_is_addition_and_needs_no_new_builder(client: Client) -> None:
    """There is no timezone here, and the composition is the answer.

    A fixed-offset conversion is adding seconds before reading the calendar
    out, which `+` already expresses — so this is a test that the documented
    workaround genuinely works, not a feature. Region names would need a
    timezone database and are not offered by any layer.
    """
    q = Query(DOCS)
    base = q.c.size * i64(SPREAD)
    q.compute(day_of_month(base), day_of_month(base + i64(12 * 3600)))
    q.sort(asc(q.c.id))
    rows = list(client.query(q))

    for row, seconds in zip(rows, _instants(), strict=True):
        assert row.computed(0) == _utc(seconds).day
        assert row.computed(1) == _utc(seconds + 12 * 3600).day
    # And they are not all the same day, or this would pass with the offset
    # dropped on the floor.
    assert any(row.computed(0) != row.computed(1) for row in rows)


def test_the_literal_guard_still_applies_inside_a_calendar_part(client: Client) -> None:
    """A bare `int` is refused, as everywhere else in this client.

    Worth pinning here because a calendar part's argument looks like a place
    where "it is obviously a timestamp" might have earned an exception. It has
    not: the wire has two integer widths and the kernel orders values by type
    before magnitude.
    """
    import pytest

    from slate.values import ValueTypeError

    with pytest.raises(ValueTypeError):
        year(lit(1_700_000_000))


def test_calendar_truncation_agrees_with_pythons_own_calendar(client: Client) -> None:
    """`month_start` and `year_start`, against `datetime.replace`.

    `date_trunc` to a fixed unit is a division. A month has no fixed length —
    that is why `CalendarUnit` is separate from `TimeUnit` — so truncating to
    one decodes the date, drops the day and encodes it again. The encoding half
    is `days_from_civil`, transcribed arithmetic, and the round trip through
    `civil_from_days` would agree with itself if both were wrong by the same
    day. Python's `datetime` knows the Gregorian calendar independently.

    The fixture spans 1977 to 2072 and crosses 2000, the century that *is* a
    leap year under the divisible-by-400 rule and is not under the
    divisible-by-100 one.
    """
    q = AggregateQuery(DOCS)
    q.compute(month_start(q.c.size * i64(SPREAD)), year_start(q.c.size * i64(SPREAD)))
    q.group_by(q.computed(0), q.computed(1))
    q.aggregate(Agg.count())

    got = {(int(g.key[0]), int(g.key[1])) for g in client.aggregate(q)}
    want = set()
    for seconds in _instants():
        when = _utc(seconds)
        want.add(
            (
                int(
                    when.replace(
                        day=1, hour=0, minute=0, second=0, microsecond=0
                    ).timestamp()
                ),
                int(
                    when.replace(
                        month=1, day=1, hour=0, minute=0, second=0, microsecond=0
                    ).timestamp()
                ),
            )
        )
    assert got == want


def test_truncation_floors_rather_than_rounding(client: Client) -> None:
    """A month boundary is at or before the instant, never after it.

    The property that distinguishes a floor from a truncation toward zero, and
    the one that matters below the epoch: rounding toward zero would move a
    December 1969 instant *forward* into 1970 and put it in the wrong year.

    The fixture is all positive, so this checks the property rather than the
    epoch case — `crates/slate-kernel/tests/calendar.rs` has the negative
    instants, where it can use a literal rather than a column.
    """
    q = AggregateQuery(DOCS)
    q.compute(q.c.size * i64(SPREAD), month_start(q.c.size * i64(SPREAD)))
    q.group_by(q.computed(0), q.computed(1))
    q.aggregate(Agg.count())
    for group in client.aggregate(q):
        instant, boundary = int(group.key[0]), int(group.key[1])
        assert boundary <= instant, f"{boundary} is after {instant}"
        # And within 31 days of it, which says it is *this* month's boundary
        # rather than some earlier one.
        assert instant - boundary < 31 * 86_400
