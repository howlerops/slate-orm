#!/usr/bin/env python3
"""The explorer's runner leaves nothing behind when it is told to stop.

`ledger/2026-09-14-the-demo-runner-left-processes-behind.md` fixed a real leak:
`go run` compiles and then runs the built binary as a child, `npm start` spawns
node as a child, so killing the recorded pid left grandchildren alive. CI's own
cleanup reported three orphans after the demo job. The fix is `set -m` plus
`kill -- -$pid`, which reaches the whole group. The entry left this:

    Nothing tests the teardown. There is no case that starts the stack, kills
    it and asserts nothing survives, so a future edit could reintroduce this
    and only a CI cleanup line would notice. That check is cheap and is not
    written.

This is that case. It starts the real stack, waits until all four services are
up, records every descendant, sends **`SIGTERM` to `run.sh` itself** — not to
the group, because the script's own trap is the thing under test — and then
asserts nothing it recorded is still alive.

**The recording has to happen before the kill**, and that is the whole design.
Asking afterwards which processes are gone is easy and proves nothing: a
grandchild that outlives its parent is reparented to init, so it is no longer
a descendant of anything this script can walk. The pids are taken while the
tree is intact and checked individually after.

**NOT YET VERIFIED, AND NOT WIRED INTO ANYTHING.** Written on 2026-09-29 and
never observed passing: the container it was written on ran out of disk while
`run.sh` was still building, so the start never reached `adapters are up` in
600 seconds. It needs a built `slate-serverd` and the adapters' toolchains,
which is why it does not belong in `scripts/check.sh`; CI's conformance job,
which already pays for two `run.sh` invocations, is the place for it — *after*
somebody has seen it pass and seen it fail. Adding an unrun check to CI is the
shape `CLAUDE.md` warns about under "a check that never fires is a check
nobody has debugged", and that warning applies to this file today.

A manual probe on the same wedged container did produce survivors after a
`SIGTERM` to `run.sh`, which is what the leak looks like. It is recorded as a
**hypothesis and not a finding**, because the observation is contaminated:
a `pkill` had already delivered one `TERM` moments earlier, the disk was full,
and `go run` and `npm start` may still have been compiling. Reproducing it on
a healthy machine is the next step and is what this file is for.

Run directly: `python3 scripts/test_run_teardown.py`.
"""

from __future__ import annotations

import collections
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / "examples" / "explorer" / "run.sh"

#: The line `run.sh` prints once every service is listening.
READY = "adapters are up"

#: How long to wait for that line. Generous: the runner builds the server, the
#: TypeScript client and a Go binary on a cold cache, and a timeout here is a
#: slow machine rather than a defect.
START_LIMIT = 600.0

#: How many of the runner's last lines to print when it fails to start.
#: Twenty is enough for the head node's log, which `run.sh` dumps inline when
#: the node will not bind.
TAIL = 20

#: How long to wait after SIGTERM. `run.sh`'s own grace period is ten 0.2s
#: polls before it escalates to KILL, so anything past a few seconds is the
#: script failing to reap rather than a service taking its time.
STOP_LIMIT = 30.0


def alive(pid: int) -> bool:
    """Is this pid a live process?

    `/proc` rather than `os.kill(pid, 0)`: the runner's children are not this
    process's children, so a signal probe would answer about permissions as
    well as existence. A zombie reads as alive here and that is correct — it
    has not been reaped.
    """
    return Path(f"/proc/{pid}").exists()


def descendants(root: int) -> set[int]:
    """Every process under `root`, by walking `/proc/*/stat`'s parent field."""
    parents: dict[int, int] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
        except OSError:
            continue  # exited between the listing and the read
        # The comm field is parenthesised and may itself contain spaces and
        # parentheses -- `(go run ...)`. Split after the *last* `)`.
        tail = stat.rsplit(")", 1)[-1].split()
        if len(tail) < 2:
            continue
        parents[int(entry.name)] = int(tail[1])

    found: set[int] = set()
    changed = True
    while changed:
        changed = False
        for pid, parent in parents.items():
            if pid not in found and (parent == root or parent in found):
                found.add(pid)
                changed = True
    return found


def main() -> int:
    if not RUNNER.exists():
        print(f"{RUNNER} does not exist", file=sys.stderr)
        return 1

    started = subprocess.Popen(
        ["bash", str(RUNNER)],
        cwd=RUNNER.parent,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        # Its own session, so the cleanup below can reap the whole tree if this
        # test fails partway and `run.sh`'s trap never runs.
        start_new_session=True,
    )
    assert started.stdout is not None

    try:
        deadline = time.monotonic() + START_LIMIT
        ready = False
        # The runner's last lines, kept for the failure message. Without them
        # the only output on a failed start is "it exited 1", and the four
        # things that actually go wrong here — a port still held by a previous
        # run, a missing toolchain, a build that ran out of disk, a service
        # that died on startup — are indistinguishable from each other and
        # from a slow machine. Three runs were spent re-running `run.sh` by
        # hand to read a message this loop had already consumed.
        said: collections.deque[str] = collections.deque(maxlen=TAIL)
        while time.monotonic() < deadline:
            line = started.stdout.readline()
            if not line:
                break
            said.append(line.rstrip())
            if READY in line:
                ready = True
                break
        if not ready:
            print(
                f"the runner never printed {READY!r}; it exited {started.poll()}"
                if started.poll() is not None
                else f"the runner never printed {READY!r} within {START_LIMIT}s",
                file=sys.stderr,
            )
            for one in said:
                print(f"      {one}", file=sys.stderr)
            return 1

        # Recorded while the tree is intact. See the docstring: a survivor is
        # reparented and unfindable afterwards.
        tree = descendants(started.pid)
        if not tree:
            # The never-fires case. With four services up there are always
            # children; an empty set means the walk is broken and every
            # assertion below would pass vacuously.
            print(
                f"no descendants found under the runner (pid {started.pid}), "
                "which cannot be right with four services up — the /proc walk "
                "has stopped working.",
                file=sys.stderr,
            )
            return 1
        print(f"      {len(tree)} processes under the runner while it serves")

        started.send_signal(signal.SIGTERM)
        try:
            started.wait(timeout=STOP_LIMIT)
        except subprocess.TimeoutExpired:
            print(f"the runner did not exit within {STOP_LIMIT}s of SIGTERM", file=sys.stderr)
            return 1

        # A moment for the KILL escalation to land: `run.sh` sends it and
        # returns, and the kernel reaps asynchronously.
        deadline = time.monotonic() + 5.0
        survivors = {pid for pid in tree if alive(pid)}
        while survivors and time.monotonic() < deadline:
            time.sleep(0.2)
            survivors = {pid for pid in tree if alive(pid)}

        if survivors:
            named = []
            for pid in sorted(survivors):
                try:
                    named.append(f"{pid} ({(Path(f'/proc/{pid}/comm')).read_text().strip()})")
                except OSError:
                    named.append(str(pid))
            print(
                "the runner exited leaving " + ", ".join(named) + " alive.\n"
                "  That is the leak `run.sh`'s `set -m` and `kill -- -$pid` "
                "exist to prevent: a service started as `( ... ) &` spawns a "
                "child, and killing the recorded pid leaves the grandchild.",
                file=sys.stderr,
            )
            return 1

        print(f"ok    the runner reaped all {len(tree)} of them on SIGTERM")
        return 0
    finally:
        if started.poll() is None:
            with_group = os.getpgid(started.pid)
            os.killpg(with_group, signal.SIGKILL)
            started.wait(timeout=10)
        if started.stdout is not None:
            started.stdout.close()


if __name__ == "__main__":
    sys.exit(main())
