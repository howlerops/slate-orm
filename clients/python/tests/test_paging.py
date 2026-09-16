"""Keyset pagination over the wire.

`Query.after` has existed in the kernel for a while and had no field in the
proto, so every remote caller paged by offset -- which is correct only while
nothing changes. The property that matters, and the one `test_paging_...`
below is for, is that paging visits every row exactly once even when rows are
being written behind the cursor. `OFFSET` cannot promise that: it counts rows,
so a delete behind the window makes the reader skip one it has not seen, and an
insert makes it see one twice, and nothing reports either.

The server builds the cursor rather than the caller pulling the key out of the
last row. Paging by hand means knowing which columns are the primary key and in
what order, on every call site -- and a cursor built from the wrong column
still pages, just through the wrong sequence. The cost of that choice is a
refusal the caller has to understand, and
`test_a_projection_that_drops_the_key_is_refused` is it.
"""

from __future__ import annotations

import pytest

from slate import Client, Query, SlateError, i64, u64

from .conftest import Serving, connect
from .fixture import DOCS

#: A hard stop on every paging loop below.
#:
#: A bug in the cursor is exactly a loop that never ends -- a server that
#: returned one for a short page would page forever, and a test that hangs
#: reports nothing. One such mutation hung a mutation run for thirty minutes
#: before the harness gave up. Twenty-five rows in pages of five is six
#: requests; twenty is room for a different fixture and not for a loop.
LOOP_BOUND = 20

#: Ids well clear of what every other module seeds, so paging sees exactly
#: these rows and the fixture's own docs do not drift the counts.
FIRST = 500_000
COUNT = 25


@pytest.fixture(scope="module", autouse=True)
def _seeded(server: Serving) -> None:
    """Twenty-five rows, once for the module.

    Module-scoped because the server is session-scoped: seeding per test would
    collide on the second insert, and clearing between tests would make the
    concurrent-writes test unable to tell its own deletes apart from a reset.
    """
    client = connect(server)
    client.insert(
        DOCS,
        [(u64(FIRST + n), "page", i64(n), None) for n in range(COUNT)],
    )


def _page_query(limit: int, cursor: list | None = None) -> Query:
    q = Query(DOCS)
    return q.where(q.c.kind.eq("page")).limit(limit).after(cursor)


def _ids(page) -> list[int]:
    return [row.get("id") for row in page.rows]


def test_a_full_page_carries_a_cursor(client: Client) -> None:
    page = client.page(_page_query(10))
    assert _ids(page) == [FIRST + n for n in range(10)]
    assert page.cursor is not None
    assert not page.is_last


def test_a_short_page_ends_the_sequence(client: Client) -> None:
    page = client.page(_page_query(1000))
    assert len(page.rows) == COUNT
    assert page.cursor is None
    assert page.is_last


def test_the_cursor_resumes_strictly_after_the_last_row(client: Client) -> None:
    first = client.page(_page_query(10))
    second = client.page(_page_query(10, first.cursor))
    assert _ids(second) == [FIRST + n for n in range(10, 20)]
    # Strictly after: no row appears on both pages.
    assert not set(_ids(first)) & set(_ids(second))


def test_paging_reaches_every_row_and_then_stops(client: Client) -> None:
    seen: list[int] = []
    cursor = None
    requests = 0
    while requests < LOOP_BOUND:
        page = client.page(_page_query(10, cursor))
        requests += 1
        seen.extend(_ids(page))
        if page.is_last:
            break
        cursor = page.cursor
    assert seen == [FIRST + n for n in range(COUNT)]
    # 25 rows in pages of ten: 10, 10, 5. The third is short, so it ends the
    # sequence and there is no fourth -- the empty final request only happens
    # when the row count divides evenly, which is what the next test covers.
    assert requests == 3


def test_a_page_that_divides_evenly_costs_one_empty_request(client: Client) -> None:
    """The documented cost of not reading one row ahead on every page."""
    seen: list[int] = []
    cursor = None
    requests = 0
    while requests < LOOP_BOUND:
        page = client.page(_page_query(5, cursor))
        requests += 1
        seen.extend(_ids(page))
        if page.is_last:
            break
        cursor = page.cursor
    assert seen == [FIRST + n for n in range(COUNT)]
    # 25 rows in pages of five is five full pages, then one empty.
    assert requests == 6


#: A second block of rows, for the one test that deletes as it pages.
#: Its own range because the deletes would otherwise change what every other
#: test in this module sees, and a test whose meaning depends on running before
#: another one is a test that breaks the day somebody reorders the file.
CHURN_FIRST = 600_000


def test_paging_visits_every_row_exactly_once_under_concurrent_writes(
    client: Client,
) -> None:
    """The property `OFFSET` cannot offer.

    Between pages, a row *behind* the cursor is deleted. Under offset-based
    paging that shifts the window and the reader silently skips a row it has
    not seen. A key does not move when its neighbours change.
    """
    client.insert(
        DOCS,
        [(u64(CHURN_FIRST + n), "churn", i64(n), None) for n in range(COUNT)],
    )
    q = Query(DOCS)
    base = q.where(q.c.kind.eq("churn")).limit(5)

    seen: list[int] = []
    deleted = 0
    cursor = None
    for round_ in range(LOOP_BOUND):
        assert round_ < LOOP_BOUND - 1, f"paging did not terminate: {seen}"
        page = client.page(base.after(cursor))
        seen.extend(_ids(page))
        if page.is_last:
            break
        cursor = page.cursor
        # A *different* row each round, all of them behind the cursor and
        # already in `seen`, so anything re-read shows up as a duplicate
        # below. Deleting the same row twice would refuse rather than churn.
        client.delete(DOCS, [(u64(seen[deleted]),)])
        deleted += 1

    assert deleted > 0, "the churn never happened, so this asserts nothing"
    assert len(seen) == len(set(seen)), f"a row was visited twice: {seen}"
    assert seen == [CHURN_FIRST + n for n in range(COUNT)]


def test_a_page_with_no_limit_is_refused(client: Client) -> None:
    q = Query(DOCS)
    with pytest.raises(SlateError) as caught:
        client.page(q.where(q.c.kind.eq("page")))
    assert "limit" in str(caught.value)


def test_a_projection_that_drops_the_key_is_refused(client: Client) -> None:
    """Refused rather than served without a cursor.

    Serving it would be the silent version: the caller loops until the cursor
    is empty, gets none on the first page, and reads ten rows of the table as
    the whole answer.
    """
    q = Query(DOCS)
    paged = q.where(q.c.kind.eq("page")).limit(5).select(q.c.kind, q.c.size)
    with pytest.raises(SlateError) as caught:
        client.page(paged)
    assert "id" in str(caught.value)

    # And the same projection reads fine when nothing is paging.
    rows = list(client.query(paged))
    assert len(rows) == 5


def test_a_cursor_of_the_wrong_width_is_refused_before_the_round_trip(
    client: Client,
) -> None:
    """The client knows the key's width, so this costs no request."""
    with pytest.raises(ValueError) as caught:
        _page_query(5, [u64(1), u64(2)]).to_proto()
    assert "primary key" in str(caught.value)


def test_a_cursor_alone_pages_without_asking_for_one_back(client: Client) -> None:
    """`after` without `paged`, for a caller keeping its own key.

    `query` is unchanged by any of this: it sends no `paged`, gets no cursor,
    and honours `after` exactly as the kernel does.
    """
    q = Query(DOCS)
    rows = list(
        client.query(
            q.where(q.c.kind.eq("page")).limit(3).after([u64(FIRST + 9)])
        )
    )
    assert [r.get("id") for r in rows] == [FIRST + n for n in (10, 11, 12)]
