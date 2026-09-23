#!/usr/bin/env python3
"""Erase soft-deleted rows on a schedule, by calling the head node.

Soft delete shipped with a `purge_deleted` RPC and nothing that runs it, which
made "a soft-deleting table grows without bound" true of every deployment that
used the convention. `ledger/2026-09-19-a-purge-something-can-call.md` recorded
it as *no scheduler, and no example of an external one*; this is the example.

**It is a script, not a feature of the server, and that is deliberate.** A
retention sweep inside the node would have to decide whose identity it runs as,
whether it runs on a follower, and what it does when it overruns its interval —
three questions whose answers belong to a deployment rather than to a database.
An external caller answers all three by existing: it holds its own credentials,
it points at whichever node it is told to, and if it is slow the next run is
simply late.

Usage:

    purge.py --address HOST:PORT --schema pkg.module --table shipments \
             --older-than 30d [--once]

`--schema` is the deployment's **generated** declaration — the module
`scripts/codegen.py` writes, exposing `BY_NAME`. A purge is an ordinary
request and carries the same schema check as any other, so a retention job
needs the same declaration the application holds; taking a bare table name
would mean this script keeping its own copy of the catalog, which is the drift
the generator exists to remove.

`--once` runs a single sweep and exits, which is what a cron entry wants and
what the harness beside this file uses. Without it the script loops on
`--every`, which is what a sidecar or a systemd timer wants.
"""

from __future__ import annotations

import argparse
import importlib
import sys
import time

from slate import Client, Identity, Table

#: Suffixes `--older-than` and `--every` accept, in seconds.
#:
#: Spelled out rather than parsed with a library: a retention window is the one
#: number in this script somebody will read in a hurry at three in the morning,
#: and `30d` is unambiguous in a way `2592000` is not.
UNITS = {"s": 1, "m": 60, "h": 3600, "d": 86400}


def duration(text: str) -> int:
    """`30d`, `12h`, `90m`, `45s` — as seconds."""
    if len(text) < 2 or text[-1] not in UNITS:
        raise argparse.ArgumentTypeError(
            f"{text!r}: expected a number and one of {''.join(sorted(UNITS))}"
        )
    try:
        count = int(text[:-1])
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"{text!r}: {error}") from error
    if count < 0:
        raise argparse.ArgumentTypeError(f"{text!r}: a window cannot be negative")
    return count * UNITS[text[-1]]


def tables_of(module_path: str, names: list[str]) -> list[Table]:
    """Resolve each name against a generated declaration module.

    Imported rather than reconstructed. `BY_NAME` is what `codegen.py` emits
    for exactly this: code that needs a table it was not written against.
    """
    module = importlib.import_module(module_path)
    by_name = getattr(module, "BY_NAME", None)
    if by_name is None:
        raise SystemExit(
            f"{module_path} has no BY_NAME; is it a module codegen.py wrote?"
        )
    resolved = []
    for name in names:
        if name not in by_name:
            known = ", ".join(sorted(by_name))
            raise SystemExit(f"{module_path} declares no table {name!r}; has {known}")
        resolved.append(by_name[name])
    return resolved


def sweep(client: Client, tables: list[Table], window: int, *, ceiling: int) -> int:
    """Purge each table once, and answer how many rows were erased.

    The ceiling is why this loops per table rather than issuing one call: the
    RPC refuses a sweep larger than its bound rather than erasing a prefix, so
    a backlog has to be worked down over several runs. Reporting the count lets
    an operator see that happening instead of guessing.
    """
    before = int(time.time()) - window
    erased = 0
    session = client.session()
    for table in tables:
        count = session.purge_deleted(table, before, at_most=ceiling).affected
        erased += count
        print(
            f"{table.name}: erased {count} row(s) retired before {before}",
            flush=True,
        )
        if count == ceiling:
            # Not an error: the bound did its job. Said out loud because a
            # sweep that keeps hitting its ceiling is a sweep that is not
            # keeping up, and that is invisible from the count alone.
            print(
                f"{table.name}: hit the ceiling of {ceiling}; more rows remain",
                flush=True,
            )
    return erased


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--address", required=True, help="the head node, HOST:PORT")
    parser.add_argument(
        "--schema",
        required=True,
        help="dotted path to the generated declaration module (exposing BY_NAME)",
    )
    parser.add_argument(
        "--table",
        action="append",
        required=True,
        dest="tables",
        help="a soft-deleting table to sweep; repeatable",
    )
    parser.add_argument(
        "--older-than",
        type=duration,
        default=duration("30d"),
        help="retire rows deleted longer ago than this (default 30d)",
    )
    parser.add_argument(
        "--every",
        type=duration,
        default=duration("1h"),
        help="how often to sweep, when not --once (default 1h)",
    )
    parser.add_argument(
        "--at-most",
        type=int,
        default=10_000,
        help="the per-table ceiling the RPC enforces (default 10000)",
    )
    parser.add_argument("--once", action="store_true", help="one sweep, then exit")
    parser.add_argument(
        "--principal",
        default="u64:1",
        help="the identity to present; a purge is a privileged write",
    )
    parser.add_argument("--role", default="app", help="the role to present")
    args = parser.parse_args(argv)

    tables = tables_of(args.schema, args.tables)
    client = Client(
        args.address, Identity(args.principal, roles=[args.role])
    )
    while True:
        sweep(client, tables, args.older_than, ceiling=args.at_most)
        if args.once:
            return 0
        time.sleep(args.every)


if __name__ == "__main__":
    sys.exit(main())
