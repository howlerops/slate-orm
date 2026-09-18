"""`Session.page_join`: keyset paging over a join and over a chain.

A page of a join is a page of its *driving table*. The cursor is input 0's
primary key, `limit` counts input-0 rows, and every joined row those rows
produce comes back with them. `docs/paging-a-join.md` has the design and the
measurements that chose it; what is established here is what a client sees.

**The property, which is the point.** Paging through visits every joined row
exactly once, even while rows are being inserted behind the cursor. That is the
thing `offset` does not do: it counts, so an insert ahead of the cursor shifts
every later page by one and a row is served twice or skipped, with nothing
reporting either.

**A page is bounded in input-0 rows, not in returned rows.** The fixture has
fan-out on purpose, so a page of two authors is more than two rows and a test
that confused the two would fail rather than pass by coincidence.

**A page whose input-0 rows match nothing still advances.** Under an inner join
those rows are read and return nothing, so a cursor taken from the *returned*
rows would not move — and the caller would ask for the same page forever.
"""

from __future__ import annotations

import pytest

from slate import Client, JoinQuery, JoinType, u64
from slate.errors import InvalidRequest

from .conftest import Serving, connect
from .fixture import AUTHORS, BOOKS

def ids(row: object, inputs: int) -> tuple[int, ...]:
    """Each input's `id`, which is column 1 of a tenant-scoped key.

    A `JoinedRow` is a sequence of its inputs' rows, so `row[n]` is input n —
    and it is `None` where an outer join preserved a row that matched nothing.
    These fixtures are inner joins, so a `None` here is a bug rather than a
    case, and it is asserted rather than silently indexed.
    """
    out = []
    for at in range(inputs):
        inner = row[at]  # type: ignore[index]
        assert inner is not None, f"input {at} was absent from an inner join"
        out.append(inner[1])
    return tuple(out)


TENANT = 1
# Authors 700..706, each with a differing number of books, and 706 with none.
FIRST, LAST = 700, 706


@pytest.fixture(scope="module", autouse=True)
def _seeded(server: Serving) -> None:
    client = connect(server)
    session = client.session()
    with session.transaction():
        session.insert(
            AUTHORS,
            [
                [u64(TENANT), u64(a), u64(1), f"paged-{a}", "uk", 1900]
                for a in range(FIRST, LAST + 1)
            ],
        )
        session.insert(
            BOOKS,
            [
                [u64(TENANT), u64(9000 + n), u64(author), f"b-{n}", 2000]
                # Uneven fan-out: author 700 gets one book, 701 two, and so on
                # up to 705; 706 gets none. So a page boundary lands inside a
                # group, and one input-0 row matches nothing.
                for n, author in enumerate(
                    a for a in range(FIRST, LAST) for _ in range(a - FIRST + 1)
                )
            ],
        )
    client.close()


def paged(cursor: list[object] | None, size: int) -> JoinQuery:
    """Authors joined to their books, restricted to this test's authors."""
    join = JoinQuery()
    authors = join.add(AUTHORS)
    books = join.add(BOOKS, on=[(authors.c.id, "author_id")], join_type=JoinType.INNER)
    authors.where(
        (authors.c.id >= u64(FIRST)) & (authors.c.id <= u64(LAST))
    )
    _ = books
    return join.limit(size).after(cursor)


def walk(session: object, size: int, between: object = None) -> list[tuple[int, int]]:
    """Every page, start to finish, as (author id, book id) pairs."""
    seen: list[tuple[int, int]] = []
    cursor: list[object] | None = None
    for round_ in range(100):
        page = session.page_join(paged(cursor, size))  # type: ignore[attr-defined]
        for row in page.rows:
            seen.append(ids(row, 2))
        if page.is_last:
            break
        cursor = page.cursor
        if between is not None:
            between(round_)
    else:  # pragma: no cover - a stuck cursor is a bug, not a slow test
        pytest.fail("the walk did not terminate: the cursor is stuck")
    return seen


def test_paging_a_join_reproduces_the_whole_join(server: Serving) -> None:
    client = connect(server)
    session = client.session()
    whole = {
        ids(row, 2)
        for row in session.join(paged(None, 10_000).limit(None).after(None))
    }
    for size in (1, 2, 3, 5):
        assert sorted(walk(session, size)) == sorted(whole), (
            f"pages of {size} input-0 rows did not reproduce the join"
        )
    client.close()


def test_a_page_is_bounded_in_input_zero_rows_not_in_returned_rows(
    server: Serving,
) -> None:
    """The overshoot is the design, stated rather than discovered.

    A page of three authors whose fan-outs are 1, 2 and 3 is six rows. A test
    asserting `len(page.rows) <= limit` would pass on a fixture with no fan-out
    and fail here, which is why the fixture has one.
    """
    client = connect(server)
    session = client.session()
    page = session.page_join(paged(None, 3))
    authors = {ids(row, 1)[0] for row in page.rows}
    assert authors == {700, 701, 702}, "a page is three *authors*"
    assert len(page.rows) == 1 + 2 + 3, "and all of their books"
    client.close()


def test_paging_under_concurrent_inserts_visits_each_row_once(
    server: Serving,
) -> None:
    """The property N3 names.

    Books are inserted for author 700 between pages — behind the cursor, which
    is the insertion an offset gets wrong. Rows that existed throughout must be
    seen exactly once, and nothing may be seen twice.
    """
    client = connect(server)
    session = client.session()
    before = {
        ids(row, 2)
        for row in session.join(paged(None, 10_000).limit(None).after(None))
    }

    writer = connect(server)
    writing = writer.session()
    inserted = 0

    def insert_behind(round_: int) -> None:
        nonlocal inserted
        with writing.transaction():
            writing.insert(
                BOOKS,
                [[u64(TENANT), u64(9500 + round_), u64(FIRST), f"late-{round_}", 2020]],
            )
        inserted += 1

    seen = walk(session, 2, insert_behind)
    assert inserted > 0, "the test did not actually write anything between pages"

    counts: dict[tuple[int, int], int] = {}
    for row in seen:
        counts[row] = counts.get(row, 0) + 1
    for row in before:
        assert counts.get(row, 0) == 1, f"{row} existed throughout, seen {counts.get(row, 0)}x"
    assert all(count == 1 for count in counts.values()), "a row was visited twice"
    writer.close()
    client.close()


def test_a_page_whose_authors_have_no_books_still_advances(server: Serving) -> None:
    """Author 706 has no books; an inner join returns nothing for it.

    A cursor taken from the returned rows would not move past it. This walk
    starts at 705 so the last page is exactly that case.
    """
    client = connect(server)
    session = client.session()
    cursor: list[object] | None = [u64(TENANT), u64(LAST - 1)]
    pages = 0
    for _ in range(20):
        page = session.page_join(paged(cursor, 1))
        pages += 1
        if page.is_last:
            break
        assert page.cursor != cursor, "the cursor did not move"
        cursor = page.cursor
    else:  # pragma: no cover
        pytest.fail("the walk did not terminate")
    assert pages >= 2
    client.close()


def test_a_right_outer_join_refuses_a_cursor(server: Serving) -> None:
    client = connect(server)
    session = client.session()
    join = JoinQuery()
    authors = join.add(AUTHORS)
    join.add(BOOKS, on=[(authors.c.id, "author_id")], join_type=JoinType.RIGHT)
    with pytest.raises(InvalidRequest) as caught:
        session.page_join(join.limit(2))
    assert "belong to no page" in str(caught.value)
    client.close()


def test_an_offset_beside_a_cursor_is_refused(server: Serving) -> None:
    client = connect(server)
    session = client.session()
    with pytest.raises(InvalidRequest) as caught:
        session.page_join(paged(None, 2).offset(3))
    assert "Drop the offset" in str(caught.value)
    client.close()


def test_a_page_with_no_size_is_refused(server: Serving) -> None:
    client = connect(server)
    session = client.session()
    with pytest.raises(InvalidRequest) as caught:
        session.page_join(paged(None, 2).limit(None))
    assert "a page needs a size" in str(caught.value)
    client.close()


def test_a_cursor_that_is_not_a_whole_key_is_refused_before_it_is_sent(
    server: Serving,
) -> None:
    """The client checks the shape, so the round trip is not spent on it.

    `authors` has a two-column key, and a one-value cursor is a caller who
    passed the id and forgot the tenant. Typed against **input 0's** key,
    which is the only table a join's cursor names.
    """
    client = connect(server)
    with pytest.raises(ValueError, match="whole primary key of its first input"):
        paged([u64(700)], 2).to_proto()
    client.close()


def test_paging_a_chain_reproduces_the_whole_chain(server: Serving) -> None:
    """Three inputs is a chain, and pages the same way: by the first table."""
    client = connect(server)
    session = client.session()

    def chain(cursor: list[object] | None, size: int | None) -> JoinQuery:
        join = JoinQuery()
        authors = join.add(AUTHORS)
        books = join.add(BOOKS, on=[(authors.c.id, "author_id")])
        # A second pass over books, joined to the first: a three-input chain
        # whose last step reaches back to the step before it.
        join.add(BOOKS, on=[(books.c.id, "id")])
        authors.where((authors.c.id >= u64(FIRST)) & (authors.c.id <= u64(LAST)))
        return join.limit(size).after(cursor)

    whole = sorted(
        (ids(row, 3))
        for row in session.join(chain(None, None))
    )

    seen: list[tuple[int, int, int]] = []
    cursor: list[object] | None = None
    for _ in range(100):
        page = session.page_join(chain(cursor, 2))
        seen.extend(
            (ids(row, 3)) for row in page.rows
        )
        if page.is_last:
            break
        cursor = page.cursor
    else:  # pragma: no cover
        pytest.fail("the walk did not terminate")
    assert sorted(seen) == whole
    client.close()
