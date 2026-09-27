#!/usr/bin/env python3
"""A workflow that pins a language version must not silently need a newer one.

    python3 scripts/check_toolchain_pins.py

`actions/setup-go` sets `GOTOOLCHAIN=local` when `go-version` is pinned, which
is the right default — it stops a module quietly pulling down a different
compiler than the job asked for. It also turns any tool whose own floor is
higher than the pin into a hard error, and
`ledger/2026-09-27-setup-go-v6-pins-gotoolchain-and-the-generator-needed-1-25.md`
is what that cost: `protoc-gen-go-grpc@v1.6.2` declares `go >= 1.25.0`, the
workflow installs 1.24, and bumping the action to its first Node 24 major
turned the Go client job red with no Go changed. `GOTOOLCHAIN` had been `auto`
for weeks and had been fetching a 1.26 toolchain nobody had asked for.

The invariant that broke is checkable here and needs no network:

  **If a workflow pins `go-version`, every `go install <pkg>@<version>` this
  repository runs must set `GOTOOLCHAIN` explicitly.**

Not "must set it to `auto`". A script that sets it to `local` has decided that
its pins must fit the workflow's Go, which is a coherent position and a
different one; what is refused is the script that says nothing and inherits
whichever default the action currently has. That inheritance is the whole
defect: the behaviour changed under a file nobody edited.

# What this cannot do, and why it is still worth having

It cannot read the plugin's own `go.mod` and compare floors. That needs the
module proxy, and `scripts/check.sh` makes no network calls — the property it
would check is also the one CI checks for free, loudly, on the first run after
a pin moves. What CI *cannot* catch is the silent version: a job that works
today because of a default, which is exactly the shape that waited weeks.

Run directly: `python3 scripts/check_toolchain_pins.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: `          go-version: "1.24"` in a workflow. A pin, rather than a range or
#: a `go.mod` reference, is what makes `setup-go` choose `GOTOOLCHAIN=local`.
GO_VERSION = re.compile(r'^\s*go-version:\s*["\']?([\d.]+)["\']?\s*$', re.MULTILINE)

#: `go install example.com/thing/cmd/x@v1.2.3`, however it is spelled: a
#: literal command line in a shell step, or a Python list of arguments.
#:
#: Matched on the `@version` rather than on `go install`, because the pinned
#: form is the one with a floor of its own. `go install ./...` builds this
#: module and cannot want a Go the module does not declare.
GO_INSTALL = re.compile(
    r"""(?x)
    (?: ["']go["']\s*,\s*["']install["']        # ["go", "install", PKG]
      | \bgo\s+install\b )                      # go install PKG
    [^\n]*?
    ([\w.\-/]+@v?[\d][\w.\-+]*)                 # the pinned package
    """
)

#: What "this file has decided" looks like: the variable named at all.
DECIDES = re.compile(r"GOTOOLCHAIN")

#: Where a `go install` could live. Not `rglob("*")` over the workspace —
#: `target/` and `node_modules/` carry vendored scripts that install their own
#: tools and are none of this repository's business.
SKIP_PARTS = frozenset(
    {"node_modules", "dist", "dist-test", "target", ".git", "_proto", "__pycache__"}
)
SUFFIXES = (".py", ".sh", ".yml", ".yaml", ".mk", ".bash")


def workflows(root: Path) -> dict[str, list[str]]:
    """`{workflow: [pinned go-version, ...]}` for every workflow that pins one."""
    found: dict[str, list[str]] = {}
    directory = root / ".github" / "workflows"
    if not directory.is_dir():
        return found
    for path in sorted(directory.glob("*.yml")) + sorted(directory.glob("*.yaml")):
        pins = GO_VERSION.findall(path.read_text(encoding="utf-8"))
        if pins:
            found[path.relative_to(root).as_posix()] = pins
    return found


def installers(root: Path) -> dict[str, list[str]]:
    """`{file: [pinned package, ...]}` for every file running a pinned `go install`."""
    found: dict[str, list[str]] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix not in SUFFIXES:
            continue
        relative = path.relative_to(root)
        if SKIP_PARTS & set(relative.parts):
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        pinned = GO_INSTALL.findall(text)
        if pinned:
            found[relative.as_posix()] = pinned
    return found


def problems(root: Path = ROOT) -> list[str]:
    """Every file that installs a pinned Go tool and leaves `GOTOOLCHAIN` to chance."""
    pinned_workflows = workflows(root)
    running = installers(root)
    said: list[str] = []

    # The never-fires halves. Either list going empty means the shape this
    # guard is about has moved, not that the tree is clean — and with no
    # `go install` anywhere the forward loop below would report nothing while
    # the rule quietly checked an empty set.
    if not pinned_workflows:
        said.append(
            "no workflow pins `go-version:`, so this checked nothing. If Go "
            "is now taken from `go.mod`, `setup-go` no longer forces "
            "`GOTOOLCHAIN=local` and this guard can go — deliberately, with "
            "the reasoning, rather than by the pattern quietly missing."
        )
    if not running:
        said.append(
            "nothing in this repository runs `go install <pkg>@<version>`, "
            "which was false when this was written: "
            "`clients/go/scripts/generate_proto.py` installs two pinned "
            "protobuf generators. Either they moved or `GO_INSTALL` stopped "
            "matching them."
        )
    if said:
        return said

    where = ", ".join(sorted(pinned_workflows))
    for path, packages in sorted(running.items()):
        text = (root / path).read_text(encoding="utf-8")
        if DECIDES.search(text):
            continue
        said.append(
            f"{path} runs a pinned `go install` ({', '.join(sorted(set(packages)))}) "
            f"and never mentions `GOTOOLCHAIN`, while {where} pins "
            "`go-version`.\n"
            "  `actions/setup-go` sets `GOTOOLCHAIN=local` for a pinned "
            "version, so a tool whose own floor is higher fails the job "
            "outright — and until it does, the job is working on a default "
            "that can change under it. Set `GOTOOLCHAIN` here, to `auto` if "
            "the tool may bring its own compiler or to `local` if its pins "
            "must fit the workflow's Go."
        )
    return said


def main(root: Path = ROOT) -> int:
    said = problems(root)
    if said:
        for one in said:
            print(one, file=sys.stderr)
        print(f"\n{len(said)} problem(s)", file=sys.stderr)
        return 1
    pinned = workflows(root)
    running = installers(root)
    tools = sum(len(set(p)) for p in running.values())
    print(
        f"ok    {sum(len(v) for v in pinned.values())} pinned `go-version` "
        f"across {len(pinned)} workflow(s), {tools} pinned `go install` "
        f"target(s) in {len(running)} file(s), every one of them deciding "
        "`GOTOOLCHAIN` rather than inheriting it"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
