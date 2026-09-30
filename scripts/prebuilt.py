#!/usr/bin/env python3
"""Refuse a prebuilt server binary older than the source it was built from.

`SLATE_SERVERD` and `SLATE_TESTSERVER` let a harness skip `cargo` and run a
binary somebody already built. CI builds one per run and shares it across
eight jobs, which is most of what makes the client suites affordable. The
hazard is the obvious one: the binary is a *snapshot*, and a suite pointed at
a stale one tests a server this tree did not produce.

WHY THIS FILE EXISTS RATHER THAN THE CHECK LIVING WHERE IT STARTED

It happened twice, in two different shapes.

The first is in `clients/python/tests/conftest.py`, which is where this code
was written: a full run reported 153 passing tests against a `slate-testserver`
built before that session's server changes, so every test of the new behaviour
was checking the old server and passing, because the client asked for
something the old binary politely ignored.

The second was on 2026-09-30 and is what moved the check here. A mutation of
`crates/slate-server/src/status.rs` — deleting the `ErrorInfo` metadata map
entirely — **survived** the three-SDK conformance runner. Not because the
runner is weak: because `examples/explorer/run.sh` was pointed at a binary
built before the mutation, so the mutated code never ran. That is
`scripts/mutate.py`'s first documented lie ("the suite runs against unmutated
code and passes") reached through a door mutate.py cannot see, and it scores
as a *survivor*, which reads as "write a test" — the most expensive possible
misreading.

The check had been in one of four places that take the variable. This is the
one place, and `scripts/test_prebuilt.py` holds the four to using it.

WHAT IT COMPARES, AND WHY THAT IS ENOUGH

Modification time, which is crude and catches the whole of the real failure: a
binary handed over by CI is minutes old, and one a contributor or a mutation
run left behind is not. A content hash would be exact and would need the
source hash recorded somewhere at build time, which is a second thing to keep
in sync — and the failure it would catch beyond this one is "somebody touched
a file without changing it", which costs a rebuild rather than a wrong answer.
"""

from __future__ import annotations

import os
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Directories whose contents decide what the server does.
#:
#: The `.proto` is in there because the protocol is the thing a client and that
#: binary have to agree about, and a stale binary speaking an older one is
#: exactly the failure this exists for.
SOURCES = (ROOT / "crates", ROOT / "clients" / "python" / "testserver")

#: What a source file looks like. Anything else in those trees — a README, a
#: fixture, a lockfile — does not change what the binary does.
SUFFIXES = frozenset({".rs", ".toml", ".proto"})


def newest_source(root: Path = ROOT) -> tuple[float, Path] | None:
    """The most recently modified file the server is built from, or `None`."""
    newest: tuple[float, Path] | None = None
    for tree in (root / "crates", root / "clients" / "python" / "testserver"):
        if not tree.exists():
            continue
        for path in tree.rglob("*"):
            # `target/` is build output, not source, and walking it is slow
            # enough to notice: it is the biggest directory in the tree.
            if "target" in path.parts or path.suffix not in SUFFIXES:
                continue
            if not path.is_file():
                continue
            stamp = path.stat().st_mtime
            if newest is None or stamp > newest[0]:
                newest = (stamp, path)
    return newest


def complaint(binary: Path, variable: str, root: Path = ROOT) -> str | None:
    """Why `binary` must not be used, or `None` if it may be.

    Returns the sentence rather than raising, so a caller can raise the
    exception its own harness expects — `pytest` wants a `RuntimeError`, a
    shell wants a message and an exit code.
    """
    if not binary.exists():
        return f"{variable}={binary} is set and is not a file"
    newest = newest_source(root)
    if newest is None:
        return None
    stamp, source = newest
    if binary.stat().st_mtime >= stamp:
        return None
    try:
        named = source.relative_to(root)
    except ValueError:  # pragma: no cover - `root` always contains `source`
        named = source
    return (
        f"{variable}={binary} was built before {named} was last changed, so "
        f"this run would test a server this tree did not produce. Rebuild it, "
        f"or unset {variable} to build from source."
    )


def refuse_if_stale(binary: Path | str, variable: str, root: Path = ROOT) -> None:
    """`complaint`, raised."""
    said = complaint(Path(binary), variable, root)
    if said is not None:
        raise RuntimeError(said)


#: The variables this knows how to check.
#:
#: `ledger/2026-09-30-the-exemptions-i-wrote-without-reading.md` recorded the
#: hazard in the pair being hard-coded: *"`prebuilt.py` compares one binary
#: against the newest source in two trees … one verdict covers both, which is
#: right while they come from the same build and wrong the moment they do
#: not."* One verdict covers both because there is one binary to name; a third
#: variable naming a second binary would be unchecked and silent, which is the
#: shape `test_prebuilt.py`'s roster exists to make loud one level up.
#:
#: So `main` checks these and then *reports* any other `SLATE_*` variable
#: whose value is an executable file — the shape a binary variable has. It
#: refuses rather than checking it, because what to compare an unknown binary
#: against is a judgement, and a silent pass is the failure this whole file is
#: about.
KNOWN = ("SLATE_SERVERD", "SLATE_TESTSERVER")


def unknown_binaries(environ: dict[str, str] | None = None) -> list[str]:
    """Every `SLATE_*` variable naming an executable file that `KNOWN` omits."""
    env = os.environ if environ is None else environ
    out = []
    for variable, value in sorted(env.items()):
        if not variable.startswith("SLATE_") or variable in KNOWN or not value:
            continue
        path = Path(value)
        if path.is_file() and os.access(path, os.X_OK):
            out.append(variable)
    return out


def main() -> int:
    """Check every `SLATE_*` binary variable that is set. For shell callers."""
    checked = 0
    for variable in KNOWN:
        value = os.environ.get(variable)
        if not value:
            continue
        checked += 1
        said = complaint(Path(value), variable)
        if said is not None:
            print(said)
            return 1
    stray = unknown_binaries()
    if stray:
        print(
            f"{', '.join(stray)} names an executable file and is not one this "
            f"checks. A second prebuilt binary that nothing compares against its "
            f"source is the failure this file exists for; add it to `KNOWN` in "
            f"scripts/prebuilt.py, or unset it."
        )
        return 1
    # Not an error: building from source is the default and needs no check.
    # Said out loud so a caller cannot read silence as "the binary is fresh".
    print(f"ok    {checked} prebuilt binaries checked, none stale")
    return 0


if __name__ == "__main__":
    import sys

    sys.exit(main())
