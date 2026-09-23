#!/usr/bin/env python3
"""Every crate's examples are run by something, and `run_examples.sh` is honest.

`scripts/run_examples.sh` holds three hand-maintained facts about this tree: a
floor per crate, a handshake line per server example, and — by omission — the
list of crates whose examples anything runs at all. None of the three was
checked, and the third was wrong when this was written: **four** crates carry
an `examples/` directory and the script named two. `slate-kernel`'s four and
`slate-orm`'s one were in precisely the state `slate-headbench`'s five were in
before #265 — compiled by `cargo clippy --all-targets`, run by nobody.

That is the same defect three times now (#265, #271, here), which is what makes
it worth a guard rather than a fourth fix. A benchmark nobody runs is a
constant nobody re-measures.

The three rules, each in both directions:

1. **A crate with examples has a floor.** This is the one that found the
   defect. A fifth crate gaining a benchmark directory fails here rather than
   joining them in silence.
2. **A floor equals the count.** `least` is compared with `-lt`, so a floor
   that drifts *below* the real count stops catching a deleted benchmark —
   quietly, since the run still passes. Deliberately equality rather than a
   bound: the floor's whole job is to be exact about how many there are.
3. **A server example has a handshake, and only a server example has one.** An
   example that never returns and is not in the roster makes `--smoke` wait
   for it to exit, forever; that is what the first run of `run_examples.sh`
   did. A roster entry for an example that has been deleted, or that now runs
   to completion, spends the wait budget on nothing. The expected line must
   also occur in that example's source, or the budget is spent and the run
   fails with `no LISTENING in 30s` against a binary that prints something
   else.

Run directly: `python3 scripts/check_examples_roster.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / "scripts/run_examples.sh"
MANIFEST = ROOT / "Cargo.toml"

#: `    slate-headbench) least=5 ;;`
FLOOR = re.compile(r"^\s*([a-z0-9-]+)\)\s*least=(\d+)\s*;;", re.MULTILINE)
#: `        slate-slatedb/s3_server) echo "LISTENING" ;;`
HANDSHAKE = re.compile(r'^\s*([a-z0-9-]+)/(\w+)\)\s*echo\s*"([^"]+)"\s*;;', re.MULTILINE)

#: What an example that never returns looks like.
#:
#: Not a roster of names — a property of the source, so a *second* server
#: benchmark is caught by being one rather than by somebody remembering. Both
#: spellings are here because either would do it; only the first is used today,
#: and a guard below fails if neither ever matches.
FOREVER = re.compile(r"future::pending|signal::ctrl_c")


def crates(manifest: Path = MANIFEST) -> list[str]:
    """Workspace members, by directory name.

    Read from `Cargo.toml` rather than globbed off disk: a directory under
    `crates/` that is not a member is not built by anything, and listing it
    here would demand a floor for examples `cargo` never compiles.
    """
    members = tomllib.loads(manifest.read_text(encoding="utf-8"))["workspace"]["members"]
    return [Path(member).name for member in members]


def problems(runner: Path = RUNNER, root: Path = ROOT, manifest: Path = MANIFEST) -> list[str]:
    """Every rule above, against a tree."""
    script = runner.read_text(encoding="utf-8")
    floors = {name: int(least) for name, least in FLOOR.findall(script)}
    shakes = {(crate, name): line for crate, name, line in HANDSHAKE.findall(script)}

    # Where the examples actually are. A member with no `examples/` is not a
    # problem — most have none.
    found: dict[str, dict[str, str]] = {}
    for crate in crates(manifest):
        directory = root / "crates" / crate / "examples"
        if not directory.is_dir():
            continue
        sources = sorted(directory.glob("*.rs"))
        if sources:
            found[crate] = {
                source.stem: source.read_text(encoding="utf-8") for source in sources
            }

    said = []

    # The never-fires halves, and there are three because this reads three
    # different files with three different patterns. Each would match nothing
    # after an ordinary edit — the `case` rewritten as an `if`, the manifest's
    # members moved, `examples/` renamed — and leave this printing `ok` over a
    # tree it did not read.
    if not floors:
        said.append(
            f"no `<crate>) least=N ;;` lines in {runner.name}, so the floors "
            "below checked nothing. The `case` was rewritten; move this "
            "pattern with it."
        )
    if not shakes:
        said.append(
            f"no `<crate>/<example>) echo \"...\" ;;` lines in {runner.name}. "
            "Either the handshake table moved, or the last server example "
            "went away — and if it went away, delete this guard deliberately."
        )
    if not found:
        said.append(
            "no workspace member has an `examples/` directory, which was "
            "false for four of them when this was written. Either the layout "
            "moved or `Cargo.toml`'s members no longer parse."
        )
    if said:
        return said

    for crate, sources in sorted(found.items()):
        if crate not in floors:
            said.append(
                f"`{crate}` has {len(sources)} example(s) and no floor in "
                f"{runner.name}, so nothing runs them — they are compiled by "
                "`cargo clippy --all-targets` and executed by no job.\n"
                f"  Add `{crate}) least={len(sources)} ;;` and a CI step that "
                "calls the runner, or say here why this crate is different."
            )
        elif floors[crate] != len(sources):
            said.append(
                f"`{crate}` has {len(sources)} example(s) and a floor of "
                f"{floors[crate]}.\n"
                "  A floor below the count stops catching a deleted "
                "benchmark; one above fails every run. Make it "
                f"`least={len(sources)}`."
            )

    for crate in sorted(floors):
        if crate not in found:
            said.append(
                f"{runner.name} carries a floor for `{crate}`, which has no "
                "examples. Delete it, or the runner refuses a crate it "
                "claims to know."
            )

    # Rule 3, both ways.
    servers = {
        (crate, name)
        for crate, sources in found.items()
        for name, body in sources.items()
        if FOREVER.search(body)
    }
    if not servers:
        said.append(
            "no example anywhere awaits forever, so the handshake table is "
            "checking nothing. If the last server benchmark went away, delete "
            "the table and this rule together; if one is spelled some new way, "
            "teach `FOREVER` about it."
        )
    for crate, name in sorted(servers):
        if (crate, name) not in shakes:
            said.append(
                f"`{crate}/{name}` never returns and has no handshake line, so "
                "a smoke run waits for it to exit and hangs.\n"
                "  Add a line to `handshake()` naming what it prints once it "
                "is up."
            )
    for (crate, name), line in sorted(shakes.items()):
        body = found.get(crate, {}).get(name)
        if body is None:
            said.append(
                f"`handshake()` names `{crate}/{name}`, which is not an "
                "example here any more. Delete the line."
            )
        elif (crate, name) not in servers:
            said.append(
                f"`handshake()` names `{crate}/{name}`, which runs to "
                "completion. The wait budget is spent on it for nothing — "
                "delete the line and let it be run like any other."
            )
        elif line not in body:
            said.append(
                f"`handshake()` waits for `{line}` from `{crate}/{name}`, "
                "which does not print it.\n"
                "  The run spends the whole budget and fails with a timeout "
                "against a binary that is up. Say what it really prints."
            )

    return said


def main() -> int:
    said = problems()
    if said:
        for one in said:
            print(one, file=sys.stderr)
        print(f"\n{len(said)} problem(s)", file=sys.stderr)
        return 1

    script = RUNNER.read_text(encoding="utf-8")
    floors = {name: int(least) for name, least in FLOOR.findall(script)}
    print(
        f"ok    {sum(floors.values())} examples across {len(floors)} crates, "
        f"every floor exact, {len(HANDSHAKE.findall(script))} handshake(s) "
        "rostered"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
