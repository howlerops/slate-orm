#!/usr/bin/env python3
"""No build output is tracked, and every package root's output is ignored.

    python3 scripts/check_build_output.py

`.gitignore` says, in a comment written after this happened twice:

> the cost is that every new example must add its own line, and nothing
> enforces that. If this happens a third time, the answer is a check rather
> than another comment.

`ledger/2026-09-18-untrack-the-benchmarks-build-output.md` recorded the same
thing as a caveat — "nothing prevents the fourth example doing this again" —
and this is the check both asked for. It is written before the third time
rather than after it, which is the only interesting thing about it.

# Two checks, because they fail at different moments

**Nothing tracked lives under a build directory.** The failure that already
happened: a half-megabyte of `dist/` and a Go binary committed, noticed later,
untracked in a follow-up, and still in the pack forever. This catches it at the
commit that does it, which is the last moment it is cheap.

**Every package root's `node_modules/` and `dist/` are ignored.** The failure
one step earlier: a new example is added, `.gitignore`'s per-directory entries
do not match it, and the output is merely *waiting* to be committed. A tree can
pass the first check and fail this one, and that gap is the whole reason the
`.gitignore` comment exists.

# Why per-directory ignores at all

A blanket `**/node_modules/` would fix this and would also hide a
`node_modules` somebody genuinely meant to vendor. `.gitignore` says so and
keeps the per-directory style deliberately; this check is the enforcement that
choice was missing, not an argument against it.

# What it does not catch

A build directory under a name not in `BUILD_DIRS` — `out/`, `.next/`, `bin/`.
The list is the names this repository's toolchains actually write, and a new
toolchain is a line here. That is the roster failure mode this repository knows
well, and it is accepted because the alternative is guessing at directory names
by shape.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]

#: Directory names whose contents are built, never authored.
#:
#: `target` is cargo's, `dist` and `dist-test` are the TypeScript builds,
#: `node_modules` is npm's, `build` is the generic one. Each is a name this
#: repository's own tooling writes — not a guess at what a build directory
#: looks like.
BUILD_DIRS = ("node_modules", "dist", "dist-test", "build", "target")

#: Output directories every JavaScript package root must have ignored.
#:
#: Narrower than `BUILD_DIRS` on purpose: `target` is cargo's and a
#: `package.json` says nothing about it, and a `build/` is not something these
#: packages write.
PACKAGE_OUTPUT = ("node_modules", "dist")


def tracked(root: Path) -> list[str]:
    """Every path git tracks, as it spells them."""
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files"],
        capture_output=True,
        text=True,
        check=True,
    )
    return [line for line in out.stdout.splitlines() if line]


def ignored(root: Path, path: str) -> bool:
    """Whether git would ignore `path` under `root`.

    `git check-ignore` exits 0 when the path *is* ignored and 1 when it is not,
    which is the opposite of the usual reading and the reason this is a named
    function rather than a call site.
    """
    return (
        subprocess.run(
            ["git", "-C", str(root), "check-ignore", "-q", path],
            capture_output=True,
        ).returncode
        == 0
    )


def package_roots(paths: list[str]) -> list[str]:
    """Directories holding a tracked `package.json`.

    From the tracked list rather than from a walk of the disk, so a
    `package.json` inside an untracked `node_modules` — of which there are
    thousands — is not mistaken for one of this repository's packages.
    """
    roots = []
    for path in paths:
        if path == "package.json":
            roots.append("")
        elif path.endswith("/package.json"):
            roots.append(path[: -len("/package.json")])
    return sorted(set(roots))


def check(root: Path) -> list[tuple[str, bool, str]]:
    """`(what, ok, detail)` per check, in the shape the other guards use."""
    out: list[tuple[str, bool, str]] = []

    def record(what: str, ok: bool, detail: str = "") -> None:
        out.append((what, ok, detail))

    paths = tracked(root)
    record("the tree tracks some files", bool(paths), f"{len(paths)}")

    committed = [path for path in paths if any(part in BUILD_DIRS for part in path.split("/")[:-1])]
    record(
        "no tracked file lives under a build directory",
        not committed,
        "\n      ".join(committed[:20]),
    )

    roots = package_roots(paths)
    record("there is at least one package root to check", bool(roots), f"{len(roots)}")

    exposed = [
        f"{here}/{name}" if here else name
        for here in roots
        for name in PACKAGE_OUTPUT
        if not ignored(root, f"{here}/{name}/x" if here else f"{name}/x")
    ]
    record(
        "every package root has its node_modules and dist ignored",
        not exposed,
        ", ".join(exposed),
    )
    return out


def main() -> int:
    failed = 0
    for what, ok, detail in check(REPO):
        if ok:
            print(f"ok    {what}")
        else:
            failed += 1
            print(f"FAIL  {what}" + (f"\n      {detail}" if detail else ""))
    print()
    if failed:
        print(f"{failed} failed")
        return 1
    roots = package_roots(tracked(REPO))
    print(
        f"no build output is tracked, and all {len(roots)} package roots have their output ignored"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
