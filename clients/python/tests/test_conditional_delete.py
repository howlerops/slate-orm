"""A conditional delete: `delete(..., expected=...)`.

`update(..., expected=...)` shipped first because overwriting somebody else's
edit is the loss everybody recognises. Deleting a row somebody else just edited
is the same mistake: the caller read the row, decided *from what it said* that
it should go, and by the time the delete lands it says something else. A
moderator deleting a post because it had no views deletes one with five
thousand, and `affected` comes back 1.

# The one place this is not simply the update's twin

A plain delete reports an absent key in `affected`, because "make sure this is
gone" is idempotent. A conditional one raises `NotFound`. The caller said what
it expected to find, so "it was already gone" is an answer it wants rather than
a smaller count it will read as success — and `NotFound` rather than `Conflict`
because a row that moved can be re-read and the decision remade, where a row
that is gone cannot, so a caller retrying a `Conflict` would loop.
"""

from __future__ import annotations

import pytest

from slate import (
    Client,
    DeleteWhere,
    NotFound,
    PyValue,
    Query,
    SlateError,
    i64,
    u64,
)

from .conftest import as_int
from .fixture import DOCS

#: Ids of this module's own, clear of every other module's.
#:
#: 900_000 and not 500_000, which is what this started at and is `test_paging`'s
#: range. The clash only showed on a *full* run — this module's cleanup filters
#: on its own `kind`, so it could not remove paging's rows and the insert
#: collided. Passing alone and failing together is what a "flake" usually is.
FIRST = 900_000
KIND = "conditional-delete"


@pytest.fixture
def seeded(client: Client) -> Client:
    """Four docs, ids FIRST..FIRST+3, size = n, before each test."""
    q = Query(DOCS)
    client.delete_where(_mine())
    client.insert(
        DOCS,
        [(u64(FIRST + n), KIND, i64(n), None) for n in range(4)],
    )
    assert len(list(client.query(q.where(q.c.kind.eq(KIND))))) == 4
    return client


def _mine() -> DeleteWhere:
    """Everything this module seeded, by its own kind.

    A predicate delete rather than a key list: these tests delete what they
    seed, so a re-run has to start from whatever the last one left — and a
    `DELETE FROM docs` with no filter would take the rest of the suite's rows
    with it.
    """
    w = DeleteWhere(DOCS)
    return w.where(w.c.kind.eq(KIND))


def _row(n: int, size: int | None = None) -> list[PyValue]:
    """The whole row, as a caller would have read it."""
    return [u64(FIRST + n), KIND, i64(n if size is None else size), None]


def _present(client: Client, n: int) -> bool:
    return client.get(DOCS, (u64(FIRST + n),)) is not None


def test_a_delete_naming_the_row_it_read_is_applied(seeded: Client) -> None:
    result = seeded.delete(DOCS, [(u64(FIRST + 2),)], expected=[_row(2)])
    assert result.affected == 1
    assert not _present(seeded, 2)


def test_a_delete_naming_a_row_that_moved_is_refused(seeded: Client) -> None:
    # Somebody else's edit lands between this caller's read and its delete.
    seeded.update(DOCS, [_row(2, size=5_000)])

    with pytest.raises(SlateError):
        seeded.delete(DOCS, [(u64(FIRST + 2),)], expected=[_row(2)])
    assert _present(seeded, 2), "a refused conditional delete removed the row"
    row = seeded.get(DOCS, (u64(FIRST + 2),))
    assert row is not None
    assert as_int(row.get("size")) == 5_000


def test_a_row_already_gone_is_refused_rather_than_counted_as_absent(
    seeded: Client,
) -> None:
    """Both halves, because the refusal only means something beside what it
    replaces."""
    seeded.delete(DOCS, [(u64(FIRST + 3),)])

    plain = seeded.delete(DOCS, [(u64(FIRST + 3),)])
    assert plain.affected == 0, "a plain delete of an absent key is not an error"

    with pytest.raises(NotFound):
        seeded.delete(DOCS, [(u64(FIRST + 3),)], expected=[_row(3)])


def test_one_stale_row_refuses_the_whole_statement(seeded: Client) -> None:
    # The stale row first, so a loop that reported the last outcome would
    # answer with a success.
    seeded.update(DOCS, [_row(0, size=99)])

    with pytest.raises(SlateError):
        seeded.delete(
            DOCS,
            [(u64(FIRST + 0),), (u64(FIRST + 1),)],
            expected=[_row(0), _row(1)],
        )
    assert _present(seeded, 0)
    assert _present(seeded, 1), "the second row was deleted by a refused statement"


def test_a_short_expected_is_refused_by_the_client(seeded: Client) -> None:
    with pytest.raises(ValueError) as raised:
        seeded.delete(
            DOCS,
            [(u64(FIRST + 0),), (u64(FIRST + 1),)],
            expected=[_row(0)],
        )
    assert "2 key(s) and 1 expected" in str(raised.value)
    assert _present(seeded, 1)


def test_no_expected_is_an_ordinary_delete(seeded: Client) -> None:
    result = seeded.delete(DOCS, [(u64(FIRST + 0),), (u64(FIRST + 1),)], expected=None)
    assert result.affected == 2
    assert not _present(seeded, 0)
    assert not _present(seeded, 1)


def test_a_conditional_delete_inside_a_transaction(seeded: Client) -> None:
    with seeded.transaction() as txn:
        txn.delete(DOCS, [(u64(FIRST + 2),)], expected=[_row(2)])
    assert not _present(seeded, 2)


def test_a_stale_conditional_delete_inside_a_transaction_is_refused(
    seeded: Client,
) -> None:
    # The session path is separate code from the autocommit one, and only a
    # *stale* row tells a conditional delete apart from a plain one.
    seeded.update(DOCS, [_row(2, size=77)])
    with pytest.raises(SlateError), seeded.transaction() as txn:
        txn.delete(DOCS, [(u64(FIRST + 2),)], expected=[_row(2)])
    assert _present(seeded, 2)
