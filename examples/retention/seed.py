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
from slate import Client, Identity, NotFound, Query
from slate.values import u64

#: The retired row, and a live one beside it.
#:
#: The live row is the control. Without it "the table is empty" would pass for
#: a sweep that erased everything, which is the failure mode a retention job
#: has and a demonstration should not be able to hide.
RETIRED = 1
LIVE = 2
#: Retired like `RETIRED`, and pulled back out of the window before the sweep.
#:
#: It is load-bearing rather than decorative: if the restore silently did
#: nothing this row would still be retired when the sweep ran, the sweep would
#: erase it, and `verify` would fail. A demonstration that cannot fail
#: demonstrates nothing, which this harness has been caught at once already.
UNDONE = 3


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
            Notes(id=UNDONE, body="undone", deleted_at=None).to_row(),
        ],
        upsert=True,
    )
    # Retire two of them the way an application would: an ordinary delete on a
    # soft-deleting table stamps rather than erases.
    session.delete(NOTES, [(u64(RETIRED),), (u64(UNDONE),)])
    print(f"seeded: {RETIRED} and {UNDONE} retired, {LIVE} live")


def undo(address: str) -> None:
    """Pull one retired row back out of the window, before the sweep reaches it.

    This is what a retention period is *for* — the rows are kept so that a
    delete can be taken back — and until recently it did not work: a write at a
    retired row's key was refused as missing at every privilege, so the only
    thing that could ever happen to a retired row was being erased. See
    `ledger/2026-09-20-the-write-that-names-a-key.md`.
    """
    session = client(address, "undo").session()
    query = Query(NOTES).include_deleted()
    rows = [Notes.from_row(list(row.values)) for row in session.query(query)]
    row = next(row for row in rows if row.id == UNDONE)
    if not row.retired:
        raise SystemExit(f"{UNDONE} was supposed to be retired and is not")
    # Through the *generated* helper, which is the whole reason it is
    # generated: the catalog knows which column carries the stamp, so a caller
    # should not have to. `restored()` clears it and changes nothing else;
    # sending it is an ordinary update, because there is no restore verb.
    session.update(NOTES, [row.restored().to_row()])
    print(f"undone: {UNDONE} is live again")

    # And the half that says the grant is doing something. The application
    # holds `update` and not `read_deleted`, so the row it deleted is not its
    # to bring back — it reads as missing, exactly as a row hidden by a policy
    # does. Without this the harness would pass on a server that let anybody
    # write any retired row.
    app = client(address).session()
    try:
        app.update(NOTES, [Notes(id=RETIRED, body="mine again", deleted_at=None).to_row()])
    except NotFound:
        print(f"refused: the application cannot undo {RETIRED} without read_deleted")
    else:
        raise SystemExit(
            f"the application restored {RETIRED} while holding no read_deleted; "
            "the grant that gates an undo is not being enforced"
        )


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
    expected = [LIVE, UNDONE]
    if ids != expected:
        raise SystemExit(
            f"after the sweep the table holds {ids}, expected {expected} — "
            f"the retired row survived, the live one did not, or {UNDONE} was "
            "never really restored and the sweep took it"
        )
    print(f"verified: {RETIRED} is gone, {LIVE} and {UNDONE} are untouched")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True)
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--undo", action="store_true")
    args = parser.parse_args(argv)
    # A soft delete stamps the clock, and the sweep asks for rows retired
    # before `now`. One second of slack, because the two calls can land inside
    # the same second and `before` is exclusive.
    if args.verify:
        verify(args.address)
    elif args.undo:
        undo(args.address)
    else:
        seed(args.address)
        time.sleep(1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
