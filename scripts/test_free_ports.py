#!/usr/bin/env python3
"""`scripts/free_ports.py`, run rather than read — and the runners that call it.

    python3 scripts/test_free_ports.py

# What this is for

`ledger/2026-09-14-ports-below-the-ephemeral-range.md` records a CI failure and
then the gap this closes:

  > **Nothing tests the allocator itself.** There is no case asserting "every
  > port is below the ephemeral floor", so a future edit could move it back
  > into the range and only CI would notice, intermittently, which is where
  > this started.

"Only CI would notice, intermittently" is the worst shape a defect can have: the
demo job goes red on `EADDRINUSE` at a port number nobody recognises, once in
some number of runs, and it reads as the SDKs disagreeing rather than as the
harness handing out a port the kernel was about to reuse. It cost a CI failure
to diagnose the first time.

# Why it runs the allocator instead of reading it

The property under test — *every port it offers is below the ephemeral floor* —
is a property of the arithmetic, not of the text. A regex asserting that the
source still says `below - 1` would pass on

    candidate = random.randint(LOW, high)   # high was meant to be below - 1

and that is precisely the edit the caveat is afraid of. So the allocator is
called, with the floor as a parameter, and the ports it returns are checked
against the floor it was given.

The floor being a parameter is what lets it be *varied*. `free_ports.py` says
the range is read rather than assumed because "a container can be configured
with a different one, and picking below 32768 on a machine whose range starts
at 15000 would reintroduce exactly the bug" — a claim about a machine this
container is not, and therefore one nothing could have checked while the floor
came only from `/proc`.

# The copies, and why there is a case about the shell

There were two allocators: `examples/explorer/run.sh` offering five ports and
`examples/batchbench/run.sh` offering one, written from it. Both are now one
call to `scripts/free_ports.py`, which is what
`ledger/2026-09-29-one-allocator-instead-of-two.md` did and why.

That turns a property of the Python into a property of the Python *plus the
two call sites*, so `the runners call the allocator` is a case here: a runner
that grew its own copy back, or stopped calling this one, would leave every
arithmetic case below passing while the thing in production went unchecked.
It is the same shape as a guard pointed at a file nobody imports.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import free_ports as allocator

ROOT = Path(__file__).resolve().parent.parent

#: `run.sh` -> how many ports it asks for.
#:
#: The count is here rather than derived, because it is the thing a caller
#: depends on: `examples/explorer/run.sh` reads five names out of one line, and
#: an allocator that started offering four would leave the last empty.
CALLERS: dict[str, int] = {
    "examples/explorer/run.sh": 5,
    "examples/batchbench/run.sh": 1,
}

#: What a caller has to contain, and what it must not contain any more. The
#: second half is the one that matters: a runner that kept calling the script
#: *and* grew a heredoc back would satisfy the first half alone.
CALL = "scripts/free_ports.py"
COPY = "<<'PORTS'"


def case(name: str, ok: bool, detail: str = "") -> bool:
    print(f"{'ok  ' if ok else 'FAIL'}  {name}")
    if not ok and detail:
        print(f"        {detail}")
    return ok


def below_the_floor(wanted: int) -> list[bool]:
    passed = []

    # The floors: the Linux default, the one `free_ports.py`'s own comment
    # names as the reason it reads `/proc` at all, one just above `LOW` so the
    # clamp has almost nothing to work with, and the unreadable case.
    for floor, described in [
        (32768, "the Linux default"),
        (15000, "a container configured low, which is why /proc is read"),
        (10050, "a floor fifty above the allocator's own bottom"),
        (allocator.ASSUMED, "the value used when /proc cannot be read"),
    ]:
        try:
            ports = allocator.free(wanted, floor)
        # Blind, deliberately: a named failure line is worth more to whoever
        # broke it than a traceback.
        except Exception as problem:  # noqa: BLE001
            passed.append(case(f"{described}", False, f"raised {problem!r}"))
            continue
        passed.append(
            case(
                f"every port is below the floor, {described}",
                bool(ports) and all(allocator.LOW <= port < floor for port in ports),
                f"floor {floor}, got {ports}",
            )
        )
        passed.append(
            case(
                f"offers {wanted} distinct ports, {described}",
                len(ports) == wanted and len(set(ports)) == wanted,
                f"got {ports}",
            )
        )
    return passed


def reading(name: str, path: str, wanted: int) -> bool:
    """`floor()` against a chosen `RANGE`, reported rather than raised.

    The `except` is not defensive padding. A mutation narrowing `floor()`'s own
    `except (OSError, ValueError)` to `OSError` makes the garbled case below
    raise straight out of this file — which kills the run before it prints its
    tally, and `scripts/mutate.py` then reads `NOTHING RAN` rather than a
    catch. That is the second of the six lies its docstring lists, and it
    arrived here through a test with no guard of its own. Met once, fixed here.
    """
    was = allocator.RANGE
    try:
        allocator.RANGE = path
        got: object = allocator.floor()
    except Exception as problem:  # noqa: BLE001 - any raise is a failing case
        return case(name, False, f"raised {problem!r}")
    finally:
        allocator.RANGE = was
    return case(name, got == wanted, f"got {got}, wanted {wanted}")


def the_floor_is_read_rather_than_assumed() -> bool:
    """A readable range gives *its* floor, not the fallback.

    The case that was missing. Every arithmetic case above passes the floor in
    as a parameter, and the two fallback cases want `ASSUMED` — so a `floor()`
    rewritten to `return ASSUMED` and never open the file passed the whole
    suite. Found by mutation, and it is the exact defect `free_ports.py`'s own
    comment says reading the file prevents: on a machine whose range starts at
    15000, assuming 32768 hands out ports inside it.
    """
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as handle:
        handle.write("15000\t60999\n")
        written = handle.name
    try:
        return reading("a readable range gives its own floor, not the fallback", written, 15000)
    finally:
        Path(written).unlink()


def the_floor_falls_back_when_proc_is_unreadable() -> bool:
    """`floor()` answers `ASSUMED` when the file is not there.

    Separate from the arithmetic cases, because making the floor a parameter
    moved this branch out of them: they would all pass with the fallback
    returning nonsense, and a container without `/proc/sys/net` is exactly
    where the original bug came back.
    """
    return reading(
        "an unreadable range falls back to the assumed floor",
        "/proc/sys/net/ipv4/there-is-no-such-file",
        allocator.ASSUMED,
    )


def a_garbled_range_falls_back_too() -> bool:
    """And when the file is there and says something that is not a number.

    The `ValueError` arm. Written because the `except` names two exceptions and
    only one of them had a case, which is the half of a branch that rots.
    """
    return reading(
        "a range that is not a number falls back to the assumed floor",
        str(ROOT / "Cargo.toml"),
        allocator.ASSUMED,
    )


def the_top_port_offered_is_below_the_floor() -> bool:
    """The upper bound handed to the draw is `floor - 1`, exactly.

    Pinned rather than sampled, through `free`'s `draw` parameter. The draw is
    inclusive at both ends, so an upper bound of `floor` offers the first port
    inside the ephemeral range — which is the whole defect — and a random draw
    would show it about once in twenty thousand runs, which is to say never.
    """
    asked: list[tuple[int, int]] = []

    def the_top(low: int, high: int) -> int:
        asked.append((low, high))
        return high

    ports = allocator.free(1, 20000, draw=the_top)
    return case(
        "the highest port it can offer is one below the floor",
        ports == [19999] and asked == [(allocator.LOW, 19999)],
        f"offered {ports} after asking for {asked}",
    )


def a_repeated_draw_is_not_offered_twice() -> bool:
    """Two callers of one port is an `EADDRINUSE` in whichever binds second.

    Also pinned: with twenty thousand candidates, five draws collide rarely
    enough that dropping the check survives a random run.
    """
    queued = iter([15001, 15001, 15002])

    def in_turn(_low: int, _high: int) -> int:
        return next(queued)

    name = "a repeated draw is not offered twice"
    try:
        ports = allocator.free(2, 20000, draw=in_turn)
    except Exception as problem:  # noqa: BLE001 - running out of draws is a failure
        return case(name, False, f"raised {problem!r}")
    return case(name, ports == [15001, 15002], f"got {ports}, wanted [15001, 15002]")


def a_floor_at_or_below_the_bottom_is_loud() -> bool:
    """A machine whose ephemeral range starts at or below `LOW` gets an error.

    Not a passing allocation. `random.randint(10000, 8999)` raises, and that is
    the right answer for a machine where the allocator's assumption does not
    hold: there is no port below the floor and above `LOW` to offer, so the
    honest outcome is a stopped script rather than a port inside the range,
    which is the bug this whole mechanism exists to avoid.

    Asserted rather than left to chance, because "it happens to raise" and "it
    is designed to raise" look the same until somebody adds a `try` around it.
    """
    name = "a floor below the allocator's bottom raises rather than allocating"
    try:
        ports = allocator.free(1, 9000)
    except ValueError:
        return case(name, True)
    # Blind for the reason above, and narrowed by the `ValueError` arm before
    # it: anything else is a different failure and should say so by name.
    except Exception as problem:  # noqa: BLE001
        return case(name, False, f"raised {problem!r}, wanted ValueError")
    return case(name, False, f"returned {ports}, every one inside the ephemeral range")


def the_runners_call_the_allocator() -> list[bool]:
    """Both `run.sh` call this script, and neither carries a copy of it.

    The never-fires guard, in the shape this file needs after the dedup: every
    case above exercises `free_ports.py`, and all of them would pass while a
    runner quietly used its own heredoc again.
    """
    passed = []
    for path in CALLERS:
        text = (ROOT / path).read_text(encoding="utf-8")
        passed.append(
            case(f"{path} calls {CALL}", CALL in text, "it does not mention the script")
        )
        passed.append(
            case(
                f"{path} carries no allocator of its own",
                COPY not in text,
                f"it still has a {COPY} heredoc, which is the duplicate this removed",
            )
        )
    return passed


def main() -> int:
    passed = the_runners_call_the_allocator()
    for _, wanted in CALLERS.items():
        passed.extend(below_the_floor(wanted))
    passed.append(the_floor_is_read_rather_than_assumed())
    passed.append(the_floor_falls_back_when_proc_is_unreadable())
    passed.append(a_garbled_range_falls_back_too())
    passed.append(the_top_port_offered_is_below_the_floor())
    passed.append(a_repeated_draw_is_not_offered_twice())
    passed.append(a_floor_at_or_below_the_bottom_is_loud())
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
