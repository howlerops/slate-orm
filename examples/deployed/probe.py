#!/usr/bin/env python3
"""One write, acknowledged, and then read back after the writer was killed.

`check.py` proves the *dataset* survives a restart, and it cannot prove
anything about `[storage] durability`: the kill happens seconds after the last
write, by which time `visible` has flushed anyway. Setting `durability =
"visible"` and running the restart was tried, and every check still passed.

This narrows the window as far as a shell can: `--write` inserts one row and
returns as soon as the server says it is committed, `run.sh` kills the process
on the next line, and `--verify` asks the replacement for it. What that proves
is the useful thing — an acknowledged write is in the bucket, not in the dead
process's memory, and a replacement finds it.

**It does not distinguish the two durability settings, and a hypothesis that
it would is withdrawn.** `durability = "visible"` documents itself as losable
"if the writer fails before its next flush", so setting it and killing
immediately after an acknowledgement ought to lose the row. It was tried three
times and the probe survived all three. Whether SlateDB flushes before
returning from this path, or whether three attempts simply never landed inside
the window, is not something this example can tell — so the claim is not made.
`slate-slatedb`'s own suites are where that distinction belongs, with a fault
injector rather than a `kill -9` and a stopwatch.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from slate import Client, Identity, NotFound, Query, i64, u64  # noqa: E402

from load import TRIPS  # noqa: E402

#: Outside the sample's ids, which run from 1, so the probe cannot collide
#: with a loaded row and cannot be mistaken for one in a count.
PROBE_ID = 9_000_001


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()

    client = Client(
        args.address, Identity(principal="u64:1", tenant="u64:1", roles=["app"])
    )
    client.wait_for_ready(timeout=30.0)
    session = client.session()

    if args.write:
        session.insert(
            TRIPS,
            [[
                u64(PROBE_ID), u64(132), u64(132), i64(1_704_067_200), i64(600),
                i64(1), 1.5, 9.5, 1.0, 10.5, "probe",
            ]],
        )
        print(f"wrote  the durability probe, id {PROBE_ID}, acknowledged")
        client.close()
        return 0

    if args.verify:
        query = Query(TRIPS)
        query.where(query.c.id.eq(u64(PROBE_ID)))
        rows = list(session.query(query))
        ok = len(rows) == 1 and int(rows[0].get("id")) == PROBE_ID
        print(
            f"{'ok   ' if ok else 'FAIL '} an acknowledged write survived "
            f"`kill -9` and is in the bucket"
            + ("" if ok else f"   found {len(rows)} rows")
        )
        # And it is gone again, so a second run of `run.sh` against a fresh
        # bucket and this one against a kept one behave the same.
        if ok:
            try:
                session.delete(TRIPS, [[u64(PROBE_ID)]])
            except NotFound:
                pass
        client.close()
        return 0 if ok else 1

    parser.error("one of --write or --verify")
    return 2


if __name__ == "__main__":
    sys.exit(main())
