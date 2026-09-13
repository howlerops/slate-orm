"""The strongest test here: does this client's request mean what it says?

Every case below is stated twice — once in `testserver/src/main.rs` against
`slate-kernel`'s flat ordinals, and once here against the wire's `ColumnRef` —
and the two must return identical rows. The statements are deliberately not
shared. A helper that built both would make them agree by construction, which
is the failure mode of every "round trip" test: it catches a field dropped on
one side and passes happily when a field is dropped on both.

What this catches that nothing else does is a client that builds a *legitimate*
request meaning something other than what the caller wrote — a `ColumnRef` with
the right kind and the wrong index, a filter attached to input 0 when it
belonged on input 1, a `HAVING` addressed as a key when it is an aggregate. The
server cannot refuse any of those. It answers them.

`test_every_oracle_case_is_used` is what stops this file rotting: a case added
to the Rust side and not here would otherwise be a case nobody runs.
"""

from __future__ import annotations

import pathlib

import pytest

from slate import (
    Agg,
    AggregateQuery,
    Client,
    JoinQuery,
    JoinType,
    Query,
    ScanOrder,
    any_of,
    asc,
    desc,
    i64,
    lower,
    u64,
    upper,
)

from .fixture import AUTHORS, BOOKS, DOCS, SALES, USERS
from .oracle import Oracle, multiset, tag_group, tag_joined, tag_row


@pytest.fixture(scope="session")
def oracle(oracle_server: object, oracle_path: pathlib.Path) -> Oracle:
    # Depends on `oracle_server` so that the file exists: the binary writes it
    # before it opens the socket, so a test that has a client cannot race it.
    return Oracle(oracle_path)


@pytest.fixture
def client(oracle_client: Client) -> Client:
    """Every test in this file reads the pristine server, never the shared one."""
    return oracle_client


# --- single-table reads -----------------------------------------------------


def test_docs_all(client: Client, oracle: Oracle) -> None:
    rows = [tag_row(r) for r in client.query(Query(DOCS))]
    assert rows == oracle.rows("docs_all")


def test_docs_filtered_paged(client: Client, oracle: Oracle) -> None:
    q = Query(DOCS)
    q.where(q.c.size >= 15).sort(desc(q.c.size), asc(q.c.id)).limit(3).offset(1)
    rows = [tag_row(r) for r in client.query(q)]
    assert rows == oracle.rows("docs_filtered_paged")
    # A paging case that returned everything would agree with the oracle and
    # prove nothing about the limit.
    assert len(rows) == 3


def test_docs_null_or_like(client: Client, oracle: Oracle) -> None:
    q = Query(DOCS)
    q.where(any_of([q.c.note.is_null(), q.c.kind.like("kind-c%")]))
    assert [tag_row(r) for r in client.query(q)] == oracle.rows("docs_null_or_like")


def test_docs_computed(client: Client, oracle: Oracle) -> None:
    # The computed value is named as `computed(0)`, never as ordinal 4. The
    # server adds the table's width; this side never learns it.
    q = Query(DOCS)
    q.compute(q.c.size * i64(2), upper(q.c.kind))
    q.where(q.computed(0) > i64(40)).sort(asc(q.c.id))
    rows = list(client.query(q))
    assert [tag_row(r) for r in rows] == oracle.rows("docs_computed")
    # And the *response* side does need the width, which is the finding: the
    # wire returns one flat row and says the computed values are after the
    # table's own columns.
    assert rows[0].computed(1) == rows[0].get("kind").upper()  # type: ignore[union-attr]


def test_docs_descending(client: Client, oracle: Oracle) -> None:
    rows = [tag_row(r) for r in client.query(Query(DOCS).order(ScanOrder.DESCENDING))]
    assert rows == oracle.rows("docs_descending")
    assert rows != oracle.rows("docs_all"), "the fixture does not distinguish direction"


def test_docs_projected(client: Client, oracle: Oracle) -> None:
    q = Query(DOCS)
    rows = list(client.query(q.select(q.c.id, q.c.kind)))
    assert [tag_row(r) for r in rows] == oracle.rows("docs_projected")
    # A column outside the projection is null rather than absent: the row keeps
    # its width, which is what makes ordinal addressing work at all.
    assert rows[0].get("size") is None
    assert rows[0].get("id") == 1


def test_a_row_policy_hides_the_same_rows_it_hides_in_process(
    client: Client, oracle: Oracle
) -> None:
    rows = [tag_row(r) for r in client.query(Query(USERS))]
    assert rows == oracle.rows("users_visible")
    # The control. Three users are seeded and the policy plus the tenant scope
    # hide two of them; without this the case would pass against a read that
    # returned nothing.
    assert len(rows) == 1


# --- joins ------------------------------------------------------------------


def _authors_books(join_type: JoinType) -> JoinQuery:
    """Authors joined to books on the author id, as the four join types."""
    j = JoinQuery()
    a = j.add(AUTHORS)
    j.add(BOOKS, on=[(a.c.id, "author_id")], join_type=join_type)
    return j


@pytest.mark.parametrize(
    ("name", "join_type"),
    [
        ("join_inner", JoinType.INNER),
        ("join_left", JoinType.LEFT),
        ("join_right", JoinType.RIGHT),
        ("join_full", JoinType.FULL),
    ],
)
def test_join_types(
    client: Client, oracle: Oracle, name: str, join_type: JoinType
) -> None:
    rows = [tag_joined(r) for r in client.join(_authors_books(join_type))]
    assert multiset(rows) == multiset(oracle.joined(name))


def test_the_four_join_types_return_different_rows(oracle: Oracle) -> None:
    """Otherwise the four cases above are one case run four times."""
    seen = {
        name: multiset(oracle.joined(name))
        for name in ("join_inner", "join_left", "join_right", "join_full")
    }
    distinct = {tuple(rows) for rows in seen.values()}
    assert len(distinct) == 4, f"two join types agree: {list(seen)}"


def test_join_cross_condition(client: Client, oracle: Oracle) -> None:
    # One expression naming two inputs. A client that got either side's
    # position wrong builds a request the server accepts and answers wrongly.
    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS, on=[(a.c.id, "author_id")])
    b.having(b.c.year > a.c.born)
    rows = [tag_joined(r) for r in client.join(j)]
    assert multiset(rows) == multiset(oracle.joined("join_cross_condition"))


def test_join_right_filtered(client: Client, oracle: Oracle) -> None:
    # The filter belongs to the *second* input. A client that put every filter
    # on input 0 would disagree here and nowhere else.
    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS, on=[(a.c.id, "author_id")])
    b.where(b.c.title.like("a-%"))
    rows = [tag_joined(r) for r in client.join(j)]
    assert multiset(rows) == multiset(oracle.joined("join_right_filtered"))


def test_join_two_keys(client: Client, oracle: Oracle) -> None:
    j = JoinQuery()
    a = j.add(AUTHORS)
    j.add(BOOKS, on=[(a.c.id, "author_id"), (a.c.tenant_id, "tenant_id")])
    rows = [tag_joined(r) for r in client.join(j)]
    assert multiset(rows) == multiset(oracle.joined("join_two_keys"))


def test_join_self(client: Client, oracle: Oracle) -> None:
    # Two inputs over one table. An input is a position, which a self-join
    # distinguishes and a qualified name could not have.
    j = JoinQuery()
    left = j.add(BOOKS)
    j.add(BOOKS, on=[(left.c.author_id, "author_id")])
    rows = [tag_joined(r) for r in client.join(j)]
    assert multiset(rows) == multiset(oracle.joined("join_self_books"))


def test_chain_of_three(client: Client, oracle: Oracle) -> None:
    # Three inputs: the wire has one shape for a join and a chain and the
    # server dispatches on the count. The third input's references are what a
    # client gets wrong.
    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS, on=[(a.c.id, "author_id")])
    j.add(SALES, on=[(b.c.id, "book_id")])
    rows = [tag_joined(r) for r in client.join(j)]
    assert multiset(rows) == multiset(oracle.joined("chain_authors_books_sales"))
    assert rows, "the chain fixture matched nothing, so this proves nothing"


# --- aggregates -------------------------------------------------------------


def test_agg_plain(client: Client, oracle: Oracle) -> None:
    a = AggregateQuery(BOOKS)
    a.aggregate(
        Agg.count(),
        Agg.count(a.c.year),
        Agg.min(a.c.year),
        Agg.max(a.c.year),
        Agg.sum(a.c.year),
        Agg.avg(a.c.year),
        Agg.count_distinct(a.c.author_id),
    )
    groups = [tag_group(g) for g in client.aggregate(a)]
    assert groups == oracle.groups("agg_plain")


def test_agg_group_having_over_an_aggregate(client: Client, oracle: Oracle) -> None:
    a = AggregateQuery(BOOKS)
    a.group_by(a.c.author_id)
    a.aggregate(Agg.count(), Agg.sum(a.c.year))
    # `>= 2`, not `>= 1`: every group has at least one row, so `>= 1` keeps all
    # of them and the case cannot tell an aggregate reference from a group key.
    # Found by `scripts/mutate.py`, which substituted one for the other and was
    # not noticed here.
    a.having(a.agg(0).ge(u64(2)))
    groups = [tag_group(g) for g in client.aggregate(a)]
    assert groups == oracle.groups("agg_group_having")
    # And the discrimination, stated: exactly one author has two visible books,
    # so a HAVING that kept everything would be a different number.
    assert len(groups) == 1


def test_agg_group_having_over_a_key(client: Client, oracle: Oracle) -> None:
    a = AggregateQuery(BOOKS)
    a.group_by(a.c.author_id)
    a.aggregate(Agg.count())
    a.having(a.key(0).eq(u64(1)))
    groups = [tag_group(g) for g in client.aggregate(a)]
    assert groups == oracle.groups("agg_group_having_key")
    assert len(groups) == 1


def test_agg_group_by_a_computed_value(client: Client, oracle: Oracle) -> None:
    a = AggregateQuery(BOOKS)
    a.compute(lower(a.c.title))
    a.group_by(a.computed(0))
    a.aggregate(Agg.count())
    groups = [tag_group(g) for g in client.aggregate(a)]
    assert groups == oracle.groups("agg_group_by_computed")


# --- the guard --------------------------------------------------------------


def test_every_oracle_case_is_used(oracle: Oracle) -> None:
    """A case the Rust side computes and this file never asks for is dead.

    Runs last by name, and depends on the session-scoped `Oracle` that every
    test above shares, so `used` is the union of what they consumed.
    """
    unused = oracle.names - oracle.used
    assert not unused, f"the oracle computes these and nothing compares them: {sorted(unused)}"
