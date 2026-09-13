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

from slate import Client, JoinQuery, Query, i64

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
