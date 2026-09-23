"""An array column, through a real server.

The kernel stores arrays and the wire carries them; this is the Python half of
proving that a list survives the round trip with its elements' *types* intact,
not merely their text. A `["1"]` decoded as strings and a `[1]` decoded as
integers are different rows to this server — its ordering is type-first — and a
test that compared printed forms would call them the same.

Two element types on one table, because a client with one array column can
hard-code the element type it decodes and pass.
"""

from __future__ import annotations

import pytest

from slate import Array, Client, Query, SlateError, i64, u64
from slate.schema import Column, Table, fingerprint_of
from slate.types import ValueType
from slate.values import ValueTypeError, from_value, to_value

from .fixture import POSTS

#: Ids of this module's own, so nothing here depends on another module's rows.
#: `posts` is not seeded by the server, so every row in it is one of these.
FIRST = 500_000


@pytest.fixture
def posted(client: Client) -> Client:
    """A client with three posts, freshly written.

    Not `autouse`, for the reason `test_decimal` gives: the encoding tests
    below need no server, and an autouse fixture would turn each of their
    assertions into a setup error under a mutation — which is the suite
    falling over rather than a named test catching anything.
    """
    client.insert(
        POSTS,
        [
            (u64(FIRST + 1), "first", Array(["rust", "db"]), Array([i64(3), i64(1)])),
            (u64(FIRST + 2), "second", Array(["db"]), None),
            (u64(FIRST + 3), "third", Array([]), Array([])),
        ],
        upsert=True,
    )
    return client


# --- encoding, without a server -------------------------------------------


def test_an_array_encodes_to_the_array_arm() -> None:
    wire = to_value(Array(["a"]))
    assert wire.WhichOneof("kind") == "array_value"
    assert wire.array_value.elements[0].WhichOneof("kind") == "string_value"


def test_an_array_comes_back_as_an_array_with_typed_elements() -> None:
    back = from_value(to_value(Array([i64(3), i64(1)])))
    assert isinstance(back, Array)
    # `i64` and `u64` are both `int` subclasses that remember which wire arm
    # they came from. A list that came back as bare `int`s would compare equal
    # here and be refused the moment it was sent back.
    assert [type(x).__name__ for x in back] == ["i64", "i64"]
    assert list(back) == [3, 1]


def test_a_bare_int_inside_an_array_is_refused() -> None:
    # A hint names the *array*, not its elements, so nothing can disambiguate
    # a bare `int` inside one. Refused rather than guessed at, for the reason
    # a bare `int` anywhere is: the wire has two integer widths and the kernel
    # orders values type first.
    with pytest.raises(ValueTypeError) as raised:
        to_value(Array([1]), ValueType.ARRAY)
    assert "i64(" in str(raised.value)


def test_an_empty_array_is_not_a_null() -> None:
    empty = to_value(Array([]))
    assert empty.WhichOneof("kind") == "array_value"
    assert from_value(empty) == ()
    assert to_value(None).WhichOneof("kind") == "null_value"


# --- the element type, in the fingerprint ---------------------------------


def test_the_element_type_is_part_of_the_declaration() -> None:
    # Not a style rule: a client that declares `tags` as an array of integers
    # reads the *right* column and decodes every element as the wrong type,
    # with the wire carrying no element type to notice by. The fingerprint is
    # the only place that can catch it, exactly as with a decimal's scale.
    wrong = Table(
        "posts",
        [
            Column("id", ValueType.U64),
            Column("title", ValueType.STR),
            Column("tags", ValueType.ARRAY, element=ValueType.I64),
            Column("scores", ValueType.ARRAY, element=ValueType.I64),
        ],
        primary_key=["id"],
    )
    assert fingerprint_of(wrong) != fingerprint_of(POSTS)


def test_an_array_column_must_declare_an_element_type() -> None:
    with pytest.raises(ValueError):
        Column("tags", ValueType.ARRAY)
    with pytest.raises(ValueError):
        Column("title", ValueType.STR, element=ValueType.STR)


# --- through a real server -------------------------------------------------


def test_an_array_survives_the_round_trip(posted: Client) -> None:
    row = posted.get(POSTS, (u64(FIRST + 1),))
    assert row is not None
    tags = row.get("tags")
    assert isinstance(tags, Array)
    assert list(tags) == ["rust", "db"]
    scores = row.get("scores")
    assert isinstance(scores, Array)
    assert [type(x).__name__ for x in scores] == ["i64", "i64"]
    assert list(scores) == [3, 1]


def test_empty_and_absent_stay_different(posted: Client) -> None:
    # The distinction a list type loses first: "no tags" and "tags unknown"
    # are different values, and they encode differently — `0x24 0x00` against
    # a bare null tag.
    third = posted.get(POSTS, (u64(FIRST + 3),))
    assert third is not None
    assert third.get("scores") == ()
    assert isinstance(third.get("scores"), Array)

    second = posted.get(POSTS, (u64(FIRST + 2),))
    assert second is not None
    assert second.get("scores") is None


def test_an_array_can_be_filtered_on(posted: Client) -> None:
    # The surface the first version offers in place of containment: which rows
    # have *this* array. Asking which rows contain an element needs an
    # inverted index and is not built.
    q = Query(POSTS)
    rows = list(posted.query(q.where(q.c.tags.eq(Array(["db"])))))
    assert [row.get("id") for row in rows] == [FIRST + 2]


def test_an_element_of_the_wrong_type_is_refused_by_the_server(posted: Client) -> None:
    with pytest.raises(SlateError) as raised:
        posted.insert(
            POSTS,
            [(u64(FIRST + 9), "bad", Array(["ok", i64(7)]), None)],
            upsert=True,
        )
    # The server names the element, not just the row: an array can be long and
    # "this row is wrong" is not enough to act on.
    assert "element 1" in str(raised.value)
