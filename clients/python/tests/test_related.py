"""`Session.related`: one relationship, many parents, one read.

The N+1 this replaces is the reason the Rust record layer has `load_related`,
and until now that was reachable only from Rust. What is being established here
is not that a relationship can be fetched — a join already did that — but three
things a join does not give:

**One read, whatever the number of parents.** Asserted by counting requests at
the transport, because "one read, not N" is the entire claim and a claim
nothing measures is a claim nothing keeps. A version that looped would return
the same rows.

**Parents sharing a key share a group.** Two shelves in one library send the
same value twice and the server reads it once; the response carries one group
that both map onto, so the saving is on the response as well as the read.

**The relationship is named, not described.** The client says
`through="shelf_library"` and holds no idea which columns that is — the server
resolves it against the foreign key the catalog already declares. A client that
described the relationship could describe it differently from the next client,
which is the divergence the conformance runner exists to catch.
"""

from __future__ import annotations

import pytest

from slate import Client, Identity, Query, u64
from slate.errors import PermissionDenied

from .conftest import Serving, connect
from .fixture import LIBRARIES, SHELVES


# Seeded once, not per test. The server fixture is session-scoped, so a
# per-test insert of the same rows collides with itself on the second test —
# which is a fact about the fixture and not about relationships.
@pytest.fixture(scope="module", autouse=True)
def _seeded(server: Serving) -> None:
    client = connect(server)
    session = client.session()
    with session.transaction():
        session.insert(
            LIBRARIES,
            [
                [u64(1), u64(10), "Main"],
                [u64(1), u64(11), "Annexe"],
                [u64(1), u64(12), "Empty"],
            ],
        )
        session.insert(
            SHELVES,
            [
                [u64(1), u64(100), u64(10), "history"],
                [u64(1), u64(101), u64(10), "poetry"],
                [u64(1), u64(102), u64(11), "maps"],
            ],
        )


def _labels(groups: list[list]) -> list[list[str]]:
    return [[str(row.get("label")) for row in group] for group in groups]


def test_children_come_back_grouped_per_parent(client: Client) -> None:
    session = client.session()

    got = session.related(
        SHELVES,
        [u64(10), u64(11), u64(12)],
        through="shelf_library",
        on=SHELVES,
    )

    assert _labels(got) == [["history", "poetry"], ["maps"], []]


def test_a_parent_with_nothing_related_gets_an_empty_list(client: Client) -> None:
    """Not a missing entry: the caller indexes this by its own loop counter."""
    session = client.session()

    got = session.related(
        SHELVES, [u64(12), u64(10)], through="shelf_library", on=SHELVES
    )

    assert got[0] == []
    assert len(got[1]) == 2
    assert len(got) == 2


def test_parents_reads_the_other_direction(client: Client) -> None:
    """A shelf's library, rather than a library's shelves."""
    session = client.session()

    got = session.related(
        LIBRARIES,
        [u64(10), u64(11)],
        through="shelf_library",
        on=SHELVES,
        children=False,
    )

    assert [[str(row.get("name")) for row in group] for group in got] == [
        ["Main"],
        ["Annexe"],
    ]


def test_repeated_keys_share_one_group(client: Client) -> None:
    """Two parents with the same key both get the rows, from one group."""
    session = client.session()

    got = session.related(
        SHELVES,
        [u64(10), u64(10), u64(11)],
        through="shelf_library",
        on=SHELVES,
    )

    assert _labels(got) == [["history", "poetry"], ["history", "poetry"], ["maps"]]


def test_it_is_one_request_however_many_parents(client: Client) -> None:
    """The claim, measured.

    Counted at the stub rather than inferred: a loop over `query` returns
    exactly the same rows, and nothing about the answer distinguishes the two.
    """
    session = client.session()

    calls = 0
    original = client._conn.stub.Related  # noqa: SLF001

    def counting(*args: object, **kwargs: object) -> object:
        nonlocal calls
        calls += 1
        return original(*args, **kwargs)

    client._conn.stub.Related = counting  # type: ignore[method-assign]  # noqa: SLF001
    try:
        many = [u64(10 + (n % 2)) for n in range(50)]
        got = session.related(SHELVES, many, through="shelf_library", on=SHELVES)
    finally:
        client._conn.stub.Related = original  # type: ignore[method-assign]  # noqa: SLF001

    assert calls == 1, f"{len(many)} parents cost {calls} requests"
    assert len(got) == len(many)
    assert _labels(got)[0] == ["history", "poetry"]


def test_no_parents_is_no_request_and_no_rows(client: Client) -> None:
    session = client.session()
    assert session.related(SHELVES, [], through="shelf_library", on=SHELVES) == []


def test_an_unknown_foreign_key_names_the_ones_that_exist(client: Client) -> None:
    session = client.session()
    with pytest.raises(Exception) as caught:
        session.related(SHELVES, [u64(10)], through="nosuch", on=SHELVES)
    assert "shelf_library" in str(caught.value)


def test_the_rows_agree_with_the_same_question_asked_as_a_query(
    client: Client,
) -> None:
    """The oracle: one relationship load equals two ordinary reads.

    Not a restatement of the seed data — a second, independent way of getting
    the same answer, so a `related` that silently dropped or duplicated a row
    disagrees with it.
    """
    session = client.session()

    related = session.related(
        SHELVES, [u64(10), u64(11), u64(12)], through="shelf_library", on=SHELVES
    )

    by_hand = []
    for library in (10, 11, 12):
        query = Query(SHELVES)
        query.where(query.c.library_id.eq(u64(library)))
        by_hand.append(sorted(str(row.get("label")) for row in session.query(query)))

    assert [sorted(group) for group in _labels(related)] == by_hand


def test_a_caller_without_the_grant_is_refused(server: Serving) -> None:
    """The read is the caller's read, not the server's.

    This exists because a mutation swapping `authorized_table` for the plain
    lookup broke nothing: every other test in this file connects as `app`,
    which holds a grant on both tables, so the authorised and unauthorised
    paths were indistinguishable. `reader` is granted only on `docs`.
    """
    guest = connect(server, Identity("u64:1", tenant="u64:1", roles=["reader"]))
    try:
        session = guest.session()
        with pytest.raises(PermissionDenied):
            session.related(SHELVES, [u64(10)], through="shelf_library", on=SHELVES)
    finally:
        guest.close()
