#!/usr/bin/env python3
"""Every crate's examples are run by something, and `run_examples.sh` is honest.

`scripts/run_examples.sh` holds four hand-maintained facts about this tree: a
floor per crate, a handshake line per server example, a refusal line per
example that reads its own command line, and — by omission — the list of
crates whose examples anything runs at all. None of the three was
checked, and the third was wrong when this was written: **four** crates carry
an `examples/` directory and the script named two. `slate-kernel`'s four and
`slate-orm`'s one were in precisely the state `slate-headbench`'s five were in
before #265 — compiled by `cargo clippy --all-targets`, run by nobody.

That is the same defect three times now (#265, #271, here), which is what makes
it worth a guard rather than a fourth fix. A benchmark nobody runs is a
constant nobody re-measures.

The five rules, each in both directions:

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
4. **An example that reads its command line is handed a bad argument, and
   only such an example is.** `head_report --typo` printed its header, ran
   nothing and exited 0; `slate_headbench::sections` fixed that by exiting 2,
   and nothing ran a built binary to see it. A line in `refuses()` is what
   does, so an example that starts reading arguments without one is back where
   the defect was found. The argument must also not be a string in that
   example's source: one that is a real section or flag makes the example run,
   exit 0, and the run report a refusal as lost. Server examples are excluded
   in both directions — the runner returns to the next example as soon as a
   handshake lands, so a `refuses()` line for one is never reached.
5. **An example that reads its command line refuses an argument it does not
   understand.** The rule the other four rest on, and the one that was
   missing: rules 1-4 keyed the roster on `sections::from_args`, so an example
   parsing `std::env::args()` by hand — which is what all three headbench ones
   did before that module existed — was invisible to every one of them.
   `ledger/2026-09-26-the-refusal-nothing-ran.md` recorded it as "a fourth kind
   of silent pass".

   Widening the key from `sections::from_args` to `std::env::args` found two
   more of the defect immediately, both in `slate-slatedb`: `bucket_layout`
   read `--json` with `.any(|a| a == "--json")` and `cost_calibration` read
   `--cold` the same way, so `--jsonn` printed the human listing and `--codl`
   ran the warm suite, each exiting 0 with nothing to say the flag had not
   been understood. Both are fixed; the shape is now what `s3_server` already
   did — match every argument, exit 2 on one you do not know.

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
#:
#: Both per-example tables in the runner are spelled exactly this way, so this
#: pattern alone cannot tell a handshake line from a refusal line. `arm()`
#: below slices the named function's body out first and applies it to that,
#: which is why the pattern is shared rather than duplicated with a
#: distinguishing prefix nobody would keep true.
ARM = re.compile(r'^\s*([a-z0-9-]+)/(\w+)\)\s*echo\s*"([^"]+)"\s*;;', re.MULTILINE)

def body(script: str, function: str) -> str:
    """The text between `<function>() {` and the next **unindented** `}`.

    The anchor on the closing brace is load-bearing and its absence is
    invisible in this tree today: neither table's arms contain a brace, so a
    lazy match to the first `}` anywhere slices exactly the same text and every
    case here still passes. A `${…}` in one arm — a variable, a default, a
    substitution, all ordinary things to write in a `case` — would then end the
    body at that line and silently drop every arm after it. That is a guard
    reading half a table and reporting `ok`, so `test_check_examples_roster.py`
    has a case with a brace in an arm rather than trusting the comment.

    Empty when the function is not there, which reads the same as a table with
    no arms. Deliberately not distinguished: the callers' message names both
    causes already — "either it moved, or the last one went away" — and a
    distinction nothing acts on is one more branch to keep true. A mutation
    collapsing the two was caught by nothing, which is what said so.
    """
    found = re.search(
        rf"^{re.escape(function)}\(\) \{{\n(.*?)^\}}", script, re.MULTILINE | re.DOTALL
    )
    return "" if found is None else found.group(1)


def arm(script: str, function: str) -> dict[tuple[str, str], str]:
    """`{(crate, example): echoed}` for one of the runner's two tables."""
    return {(crate, name): line for crate, name, line in ARM.findall(body(script, function))}


#: An example that reads its own command line at all.
#:
#: A property of the source, like `FOREVER` below, and for the same reason: an
#: example that starts taking arguments is caught by taking them rather than by
#: somebody remembering the roster exists.
#:
#: This was `sections::from_args` and that was the hole. Keying on the shared
#: parser meant an example that read `std::env::args()` by hand had no roster
#: line, no refusal run, and no rule anywhere that noticed — the state all
#: three headbench examples were in when `head_report --typo` exited 0. Two
#: `slate-slatedb` examples were still in it when this was widened.
#: Both spellings, because `sections::from_args` reads the command line inside
#: `slate-headbench` and the example that calls it never writes `env::args`
#: itself. The first draft of this pattern had only `std::env::args` and
#: reported all three headbench examples as rostered for nothing — the reverse
#: direction of rule 4 catching the widening's own bug, which is the best
#: available evidence that direction works.
READS_ARGS = re.compile(r"std::env::args|sections::from_args")

#: What refusing an argument the example does not understand looks like.
#:
#: Two spellings, because there are two: `sections::from_args` prints the
#: refusal and exits 2 itself, and a hand-written `match` with a catch-all arm
#: calls `std::process::exit` directly — which is what `s3_server` has always
#: done and what the two examples fixed alongside this now do.
#:
#: Deliberately not a check that the code exits **2** specifically. The number
#: matters and `run_examples.sh` asserts it against a built binary, which is a
#: stronger check than a regular expression over a literal; what this catches
#: is the example with no refusing path at all, which that run cannot catch
#: because such an example exits 0 and looks like a benchmark that worked.
REFUSES_UNKNOWN = re.compile(r"sections::from_args|process::exit")

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
    shakes = arm(script, "handshake")
    refusals = arm(script, "refuses")

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

    # The never-fires halves, and there are four because this reads three
    # different files with four different patterns. Each would match nothing
    # after an ordinary edit — a `case` rewritten as an `if`, a function
    # renamed, the manifest's members moved, `examples/` renamed — and leave
    # this printing `ok` over a tree it did not read.
    if not floors:
        said.append(
            f"no `<crate>) least=N ;;` lines in {runner.name}, so the floors "
            "below checked nothing. The `case` was rewritten; move this "
            "pattern with it."
        )
    if not shakes:
        said.append(
            f"no `<crate>/<example>) echo \"...\" ;;` lines in a `handshake()` "
            f"in {runner.name}. Either the handshake table moved, or the last "
            "server example went away — and if it went away, delete this guard "
            "deliberately."
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

    # Rule 4, both ways, plus the argument itself.
    #
    # `head_report --typo` printed its header, ran nothing and exited 0. The
    # fix — `slate_headbench::sections` — is unit-tested six ways, and the
    # `exit(2)` under it was reached by nothing until `refuses()` existed. An
    # example that gains sections and no refusal line goes back to the state
    # the defect was found in, silently, because a benchmark that has stopped
    # refusing still benchmarks.
    readers = {
        (crate, name)
        for crate, sources in found.items()
        for name, source in sources.items()
        if READS_ARGS.search(source)
    }
    # Rule 5, before rule 4's roster, because it is the one rule 4 rests on: an
    # example with no refusing path cannot pass a refusal run, and reporting
    # "no line in `refuses()`" for it would send the reader to the runner
    # rather than to the example.
    for crate, name in sorted(readers):
        if not REFUSES_UNKNOWN.search(found[crate][name]):
            said.append(
                f"`{crate}/{name}` reads its command line and has no path that "
                "refuses an argument it does not understand, so a mistyped one "
                "is ignored and it exits 0.\n"
                "  That is the `head_report --typo` defect: the caller gets a "
                "run they did not ask for and nothing says so. Match every "
                "argument and `std::process::exit(2)` on a catch-all arm, as "
                "`slate-slatedb/s3_server` does, or take the sections parser."
            )
    # A server's refusal line would never be reached: `run_examples.sh` moves
    # to the next example as soon as the handshake lands. Excluded from rule 4
    # in both directions rather than left to fail as "takes no arguments",
    # which would be a true message about the wrong thing.
    takers = readers - servers
    # Not in the early-return block above, unlike the other three never-fires
    # halves: an empty floor table makes every crate "missing a floor" and
    # buries the real message, while an empty refusal table cascades only into
    # rule 4's own forward loop — which this skips instead.
    if not refusals:
        said.append(
            f"no `<crate>/<example>) echo \"...\" ;;` lines in a `refuses()` "
            f"in {runner.name}, so nothing hands a built example a section "
            "name it does not have. That is the state `head_report --typo` was "
            "in when it printed a header, ran nothing and exited 0."
        )
        return said
    if not readers:
        said.append(
            "no example anywhere reads its command line, so the refusal table "
            "and rule 5 are both checking nothing. If `std::env::args` is "
            "reached some new way, teach `READS_ARGS` about it; if the last "
            "example that took an argument went away, delete the table and "
            "both rules together."
        )
    for crate, name in sorted(takers):
        if (crate, name) not in refusals:
            said.append(
                f"`{crate}/{name}` reads its command line and has no line in "
                "`refuses()`, so no run ever hands it an argument it does not "
                "understand.\n"
                f"  Add `{crate}/{name}) echo \"--not-an-argument\" ;;` — the "
                "refusal is the only thing standing between a mistyped "
                "argument and an empty report that exits 0."
            )
    for (crate, name), bad in sorted(refusals.items()):
        source = found.get(crate, {}).get(name)
        if source is None:
            said.append(
                f"`refuses()` names `{crate}/{name}`, which is not an example "
                "here any more. Delete the line."
            )
        elif (crate, name) in servers:
            said.append(
                f"`refuses()` names `{crate}/{name}`, which never returns and "
                "is run for its handshake instead. The refusal branch is not "
                "reached for it, so the line checks nothing. Delete it."
            )
        elif (crate, name) not in takers:
            said.append(
                f"`refuses()` names `{crate}/{name}`, which reads no "
                "arguments. It will exit 0 for any of them and the run will "
                "read that as a lost refusal. Delete the line."
            )
        elif f'"{bad}"' in source:
            # The failure this rule is for reads as the opposite of what it is.
            # A "bad" argument that the example's own roster lists makes it run
            # normally and exit 0 — and the runner reports `wanted 2`, which
            # sends the next reader to look for a refusal that is still there.
            said.append(
                f"`refuses()` hands `{crate}/{name}` the argument `{bad}`, "
                "which is a string in its own source and may well be a section "
                "or a flag it has.\n"
                "  Then it runs, exits 0, and the run reports the refusal as "
                "lost. Pick an argument no section could be named."
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
        f"every floor exact, {len(arm(script, 'handshake'))} handshake(s) "
        f"and {len(arm(script, 'refuses'))} refusal(s) rostered, "
        "every example that reads a command line refusing an argument it "
        "does not understand"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
