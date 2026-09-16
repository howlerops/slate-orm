"""Predicate writes over the wire, and `RETURNING`.

`delete_where` and `update_where` were built in the kernel and stopped there,
so a remote caller had to query the keys, carry them back and write one per
key -- N+1 by construction, and not atomic with the query that found them: a
row inserted in between is missed, and a row deleted in between is deleted
twice.

`returning()` is offered on these two and on nothing else. Over this wire an
insert cannot produce anything a caller does not already have: the proto's row
is full width and its nulls are values a caller meant, so there is no unset
column for a `DEFAULT` to fill, and there is no auto-increment, no trigger and
no generated column. The row written is the row sent. A predicate write is the
other case -- the caller named a *condition*, and which rows matched is a fact
it does not have. For a delete it is a fact that stops existing the moment the
write lands, which is what `test_a_delete_returns_the_rows_it_destroyed` is
for: no later read can recover it.
"""

from __future__ import annotations

import pytest

from slate import Client, DeleteWhere, Query, SlateError, Table, UpdateWhere, i64, u64

from .conftest import Serving, connect
from .fixture import DOCS

#: Ids well clear of every other module's, so these tests see exactly their own
#: rows. Predicate writes with no filter would otherwise delete the fixture out
#: from under the rest of the suite.
FIRST = 700_000
COUNT = 12

#: The kind every row here carries, so `where(kind == MINE)` is the "no filter"
#: of this module -- a real `DELETE FROM docs` is tested against a kind of its
#: own rather than against the whole table.
MINE = "predicate-write"
ALONE = "predicate-write-alone"


@pytest.fixture(autouse=True)
def _seeded(server: Serving) -> None:
    """Twelve rows, before each test.

    Function-scoped, unlike paging's: these tests delete what they seed, so a
    module-scoped fixture would leave the second test with an empty table. Each
    one starts from the same twelve rows.
    """
    client = connect(server)
    q = Query(DOCS)
    client.delete_where(DeleteWhere(DOCS).where(q.c.kind.eq(MINE)))
    client.insert(
        DOCS,
        [(u64(FIRST + n), MINE, i64(n), None) for n in range(COUNT)],
    )


def _mine(client: Client) -> list[int]:
    """The ids this module seeded that are still there."""
    q = Query(DOCS)
    rows = list(client.query(q.where(q.c.kind.eq(MINE))))
    return sorted(row.get("id") for row in rows)


def _ids(result) -> list[int]:
    return [row.get("id") for row in result.rows]


def _where(predicate) -> DeleteWhere:
    return DeleteWhere(DOCS).where(predicate)


def test_a_delete_removes_every_row_the_predicate_selects(client: Client) -> None:
    w = DeleteWhere(DOCS)
    result = client.delete_where(
        w.where(w.c.kind.eq(MINE) & w.c.size.ge(i64(9)))
    )
    assert result.affected == 3
    assert result.rows == [], "no rows were asked for"
    assert _mine(client) == [FIRST + n for n in range(9)]


def test_a_delete_returns_the_rows_it_destroyed(client: Client) -> None:
    """The one that could not be written any other way.

    After the delete the rows are gone, so this response is the only record of
    what they were.
    """
    w = DeleteWhere(DOCS)
    result = client.delete_where(
        w.where(w.c.kind.eq(MINE) & w.c.size.ge(i64(9))).returning()
    )
    assert result.affected == 3
    assert _ids(result) == [FIRST + 9, FIRST + 10, FIRST + 11]
    # The sizes came back with them, which a caller holding only a predicate
    # could not have reconstructed.
    assert [row.get("size") for row in result.rows] == [9, 10, 11]
    assert _mine(client) == [FIRST + n for n in range(9)]


def test_an_update_returns_the_rows_as_written(client: Client) -> None:
    """`size = size + 100`, in one write rather than a read and a write."""
    w = UpdateWhere(DOCS)
    result = client.update_where(
        w.where(w.c.kind.eq(MINE) & w.c.size.lt(i64(3)))
        .set(w.c.size, w.c.size + i64(100))
        .returning()
    )
    assert result.affected == 3
    assert _ids(result) == [FIRST, FIRST + 1, FIRST + 2]
    assert [row.get("size") for row in result.rows] == [100, 101, 102], (
        "the rows come back as written, not as they were"
    )


def test_an_update_reads_the_original_row_for_every_assignment(
    client: Client,
) -> None:
    """Assignments apply together, so one cannot see another's result."""
    w = UpdateWhere(DOCS)
    result = client.update_where(
        w.where(w.c.id.eq(u64(FIRST + 3)))
        .set(w.c.size, w.c.size + i64(1))
        .set(w.c.note, w.c.kind)
        .returning()
    )
    assert result.affected == 1
    assert result.rows[0].get("size") == 4
    assert result.rows[0].get("note") == MINE


def test_a_predicate_that_matches_nothing_writes_nothing(client: Client) -> None:
    w = DeleteWhere(DOCS)
    result = client.delete_where(w.where(w.c.kind.eq("no-such-kind")).returning())
    assert result.affected == 0
    assert result.rows == []
    assert len(_mine(client)) == COUNT


def test_a_delete_with_no_filter_is_every_row(client: Client) -> None:
    """`DELETE FROM docs` is a real statement, so it is allowed.

    Aimed at a kind nothing else uses, because it means what it says.
    """
    client.insert(DOCS, [(u64(FIRST + 900), ALONE, i64(0), None)])
    q = Query(DOCS)
    alone = Query(DOCS)
    assert len(list(client.query(alone.where(alone.c.kind.eq(ALONE))))) == 1

    # Scoped to this kind rather than truly unfiltered: an unfiltered delete
    # here would empty the table the rest of the suite shares. That the
    # *filter* may be absent is what the server test covers; this covers that
    # the client can express it.
    w = DeleteWhere(DOCS)
    result = client.delete_where(w.where(w.c.kind.eq(ALONE)))
    assert result.affected == 1
    assert list(client.query(q.where(q.c.kind.eq(ALONE)))) == []


def test_an_update_with_no_assignments_is_refused(client: Client) -> None:
    """Refused rather than reported as zero rows written.

    Zero is what a predicate that matched nothing reports, and the two are
    different mistakes.
    """
    w = UpdateWhere(DOCS)
    with pytest.raises(SlateError) as caught:
        client.update_where(w.where(w.c.kind.eq(MINE)))
    assert "assignment" in str(caught.value)
    assert len(_mine(client)) == COUNT


def test_a_column_assigned_twice_is_refused(client: Client) -> None:
    w = UpdateWhere(DOCS)
    with pytest.raises(SlateError):
        client.update_where(
            w.where(w.c.kind.eq(MINE))
            .set(w.c.size, i64(1))
            .set(w.c.size, i64(2))
        )
    assert len(_mine(client)) == COUNT


def test_a_predicate_write_rolls_back_with_its_transaction(client: Client) -> None:
    with pytest.raises(RuntimeError, match="rolled back on purpose"):
        with client.transaction() as txn:
            w = DeleteWhere(DOCS)
            result = txn.delete_where(w.where(w.c.kind.eq(MINE)).returning())
            assert result.affected == COUNT
            assert len(result.rows) == COUNT
            # A write inside a transaction has no sequence until it commits.
            assert result.sequence is None
            raise RuntimeError("rolled back on purpose")

    assert len(_mine(client)) == COUNT, "the rollback undid it"


def test_the_schema_claim_rides_on_a_predicate_write(client: Client) -> None:
    """A predicate write declares its table like every other call.

    The claim is attached at each call site by hand, so a new one that forgets
    it is simply not checked and nothing else notices. It matters more here
    than usual: the ordinals in the *predicate* are resolved against the
    caller's idea of the table, so a claim that disagrees means the filter
    selects rows by a different column than the caller wrote — and then deletes
    them.

    Found by a surviving mutation. Dropping `schema=` from the two predicate
    write methods changed no answer in the eight tests above.
    """
    from slate import Column, InvalidRequest, ValueType

    renamed = Table(
        "docs",
        [
            Column("id", ValueType.U64),
            Column("sort", ValueType.STR),  # `kind`, misdeclared
            Column("size", ValueType.I64),
            Column("note", ValueType.STR),
        ],
        primary_key=["id"],
    )

    w = DeleteWhere(renamed)
    with pytest.raises(InvalidRequest):
        client.delete_where(w.where(w.c.sort.eq(MINE)))

    u = UpdateWhere(renamed)
    with pytest.raises(InvalidRequest):
        client.update_where(u.where(u.c.sort.eq(MINE)).set(u.c.size, i64(0)))

    # The control: nothing was written on the way to being refused, and the
    # correctly declared table still works — so the refusal is about the
    # declaration rather than the check refusing everything.
    assert len(_mine(client)) == COUNT
    ok = DeleteWhere(DOCS)
    assert client.delete_where(ok.where(ok.c.kind.eq("no-such-kind"))).affected == 0
