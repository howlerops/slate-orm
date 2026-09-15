#!/usr/bin/env python3
"""Load the workbench's dataset into a deployed head node, over gRPC.

100,000 trips and 265 zones, through the Python SDK, into SlateDB on an S3
bucket. This is the part that makes the check below mean something: the browser
seeds the same rows into an in-memory store in one call, and this puts them
through a socket, a WAL, an SST and an object store first.

Batched, because the interesting number is how long it takes rather than how
many round trips it can be made into: one row per request would be 100,000
round trips and would say nothing about the database.
"""

from __future__ import annotations

import argparse
import sys
import time

import taxi

from slate import Client, Column, Identity, Table, i64, u64
from slate.types import ValueType


#: The tables, declared exactly as `head.toml` declares them.
#:
#: A declaration rather than a bare name, so every request carries a schema
#: check: this client hard-codes column *order*, and a server whose `trips` has
#: gained a column would otherwise take these rows and put them in the wrong
#: places. See `SchemaCheck` in `records.proto`.
TRIPS = Table(
    "trips",
    [
        Column("id", ValueType.U64),
        Column("pickup_zone", ValueType.U64),
        Column("dropoff_zone", ValueType.U64),
        Column("pickup_time", ValueType.I64),
        Column("duration", ValueType.I64),
        Column("passengers", ValueType.I64),
        Column("distance", ValueType.F64),
        Column("fare", ValueType.F64),
        Column("tip", ValueType.F64),
        Column("total", ValueType.F64),
        Column("payment", ValueType.STR),
    ],
    primary_key=["id"],
)

ZONES = Table(
    "zones",
    [
        Column("id", ValueType.U64),
        Column("borough", ValueType.STR),
        Column("zone", ValueType.STR),
        Column("service_zone", ValueType.STR),
    ],
    primary_key=["id"],
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True, help="the head node's host:port")
    parser.add_argument(
        "--batch",
        type=int,
        default=2000,
        help="rows per Insert call (default 2000)",
    )
    parser.add_argument(
        "--trips",
        type=int,
        default=0,
        help="load only the first N trips, for a quick run (default: all)",
    )
    args = parser.parse_args()

    trips, zones = taxi.load_all()
    if args.trips:
        trips = trips[: args.trips]

    client = Client(
        args.address,
        Identity(principal="u64:1", tenant="u64:1", roles=["app"]),
    )
    client.wait_for_ready(timeout=30.0)
    session = client.session()

    started = time.monotonic()
    session.insert(ZONES, [[u64(z[0]), z[1], z[2], z[3]] for z in zones])
    print(f"  {len(zones)} zones")

    written = 0
    for start in range(0, len(trips), args.batch):
        chunk = trips[start : start + args.batch]
        session.insert(
            TRIPS,
            [
                [
                    u64(t.id),
                    u64(t.pickup_zone),
                    u64(t.dropoff_zone),
                    i64(t.pickup_time),
                    i64(t.duration),
                    None if t.passengers is None else i64(t.passengers),
                    t.distance,
                    t.fare,
                    t.tip,
                    t.total,
                    t.payment,
                ]
                for t in chunk
            ],
        )
        written += len(chunk)
        if written % 20000 == 0:
            print(f"  {written} trips", flush=True)

    elapsed = time.monotonic() - started
    print(
        f"  {written} trips in {elapsed:.1f}s "
        f"({written / elapsed:,.0f} rows/s through gRPC, SlateDB and the bucket)"
    )
    client.close()
    return 0


if __name__ == "__main__":
    sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent))
    sys.exit(main())
