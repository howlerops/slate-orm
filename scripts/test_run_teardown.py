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
import contextlib
import os
import selectors
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / "examples" / "explorer" / "run.sh"

#: The mode to start the runner in, and the line it prints when ready.
#:
#: `--headless` is not a convenience. `run.sh` prints `adapters are up` in
#: **that mode only** — plain `run.sh` falls through to starting the frontend
#: in the foreground and never says the words, `--conformance` runs the suite
#: and exits, `--e2e` says something else. This test asked for the plain mode
#: and waited for the headless mode's line, so it could not have passed as
#: written, and the entry that added it recorded truthfully that it never had:
#: `ledger/2026-09-29-a-teardown-test-that-has-not-been-seen-to-pass.md`. Five
#: starts were spent on leftover ports before the sixth got far enough for
#: this to be the remaining reason.
#:
#: `--headless` is also the right mode on the merits: the frontend is a vite
#: dev server with a watcher, and what is under test is the runner reaping
#: four services rather than five.
MODE = "--headless"

#: The two things this file reports, named so a mutation run can say which
#: one a mutation broke. `STARTED` is the setup half — the runner got its four
#: services up — and `REAPED` is the property under test.
STARTED = "the runner starts its four services"
REAPED = "the runner reaps everything it started, on SIGTERM"
READY = "adapters are up"

#: How long to wait for that line. Generous: the runner builds the server, the
#: TypeScript client and a Go binary on a cold cache, and a timeout here is a
#: slow machine rather than a defect.
START_LIMIT = 600.0

#: The addresses `run.sh` binds when it is run plainly, and the variable that
#: overrides each.
#:
#: Fixed rather than drawn from the ephemeral range, which is the runner's
#: decision: it picks free ports only under `--conformance` and `--e2e`, so
#: that a person opening the demo finds it where the README says. The cost is
#: that a previous run which did not die cleanly holds the port, and the
#: symptom is a service failing to bind inside a log file this test never read.
#: Five starts were spent on that today.
ADDRESSES = {
    "the head node": ("SLATE_HEAD_ADDR", 7421),
    "the Go adapter": ("SLATE_GO_ADDR", 7431),
    "the Node adapter": ("SLATE_NODE_ADDR", 7432),
    "the Python adapter": ("SLATE_PY_ADDR", 7433),
}


def port_of(variable: str, fallback: int) -> int:
    """The port `run.sh` will use for this service."""
    address = os.environ.get(variable)
    return int(address.rsplit(":", 1)[-1]) if address else fallback


#: What `run.sh --headless` prints before it says it is ready.
#:
#: The path is read from the runner rather than derived here, and that is the
#: whole point: this file used to compute `$TMPDIR/slate-explorer-<port>`
#: itself, a second copy of an expression in `run.sh`, and a change to that
#: convention would have broken the log tails *silently* — they are printed
#: only to explain a failure, so a wrong path means no explanation rather than
#: an error. `run.sh` now prints its own.
LOGS = "logs "


def held() -> list[str]:
    """Which of the runner's ports something is already listening on.

    Checked *before* starting rather than diagnosed afterwards, because the
    two are not equally legible: a bind failure reaches a log file the runner
    never prints, and what this test reported was "the runner never printed
    'adapters are up'; it exited 1" — true, useless, and identical to a
    missing toolchain or a full disk.

    `connect`, not `bind`. Binding to test a port is the race the runner's own
    port allocator has a comment about: the probe frees it and something else
    can take it before the real listener starts. A successful connect says
    somebody is listening now, which is the thing worth refusing to start on.
    """
    busy = []
    for what, (variable, fallback) in ADDRESSES.items():
        port = port_of(variable, fallback)
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.settimeout(0.5)
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                busy.append(f"{port} ({what})")
    return busy

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


def fail(name: str, detail: str = "") -> int:
    """Report a failure the way every other `scripts/test_*.py` here does.

    The `FAIL  <name>` line goes to **stdout** and the detail to stderr, which
    is not a style choice: `scripts/mutate.py`'s `python` dialect names the
    failing test by matching `^FAIL\\s+(.+?)$`, and without one a mutation that
    this test correctly caught scored `UNREADABLE — the command exited 1,
    something reported, and no failing test was named`. A suite that fails
    without naming what failed is a suite a mutation run cannot use, which is
    the whole reason this file exists in a repository that mutation-tests
    everything.
    """
    print(f"FAIL  {name}")
    if detail:
        print(detail, file=sys.stderr)
    return 1


def main() -> int:
    if not RUNNER.exists():
        return fail(STARTED, f"{RUNNER} does not exist")

    busy = held()
    if busy:
        return fail(
            STARTED,
            "port " + ", ".join(busy) + " already has a listener, so `run.sh` "
            "cannot bind it and this test would report a start that never "
            "happened.\n"
            "  Almost always a previous run of this test or of `run.sh` that "
            "was killed rather than stopped: the runner's trap reaps its "
            "group, and a `SIGKILL` to the runner does not run the trap.",
        )

    started = subprocess.Popen(
        ["bash", str(RUNNER), MODE],
        cwd=RUNNER.parent,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        # Bytes, unbuffered, and split into lines below by hand.
        #
        # `text=True, bufsize=1` with a `select` loop loses wake-ups, and this
        # cost a diagnosis: `select` watches the file descriptor, Python's
        # `TextIOWrapper` buffers *above* it, so once a chunk has been slurped
        # into that buffer the descriptor reads as empty and the loop sleeps
        # with three of the runner's lines already in hand. The banner and
        # `adapters are up` arrive in one burst, so exactly the line being
        # waited for was the one left in the buffer — a start that had
        # succeeded reported as a six-hundred-second timeout, twice.
        #
        # That was self-inflicted: plain `readline()` had no such problem, and
        # it appeared with the `select` loop that fixed the hang. Owning both
        # halves means reading the descriptor the selector watches.
        bufsize=0,
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
        run_dir: Path | None = None
        # `select` rather than a bare `readline()`, and that is not a
        # refinement. `readline()` blocks, so the deadline above was only
        # consulted *between lines* — a runner that goes quiet was waited on
        # for ever and `START_LIMIT` bounded nothing. It happened: `run.sh`'s
        # Go adapter could not bind a port left held by an earlier run, the
        # script sat in its own wait loop printing nothing, and this test was
        # still sitting in `readline()` fifteen minutes past a six hundred
        # second limit. "A hang is not a failure" is a class this repository
        # has met before — `ledger/2026-09-21-a-hang-is-now-a-named-failure.md` —
        # and a test written to catch a runner that does not stop is a poor
        # place to meet it again.
        watch = selectors.DefaultSelector()
        watch.register(started.stdout, selectors.EVENT_READ)
        rest = b""
        try:
            while not ready:
                left = deadline - time.monotonic()
                if left <= 0:
                    break
                if not watch.select(timeout=min(left, 5.0)):
                    continue
                chunk = started.stdout.read(65536)
                if not chunk:
                    break
                rest += chunk
                # Every *complete* line in what arrived, keeping any partial
                # tail for the next chunk. A burst can carry several lines and
                # the one being waited for is as likely to be the third as the
                # first.
                while b"\n" in rest:
                    one, rest = rest.split(b"\n", 1)
                    text = one.decode("utf-8", "replace").rstrip()
                    said.append(text)
                    if text.startswith(LOGS):
                        run_dir = Path(text.removeprefix(LOGS).strip())
                    if READY in text:
                        ready = True
                        break
        finally:
            watch.close()
        if not ready:
            print(f"FAIL  {STARTED}")
            print(
                f"the runner never printed {READY!r}; it exited {started.poll()}"
                if started.poll() is not None
                else f"the runner never printed {READY!r} within {START_LIMIT}s",
                file=sys.stderr,
            )
            for one in said:
                print(f"      {one}", file=sys.stderr)
            # And the per-service logs, because `run.sh` sends each service's
            # output to a file and only its own progress to stdout. Both times
            # this failed, the cause was one line in `go.log` — a port held by
            # an earlier run — and nothing on stdout said so.
            if run_dir is None:
                print(
                    f"      the runner never printed a {LOGS!r} line, so the "
                    "per-service logs cannot be found",
                    file=sys.stderr,
                )
            for log in sorted(run_dir.glob("*.log")) if run_dir and run_dir.is_dir() else ():
                body = log.read_text(encoding="utf-8", errors="replace").strip()
                if body:
                    print(f"      --- {log.name}", file=sys.stderr)
                    for one in body.splitlines()[-TAIL:]:
                        print(f"      {one}", file=sys.stderr)
            return 1

        # Recorded while the tree is intact. See the docstring: a survivor is
        # reparented and unfindable afterwards.
        tree = descendants(started.pid)
        if not tree:
            # The never-fires case. With four services up there are always
            # children; an empty set means the walk is broken and every
            # assertion below would pass vacuously.
            return fail(
                REAPED,
                f"no descendants found under the runner (pid {started.pid}), "
                "which cannot be right with four services up — the /proc walk "
                "has stopped working.",
            )
        print(f"      {len(tree)} processes under the runner while it serves")

        started.send_signal(signal.SIGTERM)
        try:
            started.wait(timeout=STOP_LIMIT)
        except subprocess.TimeoutExpired:
            return fail(
                REAPED, f"the runner did not exit within {STOP_LIMIT}s of SIGTERM"
            )

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
            return fail(
                REAPED,
                "the runner exited leaving " + ", ".join(named) + " alive.\n"
                "  That is the leak `run.sh`'s `set -m` and `kill -- -$pid` "
                "exist to prevent: a service started as `( ... ) &` spawns a "
                "child, and killing the recorded pid leaves the grandchild.",
            )

        print(f"ok    {REAPED}  ({len(tree)} processes)")
        return 0
    finally:
        # `SIGTERM` to the runner first, and only then the group.
        #
        # It was `killpg(getpgid(started.pid), SIGKILL)` alone, and that is
        # the leak this whole file is about, inside the file itself. `run.sh`
        # sets `-m`, so **each service is its own process group** — killing
        # the runner's group reaps the runner and nothing else, and every
        # failed start left four listeners behind. That is not a hypothesis:
        # it is why five of the first six runs died on `address already in
        # use`, each poisoned by the one before, and why the port pre-flight
        # above exists at all. The test that catches a runner leaking its
        # children was leaking them.
        #
        # A `SIGTERM` runs `run.sh`'s own trap, which is the mechanism under
        # test and the only thing that knows the four group ids. The group
        # kill stays as the backstop for a runner that ignores it.
        if started.poll() is None:
            started.send_signal(signal.SIGTERM)
            with contextlib.suppress(subprocess.TimeoutExpired):
                started.wait(timeout=STOP_LIMIT)
        if started.poll() is None:
            os.killpg(os.getpgid(started.pid), signal.SIGKILL)
            started.wait(timeout=10)
        if started.stdout is not None:
            started.stdout.close()

        # And say so if anything is still listening, because a silent leak
        # here is what cost a day: the next run fails to bind and blames
        # itself.
        left = held()
        if left:
            print(
                "after cleanup, port " + ", ".join(left) + " still has a "
                "listener. The runner did not reap its services and this "
                "process could not either; the next run will refuse to start.",
                file=sys.stderr,
            )


def reported() -> int:
    """`main`, plus the one-line summary every other `scripts/test_*.py` prints.

    Not cosmetic. `scripts/mutate.py`'s `python` dialect reads
    a `N passed, M failed` line as proof that a suite reported at all, and
    without it a mutation aimed at `run.sh`'s teardown scores as **NOTHING
    RAN** rather than as caught — the second of the six lies that script's
    docstring lists. `test_check_sh.py` and `test_codegen.py` were both found
    to have exactly this hole, the same way.
    """
    failed = main()
    print(f"\n{0 if failed else 1} passed, {1 if failed else 0} failed")
    return failed


if __name__ == "__main__":
    sys.exit(reported())
