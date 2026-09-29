#!/usr/bin/env python3
"""Every client can be handed a configured transport, and something walks through.

A *transport door* is the argument through which a caller reaches the gRPC
channel a client dials for itself: interceptors, TLS, keepalive, message-size
limits. Python's `Client` takes `channel=`, Go's `Dial` takes
`...grpc.DialOption`, and TypeScript's `Client.connect` takes `options` — but
only since 2026-09-28, because it shipped without one and nobody noticed until
a round-trip counter needed an interceptor and there was nowhere to put it.
`ledger/2026-09-28-the-third-client-counts-and-the-go-instrument-was-half-blind.md`
records that, and left the caveat that nothing stops the next client repeating
it. This is that check.

**The roster is read off the filesystem, not typed here.** A fourth client is a
fourth directory under `clients/` with a manifest in it, and an unrostered one
is an error rather than a silent pass. That is the whole point: a list of three
clients maintained by hand would report "3 clients, all with doors" on the day
a fourth arrived without one — the shape `caveats.py`'s own docstring is about,
where a check that cannot see a thing reports the same clean as a check that
saw it and found nothing.

**A door nothing walks through is a string, not a door.** A parameter named
`options` proves nothing about whether an interceptor can be installed through
it; the TypeScript client had `credentials` for months and still could not take
one. So each door also names the demo adapter that uses it, and that use is
checked too. All three adapters install a round-trip counter through their
door, which is how the conformance runner compares their counts — the
demonstration already exists and this stops it quietly going away.

What it costs to add a client: an entry below naming the door and something
that walks through it. What it costs to *not* have a door is what this file
exists to make visible.

Run directly: `python3 scripts/check_transport_door.py`.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CLIENTS = ROOT / "clients"

#: A directory under `clients/` is a client when it declares a package. Any of
#: these; a fifth language brings a fifth manifest and the discovery below
#: reports a directory carrying none rather than skipping it, because
#: "unrecognised" and "fine" must not look the same here either.
MANIFESTS = ("pyproject.toml", "go.mod", "package.json", "Cargo.toml")


@dataclass(frozen=True)
class Door:
    """Where a client's transport can be configured, and what uses it."""

    #: The file declaring the entry point, relative to the repository root.
    entry: str
    #: What must appear in it. Written as the parameter's declaration rather
    #: than its name, so renaming the type is caught as well as removing it.
    opens: str
    #: How a reader should think of it, for the failure message.
    named: str
    #: The demo adapter that configures a transport through this door, and the
    #: call that does it. `examples/explorer/backends/<lang>` counts its own
    #: gRPC round trips with an interceptor installed exactly here.
    walks: str
    through: str


DOORS: dict[str, Door] = {
    "python": Door(
        entry="clients/python/src/slate/client.py",
        opens=r"channel: grpc\.Channel \| None = None",
        named="`Client(channel=...)`",
        walks="examples/explorer/backends/python/adapter/__main__.py",
        through=r"grpc\.intercept_channel\(",
    ),
    "go": Door(
        entry="clients/go/slate/client.go",
        opens=r"opts \.\.\.grpc\.DialOption",
        named="`Dial(target, identity, ...grpc.DialOption)`",
        walks="examples/explorer/backends/go/main.go",
        through=r"grpc\.WithChain(?:Unary|Stream)Interceptor\(",
    ),
    "typescript": Door(
        entry="clients/typescript/src/client.ts",
        opens=r"options: grpc\.ChannelOptions = \{\}",
        named="`Client.connect(target, identity, credentials, options)`",
        walks="examples/explorer/backends/node/src/main.ts",
        through=r"interceptors: \[",
    ),
}

#: npm is the one package manager here that will happily install a second copy
#: of a library whose objects cross the client's boundary, so the TypeScript
#: door is only usable if the client also hands out the grpc-js it was built
#: against. Python has one environment and Go has one module graph, so neither
#: needs this and neither is checked for it.
REEXPORT = ("clients/typescript/src/index.ts", r'export \* as grpc from "@grpc/grpc-js";')


def discovered() -> tuple[list[str], list[str]]:
    """Client directories under `clients/`, and any that declare no package."""
    named, unmanifested = [], []
    for child in sorted(CLIENTS.iterdir()) if CLIENTS.is_dir() else []:
        if not child.is_dir() or child.name.startswith("."):
            continue
        (named if any((child / m).exists() for m in MANIFESTS) else unmanifested).append(
            child.name
        )
    return named, unmanifested


def missing(relative: str, pattern: str) -> str | None:
    """`None` when `pattern` occurs in the file, else why it does not."""
    path = ROOT / relative
    if not path.exists():
        return f"{relative} does not exist"
    return None if re.search(pattern, path.read_text()) else f"{relative} has no `{pattern}`"


def main() -> int:
    problems = []
    clients, unmanifested = discovered()

    if not clients:
        # The never-fires case. No clients found means the tree moved, and a
        # rule that checks nothing must not print the same line as one that
        # checked three things and liked them.
        problems.append(
            f"no client directories found under {CLIENTS}. Either the layout "
            "moved or MANIFESTS is stale — both need a person, not a pass."
        )
    for name in unmanifested:
        problems.append(
            f"clients/{name} declares none of {list(MANIFESTS)}, so this rule "
            "cannot tell whether it is a client. Add its manifest name to "
            "MANIFESTS, or say here why the directory is not a client."
        )
    for name in sorted(set(clients) - set(DOORS)):
        problems.append(
            f"clients/{name} is a client and DOORS has no entry for it.\n"
            "  A client with no way to pass interceptors, TLS or keepalive "
            "through to its channel is one a caller cannot instrument or "
            "secure — the TypeScript client shipped that way and it took "
            "writing a round-trip counter to find out. Give it a door, then "
            "add it here naming the door and something that walks through it."
        )
    for name in sorted(set(DOORS) - set(clients)):
        problems.append(
            f"DOORS has an entry for {name} and clients/{name} is not a "
            "client directory any more. Fix the entry, or this rule is "
            "checking a client that is gone."
        )

    for name in sorted(set(DOORS) & set(clients)):
        door = DOORS[name]
        if why := missing(door.entry, door.opens):
            problems.append(
                f"{name}'s transport door is gone: {why}.\n"
                f"  It is {door.named}, and without it a caller cannot install "
                "an interceptor or bring their own credentials."
            )
        if why := missing(door.walks, door.through):
            problems.append(
                f"nothing walks through {name}'s transport door: {why}.\n"
                f"  {door.walks} installs the demo's round-trip counter "
                "through it, which is what makes the door a door rather than "
                "a parameter with a promising name. If the adapter moved, "
                "point this entry at whatever configures a transport now."
            )

    if "typescript" in clients and (why := missing(*REEXPORT)):
        problems.append(
            f"the TypeScript client no longer re-exports grpc-js: {why}.\n"
            "  Its door takes grpc-js types, so a caller needs *this* copy. "
            "Depending on `@grpc/grpc-js` from the calling package installs a "
            "second instance, and two copies of a library whose objects cross "
            "the boundary is the bug nobody wants to debug from an adapter."
        )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(f"ok    {len(clients)} clients, each with a transport door something walks through")
    return 0


if __name__ == "__main__":
    sys.exit(main())
