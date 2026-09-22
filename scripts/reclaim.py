#!/usr/bin/env python3
"""Reclaim build-cache disk, the way this container actually needs it done.

Disk here is a fixed per-session allowance and several builds share it. The
first symptom is never "disk full": it is a linker `Bus error`, an
`rustc-LLVM ERROR: IO failure on output stream`, or a burst of `E0463: can't
find crate` that reads exactly like broken code. `cargo test --workspace` does
not fit at all.

The recipe lived in `ledger/README.md` as a snippet to paste. It was pasted
five times in one session, twice with the working directory wrong, and the
snippet has no way to say so — it `cd`s into `target/debug/deps` and reports
"freed 0.00 GB" from wherever it happens to land, which is indistinguishable
from a tree that is already clean. A file can check.

WHAT IT REMOVES, AND WHAT IT REFUSES TO

* `target/*/deps` — every superseded build of each target, keeping the newest.
  This is where the gigabytes are.
* `target/*/incremental` and `target/*/examples` — regenerated on demand.

It never touches `target/*/build`. Those are build-script outputs, and deleting
them produces hundreds of convincing, fictional compile errors in dependencies
that were fine; `cargo clean -p <crate>` is the repair. That is not a comment
here, it is a refusal: `PROTECTED` is checked against every path before it is
unlinked, so a future edit that adds `build` to the sweep list fails a test
rather than costing somebody an afternoon.
"""

from __future__ import annotations

import argparse
import collections
import os
import re
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: `libslate_kernel-8637c26b4ed436af.rlib` → ("libslate_kernel", ".rlib").
#:
#: The extension is part of the key, and that is the whole difference between
#: this freeing 0.1 GB and freeing 6.5 GB. An earlier version keyed on the
#: stem alone and skipped any name containing a dot, which dedupped the test
#: binaries — a handful of files — and left every `.rlib` and `.rmeta` behind,
#: which is most of what is on disk. One target legitimately produces several
#: artifacts from the same hash, and they are not copies of each other.
ARTIFACT = re.compile(r"^(.*)-[0-9a-f]{16}(\.[A-Za-z0-9.]+)?$")

#: Directory names under `target/<profile>/` that are safe to remove whole.
DISPOSABLE = ("incremental", "examples")

#: Never removed, at any depth, for any reason. See the module docstring.
PROTECTED = ("build",)


def protected(path: Path) -> bool:
    """Whether `path` is inside a directory this must not touch."""
    return any(part in PROTECTED for part in path.parts)


def superseded(deps: Path) -> list[Path]:
    """Every artifact in `deps` that a newer build of the same target replaced.

    Grouped by (name, extension) and sorted by modification time, newest
    first; everything after the first in each group is superseded.
    """
    groups: dict[tuple[str, str], list[tuple[float, Path]]] = collections.defaultdict(list)
    try:
        entries = list(os.scandir(deps))
    except OSError:
        return []
    for entry in entries:
        match = ARTIFACT.match(entry.name)
        if not match:
            continue
        try:
            stat = entry.stat()
        except OSError:
            continue
        groups[(match.group(1), match.group(2) or "")].append((stat.st_mtime, Path(entry.path)))
    stale: list[Path] = []
    for found in groups.values():
        # Sorting on mtime alone leaves ties in filesystem order, which is not
        # stable between runs; the path breaks the tie so a dry run and the
        # run after it agree about which copy is kept.
        found.sort(key=lambda pair: (pair[0], str(pair[1])), reverse=True)
        stale.extend(path for _, path in found[1:])
    return stale


def sweep(
    target: Path,
    dry_run: bool = False,
    disposable: tuple[str, ...] = DISPOSABLE,
) -> tuple[int, int]:
    """Remove what is safe to remove. Returns (bytes freed, files removed).

    `disposable` is a parameter so the `PROTECTED` refusal can be exercised
    rather than assumed. With the default list it never fires — nothing in it
    is protected — which makes it exactly the dead safety check this
    repository has been bitten by twice. A test passes `("build",)` here and
    asserts the build-script output survives anyway.
    """
    freed = files = 0
    for profile in sorted(p for p in target.glob("*") if p.is_dir()):
        for path in superseded(profile / "deps"):
            if protected(path):
                continue
            try:
                size = path.stat().st_size
            except OSError:
                continue
            if not dry_run:
                try:
                    path.unlink()
                except OSError:
                    continue
            freed += size
            files += 1
        for name in disposable:
            directory = profile / name
            if not directory.is_dir() or protected(directory):
                continue
            for walked, _, names in os.walk(directory):
                for one in names:
                    try:
                        freed += (Path(walked) / one).stat().st_size
                        files += 1
                    except OSError:
                        continue
            if not dry_run:
                shutil.rmtree(directory, ignore_errors=True)
    return freed, files


def gigabytes(count: int) -> str:
    return f"{count / 1024 ** 3:.2f} GB"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dry-run", action="store_true",
                        help="report what would go, remove nothing")
    parser.add_argument("--target", type=Path, default=ROOT / "target",
                        help="the cargo target directory (default: this repo's)")
    args = parser.parse_args(argv)

    if not args.target.is_dir():
        # Not an error worth failing over — a fresh clone has no `target/` —
        # but saying so beats reporting "freed 0.00 GB", which is what the
        # pasted snippet said from the wrong directory and reads identically
        # to a tree that is already clean.
        print(f"{args.target} does not exist; nothing to reclaim")
        return 0

    before = shutil.disk_usage(args.target).free
    freed, files = sweep(args.target, args.dry_run)
    after = shutil.disk_usage(args.target).free

    verb = "would free" if args.dry_run else "freed"
    print(f"{verb} {gigabytes(freed)} across {files} files")
    if not args.dry_run:
        # The filesystem's own number, which is the one that decides whether
        # the next link succeeds. It can differ from the sum above — another
        # build writing concurrently, or a file still held open — and when it
        # does, this is the honest one.
        print(f"free on {args.target.anchor or '/'}: "
              f"{gigabytes(before)} -> {gigabytes(after)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
