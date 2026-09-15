"""Decode the workbench's packed trip file, in Python.

The independent half of the oracle. `crates/slate-wasm/src/taxi.rs` decodes the
same bytes in Rust and is what the browser runs; this is a second reading of the
format from its own description, so a check that folds with this and queries
through that is comparing two implementations rather than one with itself.

The format is 24 bytes per trip, little-endian, no header and no checksum — the
file is served from the same origin as the code that reads it, and a truncated
transfer shows up as a length that does not divide. See `site/data/make-trips.py`,
which wrote it.
"""

from __future__ import annotations

import gzip
import pathlib
import struct
from typing import NamedTuple

#: Bytes per packed trip.
RECORD = 24

#: `2024-01-01 00:00:00`, the instant every packed pickup counts from.
#:
#: The sample was built by counting from *local* midnight and this is the epoch
#: second of *UTC* midnight, so the two conversions cancel and the decoded
#: values read as New York wall clock. That is why the quietest hour is 04:00
#: rather than 09:00, and it is worth stating because an earlier note asserted
#: the opposite and had to be withdrawn.
BASE = 1_704_067_200

#: `255` in the packed byte means the source row had no passenger count.
NO_PASSENGERS = 255

PAYMENTS = ("unknown", "credit card", "cash", "no charge", "dispute", "voided")


class Trip(NamedTuple):
    id: int
    pickup_zone: int
    dropoff_zone: int
    pickup_time: int
    duration: int
    passengers: int | None
    distance: float
    fare: float
    tip: float
    total: float
    payment: str


def trips(path: pathlib.Path) -> list[Trip]:
    """Every trip in the packed file."""
    raw = gzip.decompress(path.read_bytes())
    if len(raw) % RECORD:
        raise ValueError(
            f"the trip file is {len(raw)} bytes, which is not a whole number "
            f"of {RECORD}-byte records"
        )
    out: list[Trip] = []
    for index in range(len(raw) // RECORD):
        record = raw[index * RECORD : (index + 1) * RECORD]
        (offset,) = struct.unpack_from("<i", record, 0)
        (duration,) = struct.unpack_from("<H", record, 4)
        (pickup,) = struct.unpack_from("<H", record, 6)
        (dropoff,) = struct.unpack_from("<H", record, 8)
        passengers = record[10]
        (distance,) = struct.unpack_from("<H", record, 11)
        (fare,) = struct.unpack_from("<i", record, 13)
        (tip,) = struct.unpack_from("<H", record, 17)
        (total,) = struct.unpack_from("<i", record, 19)
        payment = record[23]
        out.append(
            Trip(
                id=index + 1,
                pickup_zone=pickup,
                dropoff_zone=dropoff,
                pickup_time=BASE + offset,
                duration=duration,
                passengers=None if passengers == NO_PASSENGERS else passengers,
                distance=distance / 100.0,
                fare=fare / 100.0,
                tip=tip / 100.0,
                total=total / 100.0,
                payment=PAYMENTS[payment] if payment < len(PAYMENTS) else "unknown",
            )
        )
    return out


def zones(source: pathlib.Path) -> list[tuple[int, str, str, str]]:
    """The TLC zone table, from the same CSV the browser compiles in.

    Read rather than duplicated: there is exactly one copy of these 265 rows in
    the repository and a second would drift. Python's `csv` handles the quoting
    — `"Governor's Island/Ellis Island/Liberty Island"` is one field — where the
    Rust side hand-rolls it to avoid a dependency for one fixed file. Two
    readings of one file is the point; two copies of the data would not be.

    A wrong parse shows up as a count that is not 265 rather than as a wrong
    answer.
    """
    import csv

    with source.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.reader(handle))
    out: list[tuple[int, str, str, str]] = []
    for row in rows[1:]:
        if len(row) < 4:
            continue
        try:
            identifier = int(row[0].strip())
        except ValueError:
            continue
        out.append(
            (identifier, row[1].strip(), row[2].strip(), row[3].strip())
        )
    if len(out) != 265:
        raise ValueError(f"expected 265 zones in {source}, parsed {len(out)}")
    return out


def data_dir() -> pathlib.Path:
    return pathlib.Path(__file__).resolve().parent.parent.parent / "site" / "data"


def zone_csv() -> pathlib.Path:
    return (
        pathlib.Path(__file__).resolve().parent.parent.parent
        / "crates"
        / "slate-wasm"
        / "src"
        / "taxi_zones.csv"
    )


def load_all() -> tuple[list[Trip], list[tuple[int, str, str, str]]]:
    return trips(data_dir() / "trips.bin.gz"), zones(zone_csv())


def _main() -> int:
    t, z = load_all()
    print(f"{len(t)} trips, {len(z)} zones")
    print(t[0])
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
