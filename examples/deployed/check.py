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
import json
import math
import pathlib
import sys
from collections import defaultdict

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import taxi
from load import TRIPS, ZONES
from narrow import as_float, as_int, as_str
from slate import (
    Agg,
    AggregateQuery,
    Client,
    Freshness,
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
from slate.scalar import CalendarPart, TimeUnit

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
        "--expect",
        type=pathlib.Path,
        help="write the answers here, for the Go and TypeScript checks to "
        "assert against. The fold is in Python; this is how the other two "
        "SDKs are held to it without a second decoder in each language.",
    )
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

    # --- the rows arrived, all of them, and the run pins to that point ----
    #
    # `Freshness.latest()`, and it is load-bearing rather than tidy. This
    # deployment declares two replicas that poll on an interval, so a count
    # with no freshness demand may be served by one that has not yet seen the
    # loader's last chunk — and then this check reports data loss that did not
    # happen. It did exactly that in CI on 2026-09-16: 18,000 against 20,000,
    # one 2,000-row chunk short, while the freshness-demanding count further
    # down passed in the same run and saw all 20,000.
    #
    # The name of this check is a durability claim, and a stale read cannot
    # disprove durability. Asking for the latest snapshot is the question the
    # name was always making.
    #
    # Fixing *this* read left every other one correct-by-luck in the same way:
    # nothing else demanded freshness, and each compares against a fold of the
    # whole file, so any of them would have reported the same phantom data loss
    # on a slow poll. So the sequence that read was served at becomes the pin
    # for the rest of the run: `Freshness.at_least(loaded)` on every read.
    #
    # `at_least` rather than `latest`, deliberately. `latest` is the writer and
    # only the writer, so pinning with it would send every read in this file to
    # the writer and quietly gut the replica checks below — they would still
    # pass, by never asking a replica anything. `at_least` is satisfied by any
    # view that has polled past the load, which is the actual requirement, and
    # `[routing] catch_up` is the budget the pool waits for one to get there.
    counted = client.aggregate(
        AggregateQuery(TRIPS).aggregate(Agg.count()), freshness=Freshness.latest()
    )
    total = next((as_int(group.aggregates[0]) for group in counted), 0)
    check(
        "every trip survived the wire, the WAL and the bucket",
        total == len(trips),
        f"{total} against {len(trips)}",
    )

    loaded = counted.served_by.sequence if counted.served_by is not None else 0
    # Asserted rather than assumed, because a pin of zero is a pin of nothing:
    # `at_least(0)` is satisfied by every view including one that has read
    # none of the load, so the rest of this file would silently go back to
    # being correct-by-luck while every line still printed `ok`. That is the
    # repository's own "a skip is green" failure, one level down.
    check(
        "the writer named the sequence the load reached, so the run can pin to it",
        loaded > 0,
        f"served_by {counted.served_by!r}",
    )
    pinned = Freshness.at_least(loaded)
    print(f"       pinned every read below to sequence {loaded}")

    # --- a computed column, on one table ----------------------------------
    #
    # The workbench's first time-function example, through gRPC instead.
    hours = AggregateQuery(TRIPS)
    hours.compute(extract(TimeUnit.HOUR, hours.c.pickup_time))
    hours.group_by(hours.computed(0))
    hours.aggregate(Agg.count())
    got = {key: count for key, count in groups(client, hours, pinned)}

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
    by_day = {key: count for key, count in groups(client, days, pinned)}
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
    for group in client.aggregate(grouped, freshness=pinned):
        key = group.key[0]
        count, average = group.aggregates
        joined[as_int(key)] = (as_int(count), as_float(average))

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

    # Annotated, and that is load-bearing rather than decoration: without the
    # annotation a checker infers whatever the comprehension produces, the
    # narrowing helpers become optional, and dropping them changes nothing it
    # can see. Mutating them away survived until this line existed.
    got_borough: dict[str, int] = {
        as_str(group.key[0]): as_int(group.aggregates[0])
        for group in client.aggregate(by_borough, freshness=pinned)
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
    plan = client.explain(covering, freshness=pinned)
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
    in_zone_count = one_group(client, in_zone_query, pinned)
    check(
        "the indexed count is the fold's count",
        in_zone_count == in_zone,
        f"{in_zone_count} against {in_zone}",
    )

    # --- a fixed timezone offset is arithmetic ----------------------------
    shifted = AggregateQuery(TRIPS)
    shifted.compute(
        extract(TimeUnit.HOUR, as_scalar(shifted.c.pickup_time) + i64(-5 * 3600))
    )
    shifted.group_by(shifted.computed(0))
    shifted.aggregate(Agg.count())
    got_shift = {key: count for key, count in groups(client, shifted, pinned)}
    want_shift: dict[int, int] = defaultdict(int)
    for trip in trips:
        want_shift[(trip.pickup_time - 5 * 3600) // 3600 % 24] += 1
    check(
        "a five-hour offset rotates the hours and keeps every trip",
        got_shift == dict(want_shift) and sum(got_shift.values()) == len(trips),
        f"{sum(got_shift.values())} trips",
    )

    # --- a read replica served something ----------------------------------
    #
    # Two `[[replicas]]` are declared, so a read may be served by the writer or
    # by either of them, and the answer must be the same whichever. What is
    # asserted is that the *name* comes back and is one this config declares —
    # a pool that silently fell back to the writer for every read would look
    # identical from the answers alone.
    replica_names = {"writer", "reader-a", "reader-b"}
    # Several reads, not one: the pool is round-robin over the replicas, so a
    # single read proves only that *something* served it. Asserting that at
    # least one `reader-` name appears is what distinguishes two configured
    # replicas from two declared and never used — which is what a pool that
    # quietly fell back to the writer on every read would look like, and is
    # indistinguishable from the answers alone.
    #
    # Pinned like every other read, and the pin is what makes the third check
    # below ("the count is the one the writer has") mean anything: unpinned, a
    # replica one poll behind answers with a short count and that check reports
    # a disagreement between views that is really a disagreement in time. It is
    # also the strongest evidence here that `[routing] catch_up` works — these
    # eight reads demand a sequence *and* land on a replica.
    seen: set[str] = set()
    counts: set[int] = set()
    for _ in range(8):
        stream = client.aggregate(
            AggregateQuery(TRIPS).aggregate(Agg.count()), freshness=pinned
        )
        counts.update(as_int(group.aggregates[0]) for group in stream)
        if stream.served_by is not None:
            seen.add(stream.served_by.replica)
    check(
        "every read names the view that served it, and they are this config's",
        bool(seen) and seen <= replica_names,
        f"{sorted(seen)}",
    )
    check(
        "and a read replica served at least one of them",
        any(name.startswith("reader-") for name in seen),
        f"{sorted(seen)}",
    )
    check(
        "whichever view answered, the count is the one the writer has",
        counts == {len(trips)},
        f"{sorted(counts)} against [{len(trips)}]",
    )

    # A read that *names* a sequence can only be served by a view that has
    # reached it, which for a following replica means it has polled past it.
    # This is the check that would fail if `catch_up` were shorter than the
    # poll interval — the routing note in `head.toml` is about exactly that.
    fresh = client.aggregate(
        AggregateQuery(TRIPS).aggregate(Agg.count()), freshness=Freshness.latest()
    )
    latest = [as_int(group.aggregates[0]) for group in fresh]
    check(
        "a read demanding the latest snapshot is served, and is right",
        latest == [len(trips)],
        f"{latest} against [{len(trips)}], served by {fresh.served_by!r}",
    )

    if args.expect is not None:
        # The questions the Go and TypeScript checks re-ask, with the answers
        # this fold produced. Written rather than recomputed in each language
        # because the *fold* is the oracle and there should be one of it: three
        # decoders of the same packed file would be three places to be wrong,
        # and a common-mode error in them would agree with itself.
        args.expect.write_text(
            json.dumps(
                {
                    "trips": len(trips),
                    "byHour": {str(h): n for h, n in sorted(want.items())},
                    "byBorough": dict(sorted(want_borough.items())),
                    "joinedHours": {
                        str(h): [len(f), sum(f) / len(f)]
                        for h, f in sorted(want_joined.items())
                    },
                    "zone132": in_zone,
                    "replicas": sorted(replica_names),
                    # The sequence this run pinned to, so the Go and
                    # TypeScript arms can pin to the same one. They run after
                    # this file and against the same bucket, so every read they
                    # make is correct-by-luck in exactly the way the reads
                    # above were until this change — more so, because by then
                    # the replicas have had another few seconds to catch up and
                    # the luck holds more often.
                    #
                    # Neither client takes a per-read `freshness` the way
                    # Python's does; what they have is `Session.Observe` /
                    # `session.observe`, documented for carrying a position
                    # between processes, which is exactly this. That asymmetry
                    # between the three SDKs is real and is recorded in the
                    # example's README rather than worked around here.
                    "pinnedSequence": loaded,
                },
                indent=1,
            )
            + "\n"
        )
        print(f"wrote  the expected answers to {args.expect}")

    client.close()

    print()
    if failures:
        print(f"{len(failures)} failed: {', '.join(failures)}")
        return 1
    print("the deployed head node agrees with a fold done in Python")
    return 0


def groups(
    client: Client, query: AggregateQuery, freshness: Freshness | None = None
) -> list[tuple[int, int]]:
    return [
        (as_int(group.key[0]), as_int(group.aggregates[0]))
        for group in client.aggregate(query, freshness=freshness)
    ]


def one_group(
    client: Client, query: AggregateQuery, freshness: Freshness | None = None
) -> int:
    for group in client.aggregate(query, freshness=freshness):
        return as_int(group.aggregates[0])
    return 0


if __name__ == "__main__":
    sys.exit(main())
