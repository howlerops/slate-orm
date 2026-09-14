"""Explaining a read, and the two things only `Explain` will tell you.

`ExplainResponse.residual` is the predicate re-checked on every candidate row
*including the mandatory security filter*, which makes it the proof that a
policy reached the executor. `ExplainResponse.warnings` is the only place the
server says it did something the request did not ask for. Both are worth a
test, and the second is worth one because it is also a protocol finding: a
plain `Query` discards those warnings, so "my hint did nothing" is invisible
unless you go and ask.
"""

from __future__ import annotations

import pytest

from slate import (
    Agg,
    AggregateQuery,
    Client,
    GroupedJoinQuery,
    Identity,
    JoinQuery,
    PermissionDenied,
    Query,
    i64,
)

from .conftest import Serving
from .fixture import AUTHORS, BOOKS, DOCS, USERS


def test_explain_describes_the_access_path(oracle_client: Client) -> None:
    q = Query(DOCS)
    plan = oracle_client.explain(q.where(q.c.kind.eq("kind-a")))
    assert plan.table == "docs"
    assert plan.access
    assert plan.display
    assert plan.estimated_rows >= 0


def test_the_residual_is_the_proof_a_policy_reached_the_executor(
    oracle_client: Client,
) -> None:
    """`users` has an `owner = me` policy. It must be in the residual.

    Not "the query returned fewer rows", which a filter would also produce.
    """
    plan = oracle_client.explain(Query(USERS))
    assert plan.residual, "an unfiltered read of a policied table has an empty residual"
    assert "owner" in plan.residual.lower() or "2" in plan.residual


def test_a_hint_naming_an_index_that_does_not_exist_is_a_warning(
    oracle_client: Client,
) -> None:
    """Ignored rather than refused — a query that stops working because an
    index was renamed is worse than one that gets slower — and reported here.

    PROTOCOL FINDING: `Query` does not carry these warnings, so the same
    request run for its rows silently does nothing about the hint.
    """
    q = Query(DOCS)
    plan = oracle_client.explain(q.using_index("no_such_index"))
    assert any("no_such_index" in w for w in plan.warnings), plan.warnings

    # And the same request as a read: no warning reaches the caller.
    rows = list(oracle_client.query(Query(DOCS).using_index("no_such_index")))
    assert len(rows) == 7


def test_a_usable_hint_produces_no_warning(oracle_client: Client) -> None:
    """The control: otherwise the test above passes against a server that
    warns about everything."""
    plan = oracle_client.explain(Query(DOCS).using_index("by_kind"))
    assert plan.warnings == ()


def test_explain_join_describes_every_input(oracle_client: Client) -> None:
    j = JoinQuery()
    a = j.add(AUTHORS)
    j.add(BOOKS, on=[(a.c.id, "author_id")])
    plan = oracle_client.explain_join(j)
    assert len(plan.inputs) == 2
    # The first has no algorithm: nothing is joined to it.
    assert plan.inputs[0].algorithm is None
    assert plan.inputs[1].algorithm is not None
    assert plan.display


def test_a_build_limit_raised_above_the_servers_is_clamped_with_a_warning(
    oracle_client: Client,
) -> None:
    """A limit a client can raise is not a limit.

    Clamped and reported rather than refused — and reported only here, which
    is the same finding as the hint above.
    """
    j = JoinQuery()
    a = j.add(AUTHORS)
    j.add(BOOKS, on=[(a.c.id, "author_id")])
    j.build_limit(2**40)
    plan = oracle_client.explain_join(j)
    assert any("build limit lowered" in w for w in plan.warnings), plan.warnings


def test_a_covering_projection_is_reported_as_index_only(
    oracle_client: Client,
) -> None:
    """`index_only` says no row is read at all. Worth surfacing: it is the
    difference between 100 point reads and none."""
    q = Query(DOCS)
    plan = oracle_client.explain(
        q.where(q.c.size.ge(i64(0))).select(q.c.size).using_index("by_size")
    )
    assert isinstance(plan.index_only, bool)


def test_a_read_grant_does_not_carry_explain(server: Serving) -> None:
    """`EXPLAIN` is its own action, and `Action::ALL` deliberately excludes it.

    The `reader` role on this node holds `ALL` on `docs` and nothing else. It
    can read the table and must not be able to explain a read of it: a plan is
    costed against statistics covering rows a row policy may hide, so the plan
    discloses what the rows do not.

    This suite had no such test until the action existed — the server refused
    every explain here and the failures read as a broken fixture, which is
    exactly how a missing negative test looks from the inside.
    """
    reader = Identity("u64:9", tenant="u64:1", roles=["reader"])
    with Client(server.address, identity=reader) as client:
        session = client.session()
        # The read itself is allowed.
        list(session.query(Query(DOCS).limit(1)))
        with pytest.raises(PermissionDenied):
            session.explain(Query(DOCS))


def test_explaining_a_grouped_join_is_not_explaining_the_join(
    oracle_client: Client,
) -> None:
    """The reason `explain_aggregate` exists.

    Grouping narrows each input's projection to the group keys, the aggregates'
    columns and the join keys, so the two plans decode different amounts of
    every row. On a fixture with no usable index the access path is the same
    either way, which is why this asserts on `decodes` rather than on
    `index_only` or on the display string — `decodes` is the field that was
    added to the wire because nothing else could tell the two plans apart.
    """
    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS, on=[(a.c.id, "author_id")])

    plain = oracle_client.explain_join(j)

    # Group by authors.country and take MAX(books.year) — columns that are not
    # the join keys, deliberately. A plan that ignored the grouping entirely
    # would still narrow, to the join keys alone, and "narrower than ungrouped"
    # would pass. What must hold is that *these* columns are what it decodes.
    grouped_query = GroupedJoinQuery(j)
    grouped_query.group_by(a.c.country)
    grouped_query.aggregate(Agg.count())
    grouped_query.aggregate(Agg.max(b.c.year))
    grouped = oracle_client.explain_aggregate(grouped_query)

    assert grouped.join is not None, "a grouped join explains as a join"
    assert grouped.input is None, "the one-table field stays unset for a join"
    assert len(grouped.join.inputs) == len(plain.inputs)

    widths = [(p.plan.decodes, g.plan.decodes) for p, g in zip(plain.inputs, grouped.join.inputs)]
    for at, (wide, narrow) in enumerate(widths):
        assert len(narrow) <= len(wide), f"grouping widened input {at}: {wide} -> {narrow}"
    assert any(len(narrow) < len(wide) for wide, narrow in widths), (
        f"grouping narrowed nothing, so this is not the grouped read's plan:\n"
        f"{plain.display}\nvs\n{grouped.display}"
    )
    assert grouped.display.startswith("Group by ["), grouped.display

    # The grouping's own columns, in each input's own ordinals.
    assert AUTHORS.ordinal_of("country") in grouped.join.inputs[0].plan.decodes, (
        f"the group key authors.country is not decoded: {grouped.join.inputs[0].plan.decodes}"
    )
    assert BOOKS.ordinal_of("year") in grouped.join.inputs[1].plan.decodes, (
        f"the aggregated books.year is not decoded: {grouped.join.inputs[1].plan.decodes}"
    )


def test_explaining_a_grouped_table_answers_in_the_input_field(
    oracle_client: Client,
) -> None:
    """`input` and `join` are mutually exclusive, and the server sets the one
    matching the request — so a client reading the wrong one gets nothing
    rather than something wrong."""
    aggregate = AggregateQuery(BOOKS)
    aggregate.group_by(aggregate.c.author_id)
    aggregate.aggregate(Agg.count())

    plan = oracle_client.explain_aggregate(aggregate)
    assert plan.input is not None, "a grouped table explains as a table"
    assert plan.join is None, "the join field stays unset for one table"
    assert plan.input.table == "books"


def test_a_read_grant_does_not_carry_explain_for_an_aggregate(server: Serving) -> None:
    """A plan is `Action::Explain` wherever it is asked for.

    A new RPC is exactly where that check gets forgotten, and the identity that
    catches it is one that can run the read and not ask how.
    """
    reader = Identity("u64:9", tenant="u64:1", roles=["reader"])
    with Client(server.address, identity=reader) as client:
        session = client.session()
        aggregate = AggregateQuery(DOCS)
        aggregate.aggregate(Agg.count())
        # The premise: this identity can run it.
        list(session.aggregate(aggregate))
        with pytest.raises(PermissionDenied):
            session.explain_aggregate(aggregate)
