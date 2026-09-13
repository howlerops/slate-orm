"""Streams: batching, laziness, cancellation, and the one place they are not.

The batch size is the server's (256 rows, chosen by argument rather than
measurement) and is deliberately invisible through `RowStream` — it is not part
of the answer. `test_the_stream_really_is_batched` reaches past this client to
the generated stub to check that it happens at all, because a client that
buffered the whole result would pass every other test in this file and would
hold a million rows in memory in production.
"""

from __future__ import annotations

import grpc

from slate import Client, Query, i64, u64
from slate._proto.slate.v1 import records_pb2 as pb
from slate._proto.slate.v1 import records_pb2_grpc as pb_grpc

from .conftest import APP, Serving, connect
from .fixture import DOCS

#: More than the server's batch of 256, so several messages are needed.
BULK = 600
FIRST = 10_000


def _seed_bulk(client: Client) -> None:
    q = Query(DOCS)
    if len(list(client.query(q.where(q.c.kind.eq("bulk"))))) == BULK:
        return
    client.insert(
        DOCS,
        [(u64(FIRST + n), "bulk", i64(n), None) for n in range(BULK)],
    )


def test_a_large_result_arrives_complete(client: Client) -> None:
    _seed_bulk(client)
    q = Query(DOCS)
    rows = list(client.query(q.where(q.c.kind.eq("bulk"))))
    assert len(rows) == BULK
    # In order, so a batch boundary cannot have reordered or duplicated
    # anything: the scan is ascending by primary key.
    assert [r.get("id") for r in rows] == [FIRST + n for n in range(BULK)]


def test_the_stream_really_is_batched(server: Serving, client: Client) -> None:
    """Observed through the generated stub, because this client hides it.

    A client that read the whole result into memory before returning would
    satisfy every other assertion here.
    """
    _seed_bulk(client)
    channel = grpc.insecure_channel(server.address)
    try:
        stub = pb_grpc.RecordsStub(channel)
        q = Query(DOCS)
        request = pb.QueryRequest(query=q.where(q.c.kind.eq("bulk")).to_proto())
        messages = list(stub.Query(request, metadata=APP.metadata))
    finally:
        channel.close()
    assert len(messages) > 1, "600 rows arrived in one message; the stream is not batched"
    assert sum(len(m.rows) for m in messages) == BULK
    # The first message always carries `served_by`, even when it carries no
    # rows — which is what lets this client report where a read went before
    # the first row.
    assert messages[0].served_by.replica != ""


def test_a_stream_is_lazy(client: Client) -> None:
    """Constructing it reads one message, not the whole result."""
    _seed_bulk(client)
    q = Query(DOCS)
    stream = client.query(q.where(q.c.kind.eq("bulk")))
    # `served_by` is already there, so a message has been read...
    assert stream.served_by is not None
    taken = [next(stream) for _ in range(5)]
    assert len(taken) == 5
    # ...and the rest is still on the wire. Cancelling frees the server's task.
    stream.cancel()


def test_a_stream_is_a_context_manager(client: Client) -> None:
    _seed_bulk(client)
    q = Query(DOCS)
    with client.query(q.where(q.c.kind.eq("bulk"))) as stream:
        assert next(stream).get("kind") == "bulk"
    # Cancelling twice must not raise: `__exit__` and an explicit `cancel` in
    # the body is an ordinary thing to write.
    stream.cancel()


def test_an_empty_result_still_reports_where_it_was_served(client: Client) -> None:
    q = Query(DOCS)
    stream = client.query(q.where(q.c.kind.eq("nothing-has-this-kind")))
    assert stream.served_by is not None
    assert list(stream) == []


def test_a_transactional_read_is_not_streamed_but_reads_the_same(
    server: Serving,
) -> None:
    """A transactional read is answered in one go; the API does not change.

    The server cannot stream from inside a transaction — see `Sessions::query`
    — and replays the rows through the same batched response shape. Worth a
    test because it means a large read inside a transaction is materialised on
    the server, which is a performance cliff the wire does not mention.
    """
    with connect(server) as client:
        _seed_bulk(client)
        with client.transaction() as tx:
            q = Query(DOCS)
            rows = list(tx.query(q.where(q.c.kind.eq("bulk"))))
            tx.rollback()
    assert len(rows) == BULK
