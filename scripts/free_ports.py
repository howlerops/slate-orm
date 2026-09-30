#!/usr/bin/env python3
"""Free TCP ports below the ephemeral range, for a harness to hand to a server.

    python3 scripts/free_ports.py            # one port
    python3 scripts/free_ports.py --count 5  # five, space separated, one line

# Why not just bind to port 0

Because the harness has to *release* the port before the server binds it, and
between those two moments the kernel may hand the same number to an outgoing
connection. `ledger/2026-09-14-ports-below-the-ephemeral-range.md` records that
happening: the demo job went red with `EADDRINUSE` on port 38721, and it read
as the three SDKs disagreeing rather than as the harness handing out a port the
kernel was about to reuse.

Ports **below** the ephemeral range are never handed out automatically, so
nothing can take one from under us. Each is still probed before being offered,
because something else on the machine may already be listening there.

# Why this is a file and not two heredocs

It was two heredocs — `examples/explorer/run.sh` offering five ports and
`examples/batchbench/run.sh` offering one, written from it — and the entry that
tested them said what that costs:

> **It does not remove the duplicate.** Two allocators with one behaviour is
> one edit away from two behaviours, and the guard makes that edit *visible*
> rather than impossible: somebody who changes one copy and not the other still
> gets green, because both copies independently satisfy the invariant.
> — `ledger/2026-09-28-the-port-allocator-runs-in-a-test-now.md`

Both runners resolve the repository root already, so calling one script costs
them a line each.

Run directly: `python3 scripts/free_ports.py --count 5`.
"""

from __future__ import annotations

import argparse
import random
import socket
import sys
from collections.abc import Callable

#: Where `/proc` publishes the ephemeral range, and what to assume when it
#: cannot be read. The floor is read rather than assumed because a container
#: can be configured with a different one, and picking "below 32768" on a
#: machine whose range starts at 15000 would reintroduce exactly the bug.
RANGE: str = "/proc/sys/net/ipv4/ip_local_port_range"
ASSUMED = 32768

#: The bottom of what this will offer. Below 1024 needs privilege and the
#: 1024-10000 band is where most things a developer runs already listen.
LOW = 10000


def floor() -> int:
    """The bottom of the ephemeral range, read rather than assumed."""
    try:
        with open(RANGE) as handle:
            return int(handle.read().split()[0])
    except (OSError, ValueError):
        return ASSUMED


def free(count: int, below: int, draw: Callable[[int, int], int] = random.randint) -> list[int]:
    """`count` distinct free ports in `[LOW, below)`, each probed.

    `draw` is `random.randint` and is a parameter so a test can pin it. Two of
    the properties here are *boundary* properties — that the top port offered
    is `below - 1`, and that a repeated draw is not offered twice — and a
    random draw tests each of those with a probability rather than with a
    case: both mutations that broke them survived a real run, because one port
    in twenty thousand and one collision in five draws are not something a
    suite notices. Passing the draw in is what makes them assertions.

    The upper bound is `below - 1` and nothing else. Both heredocs this
    replaces wrote it as `min(max(below - 1, 10100), below - 1)`, and the
    `max` term never changed the answer: `min(max(x, 10100), x)` is `x` for
    every `x`, checked over every floor from 2 to 69999. The entry that tested
    those heredocs reasoned carefully about a floor of 10050 — where the `max`
    wins and the `min` is therefore load-bearing — and that reasoning was right
    about the `min`; what neither it nor this noticed until the two copies were
    put side by side is that the pair together does what the `min` does alone.

    A floor at or below `LOW` raises out of `random.randint`, and that is the
    right answer rather than a caught exception: on a machine where the
    ephemeral range starts at 10000 there is no port that satisfies the whole
    point of this file, so a stopped harness beats a port inside the range.
    """
    chosen: list[int] = []
    while len(chosen) < count:
        candidate = draw(LOW, below - 1)
        if candidate in chosen:
            continue
        probe = socket.socket()
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            probe.bind(("127.0.0.1", candidate))
        except OSError:
            continue  # somebody is listening there; try another
        finally:
            probe.close()
        chosen.append(candidate)
    return chosen


def main(argv: list[str] | None = None) -> int:
    parsed = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parsed.add_argument("--count", type=int, default=1, help="how many ports (default 1)")
    args = parsed.parse_args(argv)
    if args.count < 1:
        print("--count must be at least 1", file=sys.stderr)
        return 2
    print(" ".join(str(port) for port in free(args.count, floor())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
