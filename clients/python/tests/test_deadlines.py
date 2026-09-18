"""A per-call deadline, and the view that carries one.

Before this the client passed no `timeout` on any RPC. A head node that accepted
a connection and then stopped answering blocked the caller **for ever** — the
failure mode a deadline exists for, and the one no amount of error-code
classification helps with, because no error ever arrives.

`a_call_with_no_deadline_does_not_return` is the half that makes the rest mean
something: it demonstrates the hang, against a listener that accepts and says
nothing, rather than asserting only that the fixed version works.

The deadline is per *call*, not per connection, because a point get and a
hundred-thousand-row scan do not want the same number — and a gRPC deadline
covers the whole stream rather than each message, so a long scan genuinely
needs a longer one.
"""

from __future__ import annotations

import socket
import threading
import time
from collections.abc import Iterator

import pytest

from slate import Client, DeadlineExceeded, Query, i64, u64

from .fixture import DOCS

#: Long enough that a loopback round trip could never explain the failure, short
#: enough that the suite does not notice. Every deadline test uses it so the
#: whole file costs about a second.
GRACE = 1.0


@pytest.fixture(scope="module")
def silent_port() -> Iterator[int]:
    """A listener that accepts connections and never speaks.

    Not a slow server and not a closed port: either of those produces a
    different error. This is the one shape where the client's own deadline is
    the only thing that can end the call — gRPC completes the TCP connect, then
    waits for a server preface that never comes.
    """
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(16)

    def accept_forever() -> None:
        # The accepted sockets are owned here rather than by the fixture, so
        # that one accepted *after* teardown began still gets closed. Closing
        # them from the other thread left a race whose only symptom was a
        # ResourceWarning attributed to whichever test ran next.
        held: list[socket.socket] = []
        try:
            while True:
                try:
                    conn, _ = listener.accept()
                except OSError:
                    return
                # Kept open, so the kernel does not close it and hand gRPC a
                # reset — which would be `Unavailable`, not a deadline.
                held.append(conn)
        finally:
            for conn in held:
                conn.close()

    thread = threading.Thread(target=accept_forever, daemon=True)
    thread.start()
    try:
        yield listener.getsockname()[1]
    finally:
        # Closing the listener is what unblocks `accept`, so the order matters
        # and the join is what makes the teardown deterministic.
        listener.close()
        thread.join(timeout=5)


def test_a_silent_server_is_a_deadline_rather_than_a_hang(silent_port: int) -> None:
    with Client(f"127.0.0.1:{silent_port}", timeout=GRACE) as client:
        started = time.monotonic()
        with pytest.raises(DeadlineExceeded):
            client.get(DOCS, [1])
        elapsed = time.monotonic() - started
    # Bounded both ways. Too fast would mean something else failed the call and
    # the deadline proved nothing; too slow would mean the number is not the one
    # being honoured.
    assert GRACE <= elapsed < GRACE + 5, elapsed


def test_the_deadline_reaches_a_streaming_call_too(silent_port: int) -> None:
    """`query` streams and `get` does not, and they are separate lines in
    `_Ops` — one of them carrying the timeout is not both."""
    with (
        Client(f"127.0.0.1:{silent_port}", timeout=GRACE) as client,
        pytest.raises(DeadlineExceeded),
    ):
        list(client.query(Query(DOCS)))


def test_a_call_with_no_deadline_does_not_return(silent_port: int) -> None:
    """The hang this feature exists to end, demonstrated.

    Run on a daemon thread and left running: there is no way to cancel it, which
    is the point. If a future default gave every call a deadline, this test
    would fail and should be deleted along with the line that made it wrong.
    """
    client = Client(f"127.0.0.1:{silent_port}")
    returned = threading.Event()

    def call() -> None:
        try:
            client.get(DOCS, [1])
        except BaseException:
            # Swallowed, and it matters. The thread outlives the test: when the
            # module fixture closes the listener, the blocked call finally
            # fails, and an exception escaping a thread pytest is watching is
            # reported against whatever test is running *then*. What this test
            # asserts is that nothing came back within the window; how the call
            # ends afterwards is not its business.
            pass
        finally:
            returned.set()

    threading.Thread(target=call, daemon=True).start()
    assert not returned.wait(GRACE * 2), "a call with no deadline came back"


# --- the view -------------------------------------------------------------


def test_a_view_does_not_change_the_session_it_came_from(
    client: Client, silent_port: int
) -> None:
    """The reason it is a new object rather than a setting: a short deadline
    left switched on by a caller who forgot to restore it is a bug this shape
    cannot have."""
    view = client.with_timeout(GRACE)
    assert view._timeout == GRACE
    assert client._timeout is None
    # And the original still reaches the real server, which a mutation would
    # not have changed — this is about the attribute, and the next test is
    # about the behaviour.
    client.get(DOCS, [1])


def test_a_view_shares_the_freshness_scope_it_came_from(client: Client) -> None:
    """Write through one view, read through the other, see the write.

    Copying the watermark instead of sharing it would leave these two unable to
    see each other's writes — a monotonic-reads violation invisible to every
    other test in this suite, because every other test uses one session.
    """
    view = client.with_timeout(30)
    view.insert(DOCS, [(u64(9_100), "deadline", i64(1), "written through a view")])

    assert client.watermark is not None, "the shared watermark did not move"
    row = client.get(DOCS, [u64(9_100)])
    assert row is not None
    assert str(row[1]) == "deadline"


def test_a_transaction_inherits_its_sessions_deadline(client: Client) -> None:
    """A deadline set on the session and dropped at `begin` would leave exactly
    the calls a busy process makes most without one."""
    view = client.with_timeout(30)
    with view.transaction() as txn:
        assert txn._timeout == 30
        txn.insert(DOCS, [(u64(9_101), "deadline", i64(1), "inside a transaction")])
    # And a session that has none does not invent one.
    with client.transaction() as txn:
        assert txn._timeout is None
        txn.rollback()


def test_a_session_defaults_to_the_clients_deadline(silent_port: int) -> None:
    """`session()` inherits, and `timeout=None` is not how you opt out.

    `None` is the sentinel for "unset", so an explicit `None` means the client's
    value — stated here rather than left to be discovered, because the opposite
    reading is the natural one.
    """
    with Client(f"127.0.0.1:{silent_port}", timeout=GRACE) as client:
        assert client._timeout == GRACE
        assert client.session()._timeout == GRACE
        assert client.session(timeout=7)._timeout == 7
        # To genuinely opt out, take a view of it.
        assert client.with_timeout(None)._timeout is None


def test_no_deadline_is_the_default(client: Client) -> None:
    """Unchanged, deliberately: adding one silently would turn a slow query into
    a failure in every caller that upgraded without asking for it."""
    assert client._timeout is None
