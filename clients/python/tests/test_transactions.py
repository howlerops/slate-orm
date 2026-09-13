"""Transactions: the context manager, rollback, and a conflict that is real.

# The conflict test contends by construction

A lost-update test that starts two threads and hopes they overlap passes
because they did not. The repository's own history records fixing exactly that
("Make the lost-update test contend by construction, not by luck"), so the test
below uses a barrier: both transactions are open and have both read the same
row before either commits. One then commits, the other is refused with
`ABORTED`, and `transact` runs its body again — which is the whole reason
`transact` takes a function rather than being a context manager.

The assertion is on the *final value*, not on the number of retries. A retry
count says the machinery ran; a final value of start+2 says no update was lost,
which is the property.
"""

from __future__ import annotations

import contextlib
import threading

import grpc
import pytest

from slate import (
    Client,
    Conflict,
    Freshness,
    NotFound,
    Query,
    Transaction,
    i64,
    u64,
)
from slate._proto.slate.v1 import records_pb2 as pb
from slate._proto.slate.v1 import records_pb2_grpc as pb_grpc

from .conftest import APP, Serving, connect
from .fixture import DOCS


def test_a_transaction_commits_on_a_clean_exit(client: Client) -> None:
    with client.transaction() as tx:
        tx.insert(DOCS, [(u64(200), "kind-t", i64(1), None)])
        # No sequence yet: a write inside a transaction has none until the
        # transaction commits.
        assert tx.sequence is None
    assert tx.sequence is not None
    assert client.get(DOCS, [u64(200)]) is not None


def test_a_transaction_rolls_back_on_an_exception(server: Serving, client: Client) -> None:
    """And the rollback really reaches the server.

    Asserting only that the row is absent afterwards is not enough, and
    `scripts/mutate.py` proved it: deleting the rollback entirely broke
    nothing, because an *abandoned* transaction has uncommitted writes too and
    the row is absent either way. The difference is a transaction left open on
    the head node until its idle timeout, holding a slot against
    `max_transactions`.

    So the id is checked directly: rolling back a transaction the server has
    already discarded is `NOT_FOUND`, and rolling back one that is still open
    succeeds.
    """

    class Deliberate(Exception):
        pass

    with pytest.raises(Deliberate), client.transaction() as tx:
        tx.insert(DOCS, [(u64(201), "kind-t", i64(1), None)])
        raise Deliberate

    assert client.get(DOCS, [u64(201)]) is None

    channel = grpc.insecure_channel(server.address)
    try:
        stub = pb_grpc.RecordsStub(channel)
        with pytest.raises(grpc.RpcError) as caught:
            stub.Rollback(pb.RollbackRequest(transaction=tx.id), metadata=APP.metadata)
        assert caught.value.code() is grpc.StatusCode.NOT_FOUND, (
            "the transaction is still open on the server, so it was abandoned "
            "rather than rolled back"
        )
    finally:
        channel.close()


def test_an_explicit_rollback_discards_the_writes(client: Client) -> None:
    with client.transaction() as tx:
        tx.insert(DOCS, [(u64(202), "kind-t", i64(1), None)])
        tx.rollback()
    assert client.get(DOCS, [u64(202)]) is None


def test_a_read_inside_a_transaction_sees_its_own_uncommitted_writes(
    client: Client,
) -> None:
    with client.transaction() as tx:
        tx.insert(DOCS, [(u64(203), "kind-t", i64(7), None)])
        row = tx.get(DOCS, [u64(203)])
        assert row is not None
        assert row.get("size") == 7
        q = Query(DOCS)
        found = list(tx.query(q.where(q.c.id.eq(u64(203)))))
        assert len(found) == 1
        tx.rollback()


def test_a_read_outside_the_transaction_does_not_see_them(client: Client) -> None:
    with client.transaction() as tx:
        tx.insert(DOCS, [(u64(204), "kind-t", i64(1), None)])
        # A separate session on the same connection, so this is a genuinely
        # different reader rather than the same transaction under a new name.
        assert client.session().get(DOCS, [u64(204)]) is None
        tx.rollback()


def test_freshness_inside_a_transaction_is_refused_rather_than_ignored(
    client: Client,
) -> None:
    # The server would ignore it — a transactional read is served by the
    # transaction, on the writer. Ignoring a field a caller set is how a
    # request comes to mean something other than what was written, which is
    # the rule the server applies to a join input's `sort` and this client
    # applies here.
    with client.transaction() as tx:
        with pytest.raises(ValueError, match="served by that transaction"):
            tx.get(DOCS, [u64(1)], freshness=Freshness.latest())
        tx.rollback()


def test_a_failed_write_inside_a_transaction_still_rolls_back(client: Client) -> None:
    with pytest.raises(NotFound), client.transaction() as tx:
        tx.insert(DOCS, [(u64(205), "kind-t", i64(1), None)])
        tx.update(DOCS, [(u64(9_996), "nope", i64(0), None)])
    assert client.get(DOCS, [u64(205)]) is None


def test_a_conflict_is_visible_when_the_caller_manages_the_transaction(
    server: Serving,
) -> None:
    """`transaction()` does not retry, and says so by raising.

    A context manager cannot re-run its own block, so a `Conflict` has to reach
    the caller. `transact` is the shape that can retry.
    """
    # Nested deliberately, not merged: the inner transaction has to commit
    # *while* the outer one is open, which is what makes the two contend.
    with connect(server) as one, connect(server) as two, pytest.raises(Conflict):  # noqa: SIM117
        with one.transaction() as first:
            first.insert(DOCS, [(u64(210), "one", i64(1), None)])
            with two.transaction() as second:
                second.insert(DOCS, [(u64(210), "two", i64(2), None)])


def test_transact_retries_a_conflict_and_loses_no_update(server: Serving) -> None:
    key = u64(220)
    with connect(server) as setup:
        setup.insert(DOCS, [(key, "counter", i64(0), None)])

    # Both bodies reach the barrier on their first attempt, so both have read
    # the same value before either commits. Without this the two transactions
    # would almost always be serial and the test would prove nothing.
    barrier = threading.Barrier(2, timeout=30)
    attempts = {"a": 0, "b": 0}
    failures: list[BaseException] = []

    def bump(name: str) -> None:
        def body(tx: Transaction) -> None:
            attempts[name] += 1
            row = tx.get(DOCS, [key])
            assert row is not None
            size = row.get("size")
            assert isinstance(size, int)
            if attempts[name] == 1:
                # Only on the first attempt: waiting again would deadlock once
                # the two are no longer running in step.
                barrier.wait()
            tx.update(DOCS, [(key, "counter", i64(size + 1), None)])

        try:
            with connect(server) as own:
                own.transact(body, attempts=10)
        except BaseException as error:
            failures.append(error)
            # Release the other thread rather than leaving it on the barrier
            # until the timeout: a test that fails in 30 seconds hides which
            # thread was at fault.
            with contextlib.suppress(Exception):
                barrier.abort()

    threads = [threading.Thread(target=bump, args=(name,)) for name in ("a", "b")]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=60)

    assert not failures, failures
    with connect(server) as reader:
        row = reader.get(DOCS, [key])
    assert row is not None
    # The property. Two increments from the same starting read: if the retry
    # had re-applied the stale value, this would be 1.
    assert row.get("size") == 2
    # And the control: a run where nothing conflicted would also give 2, so
    # require that the machinery actually ran.
    assert attempts["a"] + attempts["b"] > 2, (
        "neither transaction was retried, so the barrier did not make them contend "
        "and this test is not about conflicts"
    )


def test_transact_does_not_retry_something_that_will_never_succeed(
    client: Client,
) -> None:
    """A duplicate key fails identically forever; retrying it is a hang.

    `RecordStore::transact`'s rule, restated on this side.
    """
    client.insert(DOCS, [(u64(230), "kind-t", i64(1), None)])
    calls = 0

    def body(tx: Transaction) -> None:
        nonlocal calls
        calls += 1
        tx.insert(DOCS, [(u64(230), "again", i64(2), None)])

    from slate import AlreadyExists

    with pytest.raises(AlreadyExists):
        client.transact(body, attempts=5)
    assert calls == 1
