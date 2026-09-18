"""A grouping with keys and no aggregates: the distinct combinations.

The server used to refuse this — "an aggregate request with no aggregates is a
query; use Query" — which was right about the case it was written for (no keys
*and* no aggregates asks for one group with nothing in it) and wrong about
this one. So there was no way to ask a slate head node for `SELECT DISTINCT`
short of requesting a `count(*)` and discarding it.

The oracle is the rows themselves, read back through the same client. A
hand-written expected set would be a second statement of the fixture, and this
suite already has three of those.
"""

from __future__ import annotations

from slate import Agg, AggregateQuery, Client, Query

from .conftest import as_int
from .fixture import DOCS


def _every_kind(client: Client) -> set[str]:
    """Every `kind` in the table, from the rows, with no grouping involved."""
    q = Query(DOCS)
    return {str(row[1]) for row in client.query(q)}


def test_group_keys_with_no_aggregates_are_the_distinct_values(
    oracle_client: Client,
) -> None:
    a = AggregateQuery(DOCS)
    a.group_by(a.c.kind)

    groups = list(oracle_client.aggregate(a))
    expected = _every_kind(oracle_client)
    assert len(expected) > 1, "a one-value column would prove nothing"

    assert {str(g.key[0]) for g in groups} == expected
    # Each exactly once, which the set comparison cannot see.
    assert len(groups) == len(expected)
    # And nothing beside the key: a server that helpfully added a count would
    # agree on every assertion above.
    assert all(len(g) == 0 for g in groups), [list(g) for g in groups]


def test_asking_for_an_aggregate_still_gets_one(oracle_client: Client) -> None:
    """The refusal was removed, not the capability."""
    a = AggregateQuery(DOCS)
    a.group_by(a.c.kind)
    a.aggregate(Agg.count())

    groups = list(oracle_client.aggregate(a))
    assert all(len(g) == 1 for g in groups), [list(g) for g in groups]
    assert sum(as_int(g.aggregate(0)) for g in groups) == len(
        list(oracle_client.query(Query(DOCS)))
    )


def test_neither_keys_nor_aggregates_is_still_refused(oracle_client: Client) -> None:
    """The case the old refusal was actually written for.

    One group, over every row, computing nothing. There is no answer to give,
    and the client must not paper over it — a caller who meant `Query` should
    find out here rather than get an empty group back.
    """
    import pytest

    from slate import InvalidRequest

    a = AggregateQuery(DOCS)
    with pytest.raises(InvalidRequest) as caught:
        list(oracle_client.aggregate(a))
    assert "nothing in it" in str(caught.value), str(caught.value)
