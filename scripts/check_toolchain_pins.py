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

#: `go install` being run at all, however it is spelled: a literal command line
#: in a shell step, or a Python list of arguments.
#:
#: Split from `PINNED` below, and that split is the whole fix. This was one
#: pattern requiring the `@version` on the same match as `go install` — and the
#: one file in this repository that installs pinned Go tools does not write it
#: that way. `clients/go/scripts/generate_proto.py` runs
#: `subprocess.run(["go", "install", package])` with the pins in two constants
#: fifty lines up, so the pattern never matched it, and the guard's entire
#: roster was its own docstring's example and its own test's fixture. It ran
#: for two days reporting `2 pinned go install target(s) in 2 file(s)` about
#: itself. Found by mutating the real tree: deleting the `GOTOOLCHAIN` line
#: from the real generator changed nothing here.
GO_INSTALL = re.compile(
    r"""(?x)
    (?: ["']go["']\s*,\s*["']install["']        # ["go", "install", ...]
      | \bgo\s+install\b )                     # go install ...
    """
)

#: A pinned package anywhere in the same file. Anywhere, because the pin and
#: the invocation are routinely apart: a constant at the top, a loop at the
#: bottom. A file with both is a file that installs a pinned Go tool, which is
#: what the invariant is about.
#:
#: `go install ./...` builds this module and carries no `@version`, so it does
#: not match and should not: it cannot want a Go the module does not declare.
PINNED = re.compile(r"[\w.\-/]+@v?[\d][\w.\-+]*")

#: This guard and its own test both carry example install lines — the pattern's
#: documentation and its fixture. Skipping them by name is what gives the
#: never-fires rule below something to bite on: with them in, a roster of
#: exactly these two read as a guard doing its job, and did.
NOT_AN_INSTALLER = (
    "scripts/check_toolchain_pins.py",
    "scripts/test_check_toolchain_pins.py",
)

#: What "this file has decided" looks like: the variable **assigned**, not
#: mentioned. It was `re.compile(r"GOTOOLCHAIN")` — the name anywhere — and
#: `generate_proto.py` explains its choice in twenty lines of comment above the
#: assignment, so deleting the assignment left eight mentions behind and this
#: guard green. The third time `scripts/` has met it: `check_generated_is_used.py`
#: passed on the word "schema" appearing in 483 files, `check_renamed_column.py`
#: passed on a declaration in an unrelated test, and now this. A bare mention
#: counting as a use. Comments are stripped before this is applied, because the
#: prose explaining why `GOTOOLCHAIN=auto` is right is not the setting.
DECIDES = re.compile(r"""GOTOOLCHAIN["']?\]?\s*[:=]""")

#: A whole-line comment, in every language this guard reads: Python, shell and
#: YAML all use `#`. Not an inline one — `env = {...}  # why` is a decision
#: with a note, and stripping from the `#` would be a parser this does not need
#: to be.
A_COMMENT = re.compile(r"^\s*#.*$", re.MULTILINE)

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
        name = relative.as_posix()
        if name in NOT_AN_INSTALLER:
            continue
        if not GO_INSTALL.search(text):
            continue
        # Comments stripped here too: `generate_proto.py` quotes its own pins
        # in the prose explaining them, and a message naming the same package
        # four times, once as `...protoc-gen-go-grpc@v1.6.2`, reads as four
        # tools rather than two.
        pinned = PINNED.findall(A_COMMENT.sub("", text))
        if pinned:
            found[name] = pinned
    return found


def stale_skips(root: Path = ROOT) -> list[str]:
    """Every name in `NOT_AN_INSTALLER` must still be a file carrying an example.

    The skip list was checked in neither direction: it named two paths and
    nothing asked whether they still existed or still carried the `go install`
    the skip excuses. That is the roster rot `EXTERNAL`, `WITNESS` and `PROVES`
    each check for and this one did not, recorded as a caveat in
    `ledger/2026-09-29-a-guard-whose-roster-was-itself.md` and closed here.

    A skip that excuses nothing is worse than no skip: it is a name in a
    frozenset that a reader takes for a live exemption, and if the file comes
    back carrying a *real* installer the skip hides it. The other direction
    needs no rule — a new file with an example and no entry here is reported as
    an installer, which is the guard asking to be told, and is what happened to
    this file and its test when the rule was written.
    """
    said: list[str] = []
    for name in NOT_AN_INSTALLER:
        try:
            text = (root / name).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            said.append(
                f"NOT_AN_INSTALLER names {name}, which is not a readable file. "
                "The skip excuses nothing; drop it, or fix the path."
            )
            continue
        if not GO_INSTALL.search(text):
            said.append(
                f"NOT_AN_INSTALLER names {name}, which no longer carries a "
                "`go install` example. The skip excuses nothing now, and if "
                "the file grows a real installer it will hide it. Drop the "
                "entry."
            )
    return said


def problems(root: Path = ROOT) -> list[str]:
    """Every file that installs a pinned Go tool and leaves `GOTOOLCHAIN` to chance."""
    pinned_workflows = workflows(root)
    running = installers(root)
    # Before anything else, because `installers()` has already applied the skip
    # list: a stale entry means the set below was computed with the wrong
    # exclusions, so reporting an installer against it would be reporting
    # against a tree this guard has misread.
    said: list[str] = stale_skips(root)
    if said:
        return said

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
        text = A_COMMENT.sub("", (root / path).read_text(encoding="utf-8"))
        if DECIDES.search(text):
            continue
        said.append(
            f"{path} runs a pinned `go install` ({', '.join(sorted(set(packages)))}) "
            f"and never sets `GOTOOLCHAIN`, while {where} pins "
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
