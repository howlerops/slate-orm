"""Read-your-writes, monotonic reads, and where the caller has to know.

Every test here runs against `stale_server`, whose pool holds a replica that is
empty and will never catch up. That is the point. With no lag a token does
nothing, and a read-your-writes test passes just as happily against a client
that ignores freshness entirely — which is the failure `freshness.rs` in the
server's own suite exists to rule out and which this file copies.

So each test that asserts a token worked also asserts the *control*: that the
same read without one really does miss the write.
"""

from __future__ import annotations

import pytest

from slate import Client, Freshness, Query, ReadToken, i64, u64
from slate.errors import Unavailable

from .conftest import Serving, connect
from .fixture import DOCS

#: Distinct ids per test: the stale server is session-scoped and shared.
FIRST = 300


def _rows(client: Client, key: int, **kwargs: object) -> list[object]:
    q = Query(DOCS)
    return list(client.query(q.where(q.c.id.eq(u64(key))), **kwargs))  # type: ignore[arg-type]


def test_a_session_reads_its_own_write_without_the_caller_naming_a_token(
    stale_server: Serving,
) -> None:
    key = FIRST
    with connect(stale_server) as client:
        # The control, and it has to come first: this session has observed
        # nothing yet, so its reads go to the replica.
        assert _rows(client, key) == [], "the replica is not behind; nothing here is tested"

        client.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])

        # No token in sight. The session threaded the sequence the write
        # returned, and the pool refused to serve the read from a replica that
        # cannot reach it.
        found = _rows(client, key)
        assert len(found) == 1


def test_the_control_is_a_control(stale_server: Serving) -> None:
    """Asking for ANY explicitly still misses the write, after it was written.

    Without this, the test above could be passing because the replica is not
    actually behind — and every assertion in this file would be vacuous.
    """
    key = FIRST + 1
    with connect(stale_server) as client:
        client.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])
        stale = _rows(client, key, freshness=Freshness.any())
        assert stale == []


def test_which_view_served_a_read_is_reported(stale_server: Serving) -> None:
    with connect(stale_server) as client:
        stream = client.query(Query(DOCS), freshness=Freshness.any())
        list(stream)
        assert stream.served_by is not None
        assert stream.served_by.replica == "stale-replica"


def test_a_token_moves_the_read_off_the_stale_replica(stale_server: Serving) -> None:
    key = FIRST + 2
    with connect(stale_server) as client:
        written = client.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])
        assert written.sequence is not None
        stream = client.query(Query(DOCS), freshness=Freshness.at_least(written.sequence))
        list(stream)
        assert stream.served_by is not None
        assert stream.served_by.replica != "stale-replica"


def test_latest_goes_to_the_writer(stale_server: Serving) -> None:
    key = FIRST + 3
    with connect(stale_server) as client:
        client.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])
        found = _rows(client, key, freshness=Freshness.latest())
        assert len(found) == 1


def test_a_sequence_no_view_has_reached_is_refused(stale_server: Serving) -> None:
    """PROTOCOL FINDING, since fixed: `at_least` is now checked against the writer.

    This test was written the other way round. `Freshness.at_least(n)` promises
    "only a view that has reached this sequence", and `ReplicaPool::route`
    implemented its writer fallback *unconditionally* — returning the writer
    without asking whether the writer had reached `n` either. A sequence past
    everything came back as rows. It was pinned here as the behaviour that
    existed rather than the behaviour expected, so that a fix would have
    something to break.

    The fix landed in `crates/slate-kernel/src/pool.rs` and this test broke,
    which is the whole reason it was written that way. It now asserts the
    promise: unreachable means refused, and refused as `Unavailable`, because a
    later attempt may well find a view that has caught up.
    """
    with connect(stale_server) as client:
        impossible = 2**40
        with pytest.raises(Unavailable):
            list(client.query(Query(DOCS), freshness=Freshness.at_least(impossible)))

        # The control: a sequence the writer *has* reached is still served, so
        # the refusal above is about the sequence and not about a node that has
        # started refusing every token.
        written = client.insert(DOCS, [(u64(FIRST + 9), "kind-f", i64(1), None)])
        assert written.sequence is not None
        stream = client.query(Query(DOCS), freshness=Freshness.at_least(written.sequence))
        assert list(stream), "a reachable sequence returned nothing"


def test_the_watermark_is_readable_and_only_moves_forward(
    stale_server: Serving,
) -> None:
    key = FIRST + 4
    with connect(stale_server) as client:
        assert client.watermark is None
        first = client.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])
        assert first.sequence is not None
        assert client.watermark == first.sequence

        # Observing something older must not move it back: a session that
        # went backwards would serve a read from a view behind one it had
        # already shown the caller.
        client.observe(ReadToken(1))
        assert client.watermark == first.sequence


def test_a_transaction_commit_feeds_the_session_watermark(
    stale_server: Serving,
) -> None:
    """The explicit-transaction path, which the single-statement one does not cover.

    A single-statement write returns its sequence on the `WriteResponse`; a
    transactional one returns it on the `CommitResponse`, and those are two
    different lines in this client. Found by `scripts/mutate.py`: removing the
    second one broke nothing, because every other test writes without a
    transaction.
    """
    key = FIRST + 7
    with connect(stale_server) as client:
        assert _rows(client, key) == [], "the replica is not behind"
        with client.transaction() as tx:
            tx.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])
        assert tx.sequence is not None
        assert client.watermark == tx.sequence
        # And the point of the watermark: the next read cannot be served by
        # the replica that has not reached it.
        assert len(_rows(client, key)) == 1


def test_a_token_can_be_handed_to_another_session(stale_server: Serving) -> None:
    """The case implicit threading cannot cover, and why `watermark` is public.

    A job queue writes and enqueues; a worker in another process has to see
    that write. Nothing implicit can carry a token across that boundary, so the
    token is a value the caller can take out and put in.
    """
    key = FIRST + 5
    with connect(stale_server) as writer:
        written = writer.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])
    assert written.sequence is not None

    with connect(stale_server) as worker:
        # The control: a fresh session knows nothing and reads the stale
        # replica.
        assert _rows(worker, key) == []
        worker.observe(written.sequence)
        assert len(_rows(worker, key)) == 1


def test_sessions_have_independent_watermarks(stale_server: Serving) -> None:
    """One user's write must not pin another user's reads to the writer."""
    key = FIRST + 6
    with connect(stale_server) as client:
        writer = client.session()
        reader = client.session()
        writer.insert(DOCS, [(u64(key), "kind-f", i64(1), None)])
        assert writer.watermark is not None
        assert reader.watermark is None
        stream = reader.query(Query(DOCS))
        list(stream)
        assert stream.served_by is not None
        assert stream.served_by.replica == "stale-replica"


def test_monotonic_reads_pin_a_session_that_has_seen_the_writer(
    stale_server: Serving,
) -> None:
    """Observing a read's sequence is what makes reads monotonic — and costly.

    Both halves are asserted, because the cost is the reason the flag exists.
    """
    with connect(stale_server) as monotonic, connect(
        stale_server, monotonic_reads=False
    ) as not_monotonic:
        for client in (monotonic, not_monotonic):
            # One read from the writer, which is the only view that can be
            # this fresh.
            stream = client.query(Query(DOCS), freshness=Freshness.latest())
            list(stream)

        # The monotonic session may not now be served by anything behind that.
        after = monotonic.query(Query(DOCS))
        list(after)
        assert after.served_by is not None
        assert after.served_by.replica != "stale-replica"

        # The other one falls straight back to the replica, which is cheaper
        # and is a weaker guarantee.
        relaxed = not_monotonic.query(Query(DOCS))
        list(relaxed)
        assert relaxed.served_by is not None
        assert relaxed.served_by.replica == "stale-replica"


def test_a_join_reports_one_view_for_the_whole_result(client: Client) -> None:
    """A join is several reads of one snapshot, so there is one `served_by`.

    Runs against the ordinary server: the property is about the response shape,
    not about lag.
    """
    from slate import JoinQuery

    from .fixture import AUTHORS, BOOKS

    j = JoinQuery()
    a = j.add(AUTHORS)
    j.add(BOOKS, on=[(a.c.id, "author_id")])
    stream = client.join(j)
    list(stream)
    assert stream.served_by is not None
    assert stream.served_by.replica != ""
