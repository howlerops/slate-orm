"""Several writes in one round trip, and the distinction that makes it safe.

A batch is a network optimisation. A transaction is an atomicity guarantee.
They differ *only when something fails*, so `Batch` takes its `Atomicity` in
the constructor rather than defaulting to one — the client-side half of the
server refusing an unspecified one.

`test_an_independent_batch_carries_on_past_a_failure` and
`test_an_atomic_batch_undoes_everything_before_the_failure` are the same three
operations under the two guarantees, side by side, because the difference is
the whole feature and a reader should be able to see it in one screen.
"""

from __future__ import annotations

import pytest

from slate import (
    AlreadyExists,
    Atomicity,
    Batch,
    Client,
    DeleteWhere,
    InvalidRequest,
    Query,
    UpdateWhere,
    i64,
    u64,
)

from .conftest import Serving, connect
from .fixture import DOCS

#: Ids well clear of every other module's.
FIRST = 800_000
KIND = "batched"


@pytest.fixture(autouse=True)
def _clean(server: Serving) -> None:
    """These tests write what they then count, so each starts from nothing."""
    client = connect(server)
    w = DeleteWhere(DOCS)
    client.delete_where(w.where(w.c.kind.eq(KIND)))


def _mine(client: Client) -> list[int]:
    q = Query(DOCS)
    return sorted(row.get("id") for row in client.query(q.where(q.c.kind.eq(KIND))))


def _row(n: int) -> tuple:
    return (u64(FIRST + n), KIND, i64(n), None)


def test_an_atomicity_is_required(client: Client) -> None:
    """Not defaulted, and not accepted as a bare string either."""
    with pytest.raises(TypeError, match="Atomicity"):
        Batch("independent")  # type: ignore[arg-type]


def test_an_empty_batch_is_refused_before_it_is_sent(client: Client) -> None:
    """Refused here rather than at the server, which would also refuse it.

    A round trip to be told the list was empty is a round trip the caller can
    be spared, and the message is the same either way.
    """
    with pytest.raises(InvalidRequest, match="at least one"):
        client.batch(Batch(Atomicity.INDEPENDENT))


def test_an_independent_batch_applies_every_operation(client: Client) -> None:
    b = Batch(Atomicity.INDEPENDENT)
    for n in range(5):
        b.insert(DOCS, [_row(n)])
    result = client.batch(b)

    assert len(result) == 5, "one outcome per operation"
    assert all(one.ok for one in result.outcomes)
    assert [one.written.affected for one in result.outcomes] == [1] * 5
    assert result.sequence is not None
    assert _mine(client) == [FIRST + n for n in range(5)]


def test_an_independent_batch_carries_on_past_a_failure(client: Client) -> None:
    client.insert(DOCS, [_row(1)])

    b = Batch(Atomicity.INDEPENDENT)
    b.insert(DOCS, [_row(0)])
    b.insert(DOCS, [_row(1)])  # already there
    b.insert(DOCS, [_row(2)])
    result = client.batch(b)

    assert [one.ok for one in result.outcomes] == [True, False, True]
    failure = result.failures[0].error
    assert isinstance(failure, AlreadyExists), failure
    # The stable token survives the trip through a message body, which is the
    # half a lone RPC gets from its trailers.
    assert failure.reason == "DUPLICATE_PRIMARY_KEY", failure.reason

    # 0 and 2 landed although 1 failed between them. That is independence, and
    # it is what a caller who wanted a transaction would be horrified by.
    assert _mine(client) == [FIRST, FIRST + 1, FIRST + 2]


def test_an_atomic_batch_undoes_everything_before_the_failure(client: Client) -> None:
    client.insert(DOCS, [_row(1)])

    b = Batch(Atomicity.ALL_OR_NOTHING)
    b.insert(DOCS, [_row(0)])
    b.insert(DOCS, [_row(1)])  # already there
    b.insert(DOCS, [_row(2)])
    with pytest.raises(AlreadyExists):
        client.batch(b)

    # Only the setup row: 0 was applied before 1 failed and rolled back with
    # it, and 2 never ran. The same three operations as the test above.
    assert _mine(client) == [FIRST + 1]


def test_an_atomic_batch_reports_no_per_operation_outcomes(client: Client) -> None:
    b = Batch(Atomicity.ALL_OR_NOTHING)
    for n in range(3):
        b.insert(DOCS, [_row(n)])
    result = client.batch(b)

    assert result.outcomes == [], "they all happened; there is nothing to report"
    assert result.sequence is not None
    assert _mine(client) == [FIRST, FIRST + 1, FIRST + 2]


def test_a_batch_carries_every_kind_of_write(client: Client) -> None:
    client.insert(DOCS, [_row(n) for n in range(6)])

    w = DeleteWhere(DOCS)
    u = UpdateWhere(DOCS)
    b = Batch(Atomicity.INDEPENDENT)
    b.insert(DOCS, [_row(6)])
    b.upsert(DOCS, [(u64(FIRST + 6), KIND, i64(60), None)])
    b.delete(DOCS, [[u64(FIRST)]])
    b.delete_where(w.where(w.c.kind.eq(KIND) & w.c.size.ge(i64(5))).returning())
    b.update_where(
        u.where(u.c.kind.eq(KIND)).set(u.c.note, "touched").returning()
    )
    result = client.batch(b)

    assert [one.ok for one in result.outcomes] == [True] * 5
    # The delete_where asked for its rows and got them; ids 5 and 6.
    removed = result.outcomes[3].written
    assert removed.affected == 2
    assert sorted(row.get("id") for row in removed.rows) == [FIRST + 5, FIRST + 6]
    # The update_where touched what was left and returned it.
    touched = result.outcomes[4].written
    assert touched.affected == 4
    assert all(row.get("note") == "touched" for row in touched.rows)
    assert _mine(client) == [FIRST + n for n in (1, 2, 3, 4)]


def test_a_batch_may_span_tables(client: Client) -> None:
    """Each operation names its own table, so its returned rows decode against
    the right one — a single table on the result would be wrong for all but the
    first."""
    from .fixture import USERS

    b = Batch(Atomicity.INDEPENDENT)
    b.insert(DOCS, [_row(0)])
    # `users` is keyed on (tenant_id, id), so both go in the key. The client
    # checks the arity before sending, which is how the first draft of this
    # test was caught.
    b.delete(USERS, [[u64(1), u64(9_999_999)]])  # not there; a delete of nothing
    result = client.batch(b)

    assert [one.ok for one in result.outcomes] == [True, True]
    assert result.outcomes[0].written.affected == 1
    assert result.outcomes[1].written.affected == 0


def test_an_atomic_batch_joins_an_open_transaction(client: Client) -> None:
    with pytest.raises(RuntimeError, match="rolled back on purpose"):
        with client.transaction() as txn:
            b = Batch(Atomicity.ALL_OR_NOTHING)
            for n in range(3):
                b.insert(DOCS, [_row(n)])
            result = txn.batch(b)
            # No sequence: the transaction has not committed, and the batch is
            # not the thing that commits it.
            assert result.sequence is None
            raise RuntimeError("rolled back on purpose")

    assert _mine(client) == [], "the rollback undid it"


def test_an_independent_batch_may_not_run_inside_a_transaction(client: Client) -> None:
    """"Independent operations, all of which roll back together" is two
    contradictory requests."""
    with client.transaction() as txn:
        b = Batch(Atomicity.INDEPENDENT)
        b.insert(DOCS, [_row(0)])
        with pytest.raises(InvalidRequest):
            txn.batch(b)
        txn.rollback()
