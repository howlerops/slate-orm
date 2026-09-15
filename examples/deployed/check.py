#!/usr/bin/env python3
"""Ask a deployed head node the workbench's questions, and check the answers.

The oracle is `taxi.py`: the same 100,000 trips decoded in Python and folded in
Python, with nothing in it going through the kernel, its planner, its grouper or
its scalars. The database's answers come back over gRPC from SlateDB on an S3
bucket. Any disagreement is a real defect in a path the browser never
exercises — the wire, the WAL, the SSTs, the object store.

That is the whole argument for this example. Everything the last three changes
built was tested in-process or in a tab; a computed column that works in a
browser and not through a socket is a computed column that does not work.
"""

from __future__ import annotations

import argparse
import math
import pathlib
import sys
from collections import defaultdict

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import taxi  # noqa: E402

from slate import (  # noqa: E402
    Agg,
    AggregateQuery,
    Client,
    GroupedJoinQuery,
    Identity,
    JoinQuery,
    Query,
    as_scalar,
    calendar_part,
    extract,
    i64,
    u64,
)
from slate.scalar import CalendarPart, TimeUnit  # noqa: E402

from load import TRIPS, ZONES  # noqa: E402

failures: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"{'ok   ' if ok else 'FAIL '} {name}" + (f"   {detail}" if detail and not ok else ""))
    if not ok:
        failures.append(name)


def close(got: float, want: float, tolerance: float = 1e-6) -> bool:
    return math.isclose(got, want, rel_tol=0.0, abs_tol=tolerance)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument(
        "--trips",
        type=int,
        default=0,
        help="the prefix `load.py` was given, so the fold matches what was loaded",
    )
    args = parser.parse_args()

    trips, zones = taxi.load_all()
    if args.trips:
        trips = trips[: args.trips]
    zone_ids = {z[0] for z in zones}
    borough_of = {z[0]: z[1] for z in zones}

    client = Client(
        args.address, Identity(principal="u64:1", tenant="u64:1", roles=["app"])
    )
    client.wait_for_ready(timeout=30.0)

    # --- the rows arrived, all of them ------------------------------------
    total = one_group(client, AggregateQuery(TRIPS).aggregate(Agg.count()))
    check(
        "every trip survived the wire, the WAL and the bucket",
        total == len(trips),
        f"{total} against {len(trips)}",
    )

    # --- a computed column, on one table ----------------------------------
    #
    # The workbench's first time-function example, through gRPC instead.
    hours = AggregateQuery(TRIPS)
    hours.compute(extract(TimeUnit.HOUR, hours.c.pickup_time))
    hours.group_by(hours.computed(0))
    hours.aggregate(Agg.count())
    got = {key: count for key, count in groups(client, hours)}

    want: dict[int, int] = defaultdict(int)
    for trip in trips:
        want[trip.pickup_time // 3600 % 24] += 1
    check(
        "trips per hour of day agree with a fold done in Python",
        got == dict(want),
        f"{sorted(got.items())[:4]} against {sorted(want.items())[:4]}",
    )
    # The one *absolute* assertion here, and the only one that can catch a
    # common-mode error: `load.py` and this fold share `taxi.py`, so a bug in
    # the decoder moves both sides and every differential check still agrees.
    # Shifting the decoder's epoch by an hour was tried, and this was the only
    # check that noticed.
    #
    # It is a claim about the *whole* January sample, so it is skipped on a
    # prefix: the first 20,000 trips are the first few days, whose busiest hour
    # is legitimately not the month's.
    if args.trips:
        print("skip  the wall-clock shape needs the whole sample, not a prefix")
    else:
        trough = min(want, key=lambda h: want[h])
        peak = max(want, key=lambda h: want[h])
        check(
            "and the sample reads as New York wall clock, trough at 04:00",
            trough == 4 and peak == 18,
            f"trough {trough}, peak {peak}",
        )

    # --- a calendar field, which is not a fixed number of seconds ---------
    days = AggregateQuery(TRIPS)
    days.compute(calendar_part(CalendarPart.DAY_OF_WEEK, days.c.pickup_time))
    days.group_by(days.computed(0))
    days.aggregate(Agg.count())
    by_day = {key: count for key, count in groups(client, days)}
    want_day: dict[int, int] = defaultdict(int)
    for trip in trips:
        # Python's weekday() is Monday=0; the kernel's is Sunday=0, matching
        # ClickHouse and SQLite. Converted here rather than assumed.
        import datetime

        when = datetime.datetime.fromtimestamp(trip.pickup_time, datetime.UTC)
        want_day[(when.weekday() + 1) % 7] += 1
    check(
        "day of week agrees with Python's own datetime",
        by_day == dict(want_day),
        f"{sorted(by_day.items())} against {sorted(want_day.items())}",
    )

    # --- a join, and a value the join computes ----------------------------
    #
    # The query the previous entry recorded as not expressible: the group key
    # and the aggregate both read `trips`, across a join to `zones`.
    join = JoinQuery()
    trips_in = join.add(TRIPS)
    join.add(ZONES, on=[(trips_in.c.pickup_zone, "id")])
    join.compute(extract(TimeUnit.HOUR, as_scalar(trips_in.c.pickup_time)))
    grouped = GroupedJoinQuery(join)
    grouped.group_by(join.computed(0))
    grouped.aggregate(Agg.count(), Agg.avg(trips_in.c.fare))

    joined: dict[int, tuple[int, float]] = {}
    for group in client.aggregate(grouped):
        key = group.key[0]
        count, average = group.aggregates
        joined[int(key)] = (int(count), float(average))

    want_joined: dict[int, list[float]] = defaultdict(list)
    for trip in trips:
        if trip.pickup_zone in zone_ids:
            want_joined[trip.pickup_time // 3600 % 24].append(trip.fare)

    check(
        "the hour and the average fare, both from `trips`, across a join",
        set(joined) == set(want_joined)
        and all(joined[h][0] == len(want_joined[h]) for h in want_joined)
        and all(
            close(joined[h][1], sum(want_joined[h]) / len(want_joined[h]))
            for h in want_joined
        ),
        f"{sorted(joined.items())[:2]}",
    )

    # --- a group key on the right side of the join ------------------------
    boroughs = JoinQuery()
    trips_b = boroughs.add(TRIPS)
    zones_b = boroughs.add(ZONES, on=[(trips_b.c.pickup_zone, "id")])
    by_borough = GroupedJoinQuery(boroughs)
    by_borough.group_by(zones_b.c.borough)
    by_borough.aggregate(Agg.count())

    got_borough = {
        str(group.key[0]): int(group.aggregates[0])
        for group in client.aggregate(by_borough)
    }
    want_borough: dict[str, int] = defaultdict(int)
    for trip in trips:
        if trip.pickup_zone in zone_ids:
            want_borough[borough_of[trip.pickup_zone]] += 1
    check(
        "grouping by the right table's borough agrees with the fold",
        got_borough == dict(want_borough),
        f"{sorted(got_borough.items())} against {sorted(want_borough.items())}",
    )

    # --- the index is used, and answers without reading a row -------------
    covering = Query(TRIPS)
    covering.where(covering.c.pickup_zone.eq(u64(132)))
    covering.select(covering.c.pickup_zone)
    plan = client.explain(covering)
    check(
        "an index answers the covering query without touching a row",
        "by_pickup_zone" in plan.display and plan.index_only,
        f"{plan.display!r}, index_only={plan.index_only}",
    )

    # --- and the rows behind it are right ---------------------------------
    in_zone = sum(1 for t in trips if t.pickup_zone == 132)
    in_zone_query = AggregateQuery(TRIPS)
    in_zone_query.where(in_zone_query.c.pickup_zone.eq(u64(132)))
    in_zone_query.aggregate(Agg.count())
    counted = one_group(client, in_zone_query)
    check(
        "the indexed count is the fold's count",
        counted == in_zone,
        f"{counted} against {in_zone}",
    )

    # --- a fixed timezone offset is arithmetic ----------------------------
    shifted = AggregateQuery(TRIPS)
    shifted.compute(
        extract(TimeUnit.HOUR, as_scalar(shifted.c.pickup_time) + i64(-5 * 3600))
    )
    shifted.group_by(shifted.computed(0))
    shifted.aggregate(Agg.count())
    got_shift = {key: count for key, count in groups(client, shifted)}
    want_shift: dict[int, int] = defaultdict(int)
    for trip in trips:
        want_shift[(trip.pickup_time - 5 * 3600) // 3600 % 24] += 1
    check(
        "a five-hour offset rotates the hours and keeps every trip",
        got_shift == dict(want_shift) and sum(got_shift.values()) == len(trips),
        f"{sum(got_shift.values())} trips",
    )

    client.close()

    print()
    if failures:
        print(f"{len(failures)} failed: {', '.join(failures)}")
        return 1
    print("the deployed head node agrees with a fold done in Python")
    return 0


def groups(client: Client, query: AggregateQuery) -> list[tuple[int, int]]:
    return [
        (int(group.key[0]), int(group.aggregates[0]))
        for group in client.aggregate(query)
    ]


def one_group(client: Client, query: AggregateQuery) -> int:
    for group in client.aggregate(query):
        return int(group.aggregates[0])
    return 0


if __name__ == "__main__":
    sys.exit(main())
