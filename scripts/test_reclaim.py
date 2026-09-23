#!/usr/bin/env python3
"""Cases for `scripts/reclaim.py`, over a fabricated `target/`.

Nothing here touches the real build cache. Every case builds its own tree,
which matters more than usual for this script: the thing under test deletes
files, and a fixture that reached the repository's `target/` would be
indistinguishable from the script working.
"""

from __future__ import annotations

import os
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import reclaim


def build(root: pathlib.Path, files: dict[str, tuple[int, int]]) -> None:
    """`{"debug/deps/liba-<hash>.rlib": (size, mtime)}` → a tree on disk."""
    for name, (size, when) in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"\0" * size)
        os.utime(path, (when, when))


def case_newest_of_each_target_is_kept() -> tuple[bool, str]:
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        build(root, {
            "debug/deps/libkernel-000000000000000a.rlib": (100, 1_000),
            "debug/deps/libkernel-000000000000000b.rlib": (100, 2_000),
            "debug/deps/libkernel-000000000000000c.rlib": (100, 3_000),
        })
        freed, files = reclaim.sweep(root)
        left = sorted(p.name for p in (root / "debug/deps").iterdir())
        if left != ["libkernel-000000000000000c.rlib"]:
            return False, f"kept {left}"
        if (freed, files) != (200, 2):
            return False, f"reported {freed} bytes across {files}"
    return True, ""


def case_extension_is_part_of_the_key() -> tuple[bool, str]:
    """The 0.1 GB / 6.5 GB defect, as a test.

    One target emits several artifacts under the same hash. Keying on the
    stem alone treats `.rlib`, `.rmeta` and `.d` as copies of each other and
    deletes two of the three — and, in the version this replaces, skipped
    every name containing a dot instead, leaving the rlibs that are most of
    what is on disk.
    """
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        build(root, {
            "debug/deps/libkernel-000000000000000a.rlib": (100, 1_000),
            "debug/deps/libkernel-000000000000000a.rmeta": (100, 1_000),
            "debug/deps/libkernel-000000000000000a.d": (100, 1_000),
        })
        freed, _ = reclaim.sweep(root)
        left = sorted(p.suffix for p in (root / "debug/deps").iterdir())
        if left != [".d", ".rlib", ".rmeta"]:
            return False, f"kept {left}, freed {freed}"
    return True, ""


def case_build_is_never_touched() -> tuple[bool, str]:
    """`target/*/build` holds build-script output. Deleting it is the trap.

    Swept with `build` in the disposable list *on purpose*. With the real
    list the refusal never runs — nothing in it is protected — so a case
    using the default would assert nothing and pass for the wrong reason.
    This asks the sweep to delete it and requires the refusal to say no.
    """
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        build(root, {
            "debug/build/lz4-sys-000000000000000a/out/liblz4.a": (500, 1_000),
            "debug/build/lz4-sys-000000000000000b/out/liblz4.a": (500, 2_000),
        })
        reclaim.sweep(root, disposable=("incremental", "examples", "build"))
        survived = list((root / "debug/build").rglob("*.a"))
        if len(survived) != 2:
            return False, f"{len(survived)} of 2 build-script outputs survived"
        if not reclaim.protected(pathlib.Path("target/debug/build/x/out/y.a")):
            return False, "protected() does not recognise a build path"
    return True, ""


def case_disposable_directories_go_whole() -> tuple[bool, str]:
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        build(root, {
            "debug/incremental/a/b.bin": (700, 1_000),
            "debug/examples/cost_at_scale-000000000000000a": (300, 1_000),
        })
        freed, files = reclaim.sweep(root)
        if (root / "debug/incremental").exists() or (root / "debug/examples").exists():
            return False, "a disposable directory survived"
        if (freed, files) != (1000, 2):
            return False, f"reported {freed} bytes across {files}"
    return True, ""


def case_dry_run_removes_nothing() -> tuple[bool, str]:
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        build(root, {
            "debug/deps/libkernel-000000000000000a.rlib": (100, 1_000),
            "debug/deps/libkernel-000000000000000b.rlib": (100, 2_000),
            "debug/incremental/a.bin": (100, 1_000),
        })
        freed, files = reclaim.sweep(root, dry_run=True)
        if (freed, files) != (200, 2):
            return False, f"reported {freed} bytes across {files}"
        if len(list((root / "debug/deps").iterdir())) != 2:
            return False, "the dry run deleted an artifact"
        if not (root / "debug/incremental").is_dir():
            return False, "the dry run deleted a directory"
    return True, ""


def case_a_missing_target_says_so() -> tuple[bool, str]:
    """Rather than reporting 0.00 GB, which reads as "already clean".

    This is the failure the pasted snippet had no way to report: run from the
    wrong directory it found nothing and said so in the same words it uses
    for a tree with nothing to free.
    """
    import contextlib
    import io

    with tempfile.TemporaryDirectory() as directory:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = reclaim.main(["--target", str(pathlib.Path(directory) / "nope")])
        if code != 0 or "does not exist" not in out.getvalue():
            return False, f"exit {code}: {out.getvalue()!r}"
    return True, ""


def case_a_tie_is_broken_the_same_way_twice() -> tuple[bool, str]:
    """Two artifacts with identical mtimes must not swap between runs."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        names = {f"debug/deps/libkernel-00000000000000{n:02x}.rlib": (100, 1_000)
                 for n in range(8)}
        build(root, names)
        stale = reclaim.superseded(root / "debug/deps")
        if len(stale) != 7:
            return False, f"kept {8 - len(stale)} of 8"
        # Which one survives has to be decided by the data, not by the order
        # the filesystem happened to hand them over: `scandir` order is stable
        # within one process, so comparing two calls proves nothing. The path
        # is the documented tiebreak, so the survivor is the greatest of them.
        kept = {f"libkernel-00000000000000{n:02x}.rlib" for n in range(8)}
        kept -= {path.name for path in stale}
        if kept != {max(f"libkernel-00000000000000{n:02x}.rlib" for n in range(8))}:
            return False, f"kept {kept}, which is not the documented tiebreak"
    return True, ""


CASES = [
    ("the newest build of each target is kept", case_newest_of_each_target_is_kept),
    ("the extension is part of the key", case_extension_is_part_of_the_key),
    ("build-script output is never touched", case_build_is_never_touched),
    ("incremental and examples go whole", case_disposable_directories_go_whole),
    ("a dry run removes nothing", case_dry_run_removes_nothing),
    ("a missing target says so, not 0.00 GB", case_a_missing_target_says_so),
    ("a tie is broken the same way twice", case_a_tie_is_broken_the_same_way_twice),
]


def main() -> int:
    failed = 0
    for name, run in CASES:
        try:
            ok, why = run()
        except Exception as raised:  # noqa: BLE001 - a crash is this failure
            ok, why = False, f"raised {raised!r}"
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        {why}")
    print(f"\n{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
