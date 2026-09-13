"""The first thing to check: this client can talk to that server at all."""

from __future__ import annotations

from slate import Client, Query

from .fixture import DOCS


def test_a_plain_query_returns_the_seeded_rows(oracle_client: Client) -> None:
    # The pristine server, so the row set is the fixture's and not whatever
    # the writing tests happen to have left behind.
    rows = list(oracle_client.query(Query(DOCS)))
    assert [row.get("id") for row in rows] == [1, 2, 3, 4, 5, 6, 7]
    assert rows[0].get("kind") == "kind-a"


def test_served_by_is_populated_before_the_first_row(client: Client) -> None:
    # The server always sends a first message even for an empty result, because
    # it carries `served_by`. This client reads it eagerly, so the property is
    # observable without consuming a row.
    q = Query(DOCS)
    stream = client.query(q.where(q.c.id.eq(999)))
    assert stream.served_by is not None
    assert stream.served_by.replica != ""
    assert list(stream) == []
