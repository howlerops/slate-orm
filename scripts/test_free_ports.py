#!/usr/bin/env python3
"""The port allocators in `run.sh`, run rather than read.

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

The allocator is eleven lines of Python inside a shell heredoc, and the
property under test — *every port it offers is below the ephemeral floor* — is
a property of the arithmetic, not of the text. A regex asserting that the
source still says `ephemeral_low - 1` would pass on

    candidate = random.randint(low, high)   # high was meant to be ephemeral_low - 1

and that is precisely the edit the caveat is afraid of. So the heredoc is
extracted and executed, with `open` patched to serve a chosen range, and the
ports it prints are checked against the floor it was given.

Patching `open` rather than writing a fake `/proc` file is what lets the floor
be *varied*. The comment inside the allocator says the range is read rather
than assumed because "a container can be configured with a different one, and
picking below 32768 on a machine whose range starts at 15000 would reintroduce
exactly the bug" — a claim about a machine this container is not, and therefore
one nothing could have checked until the floor became a parameter.

# The two copies

`examples/explorer/run.sh` has the original, offering five ports; the
`examples/batchbench/run.sh` copy offers one and was written from it. Both are
checked, because the reason the caveat gives — a future edit — applies to
whichever copy somebody edits, and a guard that covered one would make the
other the dangerous one.

That there are two copies at all is a finding of this file and not something it
fixes: see the ledger entry.
"""

from __future__ import annotations

import io
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: `run.sh` -> how many ports its allocator offers.
#:
#: The count is here rather than derived, because it is the thing a caller
#: depends on: `examples/explorer/run.sh` reads five names out of one line, and
#: an allocator that started offering four would leave the last empty.
ALLOCATORS: dict[str, int] = {
    "examples/explorer/run.sh": 5,
    "examples/batchbench/run.sh": 1,
}

HEREDOC = re.compile(r"python3 - <<'PORTS'\n(.*?)\nPORTS", re.DOTALL)


def allocator(path: Path) -> str:
    """The Python inside the `PORTS` heredoc, or a failure that says which file.

    A `RuntimeError` rather than a returned `None`: a run.sh whose allocator
    this cannot find is the never-fires case, and the whole point of the file
    is that a silent zero here reads exactly like a passing check.
    """
    found = HEREDOC.findall(path.read_text(encoding="utf-8"))
    if len(found) != 1:
        raise RuntimeError(f"{path}: expected one PORTS heredoc, found {len(found)}")
    return found[0]


def run(source: str, floor: int | None) -> list[int]:
    """Execute the allocator with `/proc` reporting `floor`, and return its ports.

    `floor` of `None` makes the read raise, which is the branch the allocator's
    `except (OSError, ValueError)` handles and its fallback of 32768 covers.
    """
    printed: list[str] = []

    def fake_open(*_args: object, **_kwargs: object) -> io.StringIO:
        if floor is None:
            raise OSError("no /proc here")
        # The real file is "<low> <high>\n" and the allocator splits and takes
        # the first field, so the second number has to be there to be ignored.
        return io.StringIO(f"{floor} 60999\n")

    namespace: dict[str, object] = {
        "__builtins__": __builtins__,
        "open": fake_open,
        "print": lambda *parts: printed.append(" ".join(str(part) for part in parts)),
    }
    exec(compile(source, "<allocator>", "exec"), namespace)
    return [int(word) for line in printed for word in line.split()]


def case(name: str, ok: bool, detail: str = "") -> bool:
    print(f"{'ok  ' if ok else 'FAIL'}  {name}")
    if not ok and detail:
        print(f"        {detail}")
    return ok


def below_the_floor(path: str, wanted: int) -> list[bool]:
    source = allocator(ROOT / path)
    passed = []

    # The floors: the Linux default, the one the allocator's own comment names
    # as the reason it reads `/proc` at all, one just above the allocator's
    # `low` so that its `max(..., 10100)` and its `min(..., ephemeral_low - 1)`
    # have to disagree, and the unreadable case.
    for floor, described in [
        (32768, "the Linux default"),
        (15000, "a container configured low, which is why /proc is read"),
        (10050, "a floor inside the allocator's own 10100 ceiling"),
        (None, "/proc unreadable, so the 32768 fallback"),
    ]:
        effective = 32768 if floor is None else floor
        try:
            ports = run(source, floor)
        # Blind, deliberately: the subject is arbitrary code extracted from a
        # shell script, and a named failure line is worth more to whoever broke
        # it than a traceback out of `exec`.
        except Exception as problem:  # noqa: BLE001
            passed.append(case(f"{path}: {described}", False, f"raised {problem!r}"))
            continue
        passed.append(
            case(
                f"{path}: every port is below the floor, {described}",
                bool(ports) and all(10000 <= port < effective for port in ports),
                f"floor {effective}, got {ports}",
            )
        )
        passed.append(
            case(
                f"{path}: offers {wanted} distinct ports, {described}",
                len(ports) == wanted and len(set(ports)) == wanted,
                f"got {ports}",
            )
        )
    return passed


def a_floor_at_or_below_the_bottom_is_loud() -> bool:
    """A machine whose ephemeral range starts at or below 10000 gets an error.

    Not a passing allocation. `random.randint(10000, 9999)` raises, and that is
    the right answer for a machine where the allocator's assumption does not
    hold: there is no port below the floor and above 10000 to offer, so the
    honest outcome is a stopped script rather than a port inside the range,
    which is the bug this whole mechanism exists to avoid.

    Asserted rather than left to chance, because "it happens to raise" and "it
    is designed to raise" look the same until somebody adds a `try` around it.
    """
    source = allocator(ROOT / "examples/explorer/run.sh")
    try:
        ports = run(source, 9000)
    except ValueError:
        return case("a floor below the allocator's bottom raises rather than allocating", True)
    # Blind for the reason above, and narrowed by the `ValueError` arm before
    # it: anything else is a different failure and should say so by name.
    except Exception as problem:  # noqa: BLE001
        return case(
            "a floor below the allocator's bottom raises rather than allocating",
            False,
            f"raised {problem!r}, wanted ValueError",
        )
    return case(
        "a floor below the allocator's bottom raises rather than allocating",
        False,
        f"returned {ports}, every one of them inside the ephemeral range",
    )


def the_heredoc_is_found() -> bool:
    """The never-fires guard: a renamed heredoc marker finds nothing.

    Every case above starts with `allocator()`, so a `run.sh` whose marker
    changed from `PORTS` would make them all raise rather than all pass — but
    only if something asserts the extraction worked at all, and a reader
    checking this file should see that assertion rather than infer it.
    """
    ok = True
    for path in ALLOCATORS:
        try:
            body = allocator(ROOT / path)
        except (OSError, RuntimeError) as problem:
            ok = case(f"{path}: the allocator is where this expects it", False, str(problem)) and ok
            continue
        ok = case(
            f"{path}: the allocator is where this expects it",
            "ip_local_port_range" in body and "randint" in body,
            "found a heredoc, but it does not read the range or draw a port",
        ) and ok
    return ok


def main() -> int:
    passed = [the_heredoc_is_found()]
    for path, wanted in ALLOCATORS.items():
        passed.extend(below_the_floor(path, wanted))
    passed.append(a_floor_at_or_below_the_bottom_is_loud())
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
