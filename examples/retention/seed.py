#!/usr/bin/env python3
"""Put a retired row in front of the sweep, and check afterwards that it went.

Two modes rather than two scripts, because the "before" and the "after" are one
claim and splitting them across files is how the second one drifts from the
first.
"""

from __future__ import annotations

import argparse
import sys
import time

from schema import NOTES, Notes
from slate import Client, Identity, Query
from slate.values import u64

#: The retired row, and a live one beside it.
#:
#: The live row is the control. Without it "the table is empty" would pass for
#: a sweep that erased everything, which is the failure mode a retention job
#: has and a demonstration should not be able to hide.
RETIRED = 1
LIVE = 2


def client(address: str, role: str = "app") -> Client:
    return Client(address, Identity("u64:1", roles=[role]))


def seed(address: str) -> None:
    session = client(address).session()
    session.insert(
        NOTES,
        [
            # Through the *generated* encoder, which is how an application
            # writes a row: named fields rather than a positional list.
            Notes(id=RETIRED, body="retired", deleted_at=None).to_row(),
            Notes(id=LIVE, body="live", deleted_at=None).to_row(),
        ],
        upsert=True,
    )
    # Retire one of them the way an application would: an ordinary delete on a
    # soft-deleting table stamps rather than erases.
    session.delete(NOTES, [(u64(RETIRED),)])
    print(f"seeded: {RETIRED} retired, {LIVE} live")


def verify(address: str) -> None:
    # As `retention`, not `app`: seeing retired rows needs `read_deleted`, and
    # the application deliberately does not hold it.
    session = client(address, "retention").session()
    # `include_deleted()` is a *method*. The first version of this assigned
    # `query.include_deleted = True`, which replaced the bound method with a
    # boolean, asked an ordinary question, and got an ordinary answer — so the
    # check passed whether or not the sweep had done anything. Found by running
    # the harness with a window that erases nothing and watching it still
    # report success.
    query = Query(NOTES).include_deleted()
    rows = [Notes.from_row(list(row.values)) for row in session.query(query)]
    ids = sorted(row.id for row in rows)
    if ids != [LIVE]:
        raise SystemExit(
            f"after the sweep the table holds {ids}, expected [{LIVE}] — "
            "the retired row survived, or the live one did not"
        )
    print(f"verified: {RETIRED} is gone, {LIVE} is untouched")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args(argv)
    # A soft delete stamps the clock, and the sweep asks for rows retired
    # before `now`. One second of slack, because the two calls can land inside
    # the same second and `before` is exclusive.
    if args.verify:
        verify(args.address)
    else:
        seed(args.address)
        time.sleep(1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
