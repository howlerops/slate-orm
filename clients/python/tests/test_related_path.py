"""`Session.related_path`: a path of relationships, one request, one read per level.

`slate-orm` has had `load_related_through`, `load_through` and `load_nested`
since P4, and every one of them was reachable only from Rust. A caller of this
SDK who wanted the copies on a library's shelves issued two `related` calls and
regrouped by hand — which is the same shape as the N+1 this whole layer exists
to prevent, one level up.

What is established here, beyond "the rows come back":

**Two levels, two reads, whatever the number of parents.** Counted at the
transport, because that is the entire claim. A version that looped per parent
would return exactly the same rows.

**The tree is rebuilt from the levels the server sends, not from the schema.**
The client holds no catalog; the server says, per level, which ordinal of the
level above carries the key. An SDK that guessed instead would be right for
this fixture and wrong for a schema whose columns sit in another order.

**The depth is bounded.** `load_nested` needs no limit because its depth is a
type parameter, fixed at compile time. A path's depth arrives in the request,
one step is one read, so it is bounded — and refused by name when exceeded.
"""

from __future__ import annotations

import pytest

from slate import Client, Step, u64
from slate.errors import InvalidRequest

from .conftest import Serving, connect
from .fixture import COPIES, LIBRARIES, SHELVES


@pytest.fixture(scope="module", autouse=True)
def _seeded(server: Serving) -> None:
    client = connect(server)
    session = client.session()
    with session.transaction():
        session.insert(
            LIBRARIES,
            [[u64(1), u64(210), "Path Main"], [u64(1), u64(211), "Path Annexe"]],
        )
        session.insert(
            SHELVES,
            [
                [u64(1), u64(300), u64(210), "alpha"],
                [u64(1), u64(301), u64(210), "beta"],
                [u64(1), u64(302), u64(211), "gamma"],
                # A shelf with no copies at all, so the middle level has a row
                # whose own level below is empty.
                [u64(1), u64(303), u64(211), "bare"],
            ],
        )
        session.insert(
            COPIES,
            [
                [u64(1), u64(400), u64(300), "a-1"],
                [u64(1), u64(401), u64(300), "a-2"],
                [u64(1), u64(402), u64(301), "b-1"],
                [u64(1), u64(403), u64(302), "g-1"],
            ],
        )


def path() -> list[Step]:
    """`libraries → shelves → copies`, both steps downward."""
    return [
        Step(on=SHELVES, through="shelf_library", table=SHELVES),
        Step(on=COPIES, through="copy_shelf", table=COPIES),
    ]


def test_a_path_returns_a_tree_per_parent(client: Client) -> None:
    session = client.session()

    trees = session.related_path([u64(210), u64(211)], path())

    assert [[node.row.get("label") for node in tree] for tree in trees] == [
        ["alpha", "beta"],
        ["gamma", "bare"],
    ]
    # And each shelf carries its own copies, which is the nesting.
    assert [
        [[leaf.row.get("barcode") for leaf in node.related] for node in tree]
        for tree in trees
    ] == [[["a-1", "a-2"], ["b-1"]], [["g-1"], []]]


def test_a_parent_with_nothing_related_gets_an_empty_tree(client: Client) -> None:
    session = client.session()
    # Library 12 is seeded by `test_related`'s module fixture and has no
    # shelves. An absent key is an empty list, not a missing entry, so the
    # result stays indexable by the caller's own loop counter.
    trees = session.related_path([u64(210), u64(12)], path())
    assert len(trees) == 2
    assert trees[1] == []


def test_related_through_drops_the_middle(client: Client) -> None:
    session = client.session()
    through = session.related_through([u64(210), u64(211)], path())
    assert [[row.get("barcode") for row in rows] for rows in through] == [
        ["a-1", "a-2", "b-1"],
        ["g-1"],
    ]


def test_a_whole_path_is_one_request_however_many_parents(client: Client) -> None:
    """The whole claim, counted at the stub rather than inferred.

    Fifty parents across two levels cost **one** request. A client resolving
    the path itself would issue one per level at best and one per intermediate
    row at worst, and both return exactly the same rows — which is why only a
    count can tell them apart. The reads the server performs are one per level;
    what this pins is that the client makes one call.
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
        many = [u64(210 + (n % 2)) for n in range(50)]
        trees = session.related_path(many, path())
    finally:
        client._conn.stub.Related = original  # type: ignore[method-assign]  # noqa: SLF001

    assert calls == 1, f"{len(many)} parents over two levels cost {calls} requests"
    assert len(trees) == len(many)
    assert [node.row.get("label") for node in trees[0]] == ["alpha", "beta"]


def test_a_path_past_the_depth_limit_is_refused_by_name(client: Client) -> None:
    session = client.session()
    # The shipped limit is four. Five steps of the same two relationships do
    # not compose, but the depth is checked first — deliberately, because the
    # work the limit bounds is the resolution that would otherwise happen.
    deep = path() * 3
    with pytest.raises(InvalidRequest) as refused:
        session.related_path([u64(210)], deep)
    assert "max_relation_depth" in str(refused.value)


def test_an_empty_path_is_refused_by_the_client(client: Client) -> None:
    session = client.session()
    # Refused here rather than at the server: a path of no steps has no answer
    # shape at all, and the round trip would only confirm it.
    with pytest.raises(InvalidRequest):
        session.related_path([u64(210)], [])
