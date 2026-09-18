#!/usr/bin/env python3
"""Build the workbench's trip sample from the real TLC parquet.

Run this only to regenerate `site/data/trips.bin.gz`, which *is* committed.

That is a deliberate exception to this repository's rule that build outputs
stay out of git. The wasm is rebuilt every deploy because it must not drift
from the kernel; this file cannot drift from anything — it is a fixed sample of
a published dataset that has not changed since March 2024. Rebuilding it in CI
would mean a 50 MB download on every run, a pyarrow dependency, and a sampling
step that has to be bit-for-bit deterministic or the committed file changes
under you. Committing 1 MB is the smaller cost.

    python3 site/data/make-trips.py            # needs pyarrow and the parquet

Source: NYC TLC yellow taxi, January 2024.
https://d37ci6vzurychx.cloudfront.net/trip-data/yellow_tripdata_2024-01.parquet

Encoding: fixed 24-byte little-endian records, sorted by pickup time.

    i32  pickup      seconds from 2024-01-01 00:00:00 local
    u16  duration    seconds, clamped to 65535
    u16  pu_zone     TLC LocationID
    u16  do_zone     TLC LocationID
    u8   passengers  255 means the source had no value
    u16  distance    hundredths of a mile, clamped
    i32  fare        cents
    u16  tip         cents, clamped
    i32  total       cents
    u8   payment     TLC payment_type

Nulls are preserved rather than filled in. `passenger_count` is genuinely
missing on some real trips, and that is the only thing in this dataset that
makes `count(*)` and `count(passengers)` different numbers — which is the
distinction the aggregate tests could not otherwise draw.
"""

import datetime
import gzip
import pathlib
import random
import struct
import sys

#: How many trips the browser gets. The whole month is 2,964,619 and loads in
#: 17.8 s in-process, but the in-memory store costs about 1.3 KB a row, so the
#: month would want ~3.8 GB and a wasm32 tab has 4 GB of address space in
#: theory and around 2 GB in practice. 100,000 is ~130 MB, which a tab holds
#: without complaint, and ~1.1 MB gzipped on the wire.
SAMPLE = 100_000
#: Fixed, so the committed file is reproducible from this script.
SEED = 20240101

RECORD = struct.Struct("<iHHHBHiHiB")
ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "site" / "data" / "trips.bin.gz"


def main(parquet: str) -> int:
    # Function-local, and this file is excluded from `ty` because of it: see
    # the root `pyproject.toml`. `pyarrow` is deliberately not a dependency of
    # anything CI installs — the docstring above is where that was decided —
    # so a type checker cannot resolve this import there. It resolves fine on a
    # machine that has pyarrow, which is what makes it the worst kind of
    # difference: green locally, red in CI, for a reason neither run mentions.
    import pyarrow.parquet as pq

    columns = [
        "tpep_pickup_datetime", "tpep_dropoff_datetime", "passenger_count",
        "trip_distance", "PULocationID", "DOLocationID", "payment_type",
        "fare_amount", "tip_amount", "total_amount",
    ]
    table = pq.read_table(parquet, columns=columns)
    base = int(datetime.datetime(2024, 1, 1).timestamp())
    col = {name: table[name].to_pylist() for name in columns}

    # Sample from the whole month rather than taking a prefix: the first
    # 100,000 rows of the file are the first few hours of 1 January, which is a
    # public holiday. Any query about time of day would be answered off New
    # Year's Day and look like a bug in the data.
    random.seed(SEED)
    candidates = random.sample(range(table.num_rows), min(SAMPLE * 3, table.num_rows))

    rows = []
    for i in candidates:
        pickup, dropoff = col["tpep_pickup_datetime"][i], col["tpep_dropoff_datetime"][i]
        if pickup is None or dropoff is None:
            continue
        seconds = int(pickup.timestamp()) - base
        # The file contains a handful of trips stamped in 2002 and 2009. They
        # are real rows of the real dataset and they are also junk; a reader
        # grouping by day would get a hundred empty days and one outlier.
        if not 0 <= seconds < 31 * 86400:
            continue
        passengers = col["passenger_count"][i]
        rows.append((
            seconds,
            max(0, min(65535, int((dropoff - pickup).total_seconds()))),
            int(col["PULocationID"][i] or 0),
            int(col["DOLocationID"][i] or 0),
            255 if passengers is None else max(0, min(9, int(passengers))),
            max(0, min(65535, int((col["trip_distance"][i] or 0) * 100))),
            max(-2**31, min(2**31 - 1, int((col["fare_amount"][i] or 0) * 100))),
            max(0, min(65535, int((col["tip_amount"][i] or 0) * 100))),
            max(-2**31, min(2**31 - 1, int((col["total_amount"][i] or 0) * 100))),
            max(0, min(5, int(col["payment_type"][i] or 0))),
        ))

    # Take the sample from the *surviving* rows, then sort. Cutting the loop
    # short at SAMPLE instead looks equivalent and is not: the candidate
    # indices were sorted, so stopping early took a prefix of the file — the
    # first hours of New Year's Day — which is the exact failure the sampling
    # above exists to avoid. It showed up as zero rows with a missing
    # passenger count, in a month where 4.73% of rows have one.
    if len(rows) < SAMPLE:
        raise SystemExit(f"only {len(rows)} usable rows; raise the candidate multiplier")
    rows = random.sample(rows, SAMPLE)
    rows.sort()
    packed = b"".join(RECORD.pack(*row) for row in rows)
    OUT.write_bytes(gzip.compress(packed, 9))
    nulls = sum(1 for r in rows if r[4] == 255)
    print(f"{len(rows)} trips, {nulls} with no passenger count")
    print(f"{len(packed) / 1e6:.2f} MB raw, {OUT.stat().st_size / 1e6:.2f} MB gzipped -> {OUT}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} <yellow_tripdata_2024-01.parquet>")
    raise SystemExit(main(sys.argv[1]))
