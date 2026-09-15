"""Starting a real head node, and connecting to it.

Every test in this suite runs against the head node from `crates/slate-server`,
built with cargo and started as a subprocess. There is no mock and there is no
in-process fake, for the reason the task that produced this package gives and
for one more: a mock is a second statement of what the server does, written by
whoever wrote the client, and it therefore agrees with the client's
misunderstandings. The whole value of a first client is finding out where those
are.

# The handshake

The server prints `LISTENING <addr>` once its listener is bound and before it
starts serving. Waiting for that line rather than polling the port closes the
race where a connection arrives between `bind` and `accept` — which is rare
enough to pass a hundred runs and fail in CI.

# Which servers there are

Three, because three different arrangements are needed and none of them can be
reached by configuration after the fact:

- `server` — the ordinary one. Seeded, leader, no replicas, and *written to*.
- `oracle_server` — a second pristine one, whose seeded rows nothing changes.
  Separate because the oracle compares whole-table reads against answers
  computed at startup, and a write from any other test would make those
  comparisons depend on which file pytest ran first. That is the flakiness
  `docs/correctness.md` calls out as worse than a missing test.
- `stale_server` — the same, plus a replica that is empty and will never catch
  up. Freshness cannot be tested without one: with no lag, a token does nothing
  and a read-your-writes test passes against a client that ignores it.
- `follower_server` — a node that does not hold the lease, which is the only
  way to see the `UNAVAILABLE` + `slate-leader` redirect.

`server` is session-scoped and the tests that write to it use distinct keys.
The two others are session-scoped as well but used by few tests. A per-test
server would be cleaner and costs about 40 ms of process start each; it is
worth revisiting if isolation ever bites.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import threading
from collections.abc import Iterator
from typing import Any

import pytest

from slate import Client, Identity

HERE = pathlib.Path(__file__).resolve().parent
PYTHON_ROOT = HERE.parent
REPO_ROOT = PYTHON_ROOT.parents[1]
TESTSERVER = PYTHON_ROOT / "testserver"
#: Shared with the workspace so that a build here reuses the workspace's
#: compiled dependencies instead of making a second 20 GB copy of them.
TARGET_DIR = REPO_ROOT / "target"
BINARY = TARGET_DIR / "debug" / "slate-testserver"


def _build() -> pathlib.Path:
    """Build the test server once per session, or take one already built.

    `SLATE_TESTSERVER` names a prebuilt binary. Two reasons, and neither is
    speed: CI builds it once and hands the same binary to the suite rather than
    installing the Rust toolchain to rebuild it, and a contributor working only
    on this client can run the suite with no Rust installed.

    A path that is set and missing is a hard error rather than a fallback: the
    fallback would quietly test a different binary from the one the caller
    named. Set and missing also fails rather than *skipping*, which matters
    more than it looks — the `cargo`-missing branch below skips, and a skip is
    green. In CI that would mean a suite reporting success having started no
    server and exercised nothing, which is the failure this whole client is
    least able to notice.
    """
    named = os.environ.get("SLATE_TESTSERVER")
    if named:
        path = pathlib.Path(named)
        if not path.exists():
            raise RuntimeError(f"SLATE_TESTSERVER={named} does not exist")
        _refuse_if_stale(path, "SLATE_TESTSERVER")
        return path
    if shutil.which("cargo") is None:
        pytest.skip("cargo is not on PATH, so the head node cannot be built")
    environment = dict(os.environ, CARGO_TARGET_DIR=str(TARGET_DIR))
    result = subprocess.run(
        ["cargo", "build", "--quiet"],
        cwd=TESTSERVER,
        env=environment,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"building the head node failed:\n{result.stdout}\n{result.stderr}"
        )
    if not BINARY.exists():
        raise RuntimeError(f"cargo reported success but {BINARY} is not there")
    return BINARY


#: Directories whose contents decide what the server does.
#:
#: The `.proto` is in there because the protocol is the thing this client and
#: that binary have to agree about, and a stale binary speaking an older one is
#: exactly the failure this check exists for.
SERVER_SOURCES = (
    REPO_ROOT / "crates",
    REPO_ROOT / "clients" / "python" / "testserver",
)


def _newest_source() -> tuple[float, pathlib.Path] | None:
    """The most recently modified file the server is built from."""
    newest: tuple[float, pathlib.Path] | None = None
    for root in SERVER_SOURCES:
        if not root.exists():
            continue
        for path in root.rglob("*"):
            # `target/` is build output, not source, and walking it is slow
            # enough to notice: it is the biggest directory in the tree.
            if "target" in path.parts or not path.is_file():
                continue
            if path.suffix not in {".rs", ".toml", ".proto"}:
                continue
            stamp = path.stat().st_mtime
            if newest is None or stamp > newest[0]:
                newest = (stamp, path)
    return newest


def _refuse_if_stale(binary: pathlib.Path, variable: str) -> None:
    """Refuse a prebuilt binary older than the source it was built from.

    This is here because it happened. A full run of this suite reported 153
    passing tests against a `slate-testserver` built before that session's
    server changes — so every test of the new behaviour was checking the *old*
    server, and passing, because the client asked for something the old binary
    politely ignored. It was found by three new tests failing once the binary
    was rebuilt, which is luck rather than a process.

    Compared by modification time, which is crude and catches the whole of the
    real failure: a binary handed over by CI is minutes old, and one a
    contributor built last week is not.
    """
    newest = _newest_source()
    if newest is None:
        return
    stamp, source = newest
    if binary.stat().st_mtime >= stamp:
        return
    raise RuntimeError(
        f"{variable}={binary} was built before {source.relative_to(REPO_ROOT)} "
        f"was last changed, so the suite would test a server this tree did not "
        f"produce. Rebuild it, or unset {variable} to build from source."
    )


class Serving:
    """A running head node and the address it answers on."""

    def __init__(self, process: subprocess.Popen[str], address: str) -> None:
        self.process = process
        self.address = address
        self._drain: threading.Thread | None = None
        # Anything the server writes after the handshake goes nowhere useful,
        # but a full pipe buffer would block it mid-request. Drained on a
        # thread rather than closed, so a panic message survives to be printed
        # by `stop`.
        self.tail: list[str] = []
        self._drain = threading.Thread(target=self._pump, daemon=True)
        self._drain.start()

    def _pump(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            self.tail.append(line.rstrip("\n"))
            del self.tail[:-50]

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:  # pragma: no cover - a wedged server
                self.process.kill()
                self.process.wait(timeout=10)
        # The drain thread ends when the pipe reaches EOF, which the exit
        # above causes. Joined and then closed explicitly: `filterwarnings =
        # error` turns a leaked pipe into a suite failure, which is how this
        # was noticed rather than shipped.
        if self._drain is not None:
            self._drain.join(timeout=5)
        if self.process.stdout is not None:
            self.process.stdout.close()


def start(*args: str, tmp: pathlib.Path | None = None) -> Serving:
    """Start a head node with `args` and wait for it to be listening."""
    binary = _build()
    process = subprocess.Popen(
        [str(binary), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    assert process.stdout is not None
    banner: str | None = None
    for line in process.stdout:
        if line.startswith("LISTENING "):
            banner = line.split(" ", 1)[1].strip()
            break
    if banner is None:  # pragma: no cover - a server that failed to start
        process.wait(timeout=5)
        raise RuntimeError("the head node exited before it started listening")
    return Serving(process, banner)


APP = Identity("u64:1", tenant="u64:1", roles=["app"])
"""The identity the oracle runs as, and the one almost every test uses.

It must match `ctx()` in `testserver/src/main.rs`: the oracle's whole point is
that gRPC and the kernel answer identically, and they cannot if they are
running as different principals.
"""


@pytest.fixture(scope="session")
def oracle_path(tmp_path_factory: pytest.TempPathFactory) -> pathlib.Path:
    return tmp_path_factory.mktemp("oracle") / "answers.json"


@pytest.fixture(scope="session")
def server() -> Iterator[Serving]:
    """The ordinary head node: seeded, leader, no replicas. Tests write to it."""
    serving = start()
    try:
        yield serving
    finally:
        serving.stop()


@pytest.fixture(scope="session")
def oracle_server(oracle_path: pathlib.Path) -> Iterator[Serving]:
    """A head node nothing writes to, with the kernel's answers dumped.

    The oracle file is written before the socket opens, so a test that has a
    client cannot race it.
    """
    serving = start("--oracle-out", str(oracle_path))
    try:
        yield serving
    finally:
        serving.stop()


@pytest.fixture(scope="session")
def stale_server() -> Iterator[Serving]:
    """A head node whose pool holds a replica that is genuinely behind."""
    serving = start("--frozen-replica")
    try:
        yield serving
    finally:
        serving.stop()


@pytest.fixture(scope="session")
def follower_server() -> Iterator[Serving]:
    """A head node that does not hold the lease."""
    serving = start("--follower")
    try:
        yield serving
    finally:
        serving.stop()


def connect(serving: Serving, identity: Identity = APP, **kwargs: Any) -> Client:
    client = Client(serving.address, identity, **kwargs)
    client.wait_for_ready()
    return client


@pytest.fixture
def client(server: Serving) -> Iterator[Client]:
    with connect(server) as connected:
        yield connected


@pytest.fixture
def oracle_client(oracle_server: Serving) -> Iterator[Client]:
    with connect(oracle_server) as connected:
        yield connected


@pytest.fixture
def stale_client(stale_server: Serving) -> Iterator[Client]:
    with connect(stale_server) as connected:
        yield connected
