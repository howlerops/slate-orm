"""Writes and point reads, against the real head node.

Ids are in a range of their own (100+) because the head node is session-scoped
and shared: a test that reused the seeded rows would pass or fail depending on
which other test ran first, which is the flakiness this project's own notes
call out as worse than a missing test.
"""

from __future__ import annotations

import pytest

from slate import AlreadyExists, Client, InvalidRequest, NotFound, Query, i64, u64

from .fixture import DOCS


def test_insert_then_get(client: Client) -> None:
    result = client.insert(DOCS, [(u64(100), "kind-x", i64(1), "note")])
    assert result.affected == 1
    # A single-statement write commits, so it has a sequence. A write inside a
    # transaction does not until that transaction commits.
    assert result.sequence is not None
    assert result.sequence.sequence > 0

    row = client.get(DOCS, [u64(100)])
    assert row is not None
    assert row.get("kind") == "kind-x"
    assert row.get("note") == "note"


def test_get_of_an_absent_row_is_none_not_an_error(client: Client) -> None:
    assert client.get(DOCS, [u64(9_999)]) is None


def test_insert_many_in_one_call(client: Client) -> None:
    rows = [(u64(110 + n), "kind-batch", i64(n), None) for n in range(5)]
    result = client.insert(DOCS, rows)
    assert result.affected == 5
    q = Query(DOCS)
    got = list(client.query(q.where(q.c.kind.eq("kind-batch"))))
    assert len(got) == 5


def test_a_null_is_written_and_read_back_as_none(client: Client) -> None:
    client.insert(DOCS, [(u64(120), "kind-null", i64(0), None)])
    row = client.get(DOCS, [u64(120)])
    assert row is not None
    assert row.get("note") is None


def test_duplicate_primary_key_is_already_exists(client: Client) -> None:
    client.insert(DOCS, [(u64(130), "kind-dup", i64(1), None)])
    with pytest.raises(AlreadyExists):
        client.insert(DOCS, [(u64(130), "kind-dup", i64(2), None)])


def test_upsert_replaces_instead_of_refusing(client: Client) -> None:
    client.insert(DOCS, [(u64(140), "before", i64(1), None)])
    client.insert(DOCS, [(u64(140), "after", i64(2), None)], upsert=True)
    row = client.get(DOCS, [u64(140)])
    assert row is not None
    assert row.get("kind") == "after"


def test_update_replaces_a_whole_row(client: Client) -> None:
    client.insert(DOCS, [(u64(150), "kind-u", i64(1), "old")])
    client.update(DOCS, [(u64(150), "kind-u2", i64(9), None)])
    row = client.get(DOCS, [u64(150)])
    assert row is not None
    assert row.get("kind") == "kind-u2"
    assert row.get("size") == 9
    # An update is a replacement, not a patch: the wire carries whole rows and
    # there is no way to send fewer. Stated as a test because a caller coming
    # from an ORM will assume otherwise.
    assert row.get("note") is None


def test_updating_a_missing_row_is_not_found(client: Client) -> None:
    with pytest.raises(NotFound):
        client.update(DOCS, [(u64(9_998), "nope", i64(0), None)])


def test_delete_reports_how_many_existed(client: Client) -> None:
    client.insert(DOCS, [(u64(160), "kind-d", i64(1), None)])
    result = client.delete(DOCS, [[u64(160)], [u64(9_997)]])
    # Unlike an insert, this count is discovered rather than echoed: only one
    # of the two keys was there.
    assert result.affected == 1
    assert client.get(DOCS, [u64(160)]) is None


def test_a_row_of_the_wrong_width_is_refused_before_it_is_sent(client: Client) -> None:
    # Refused here rather than by the server, because the message can name the
    # table's own columns and the server's cannot name the caller's variable.
    with pytest.raises(ValueError, match="declares 4 columns"):
        client.insert(DOCS, [(u64(170), "short")])


def test_a_primary_key_of_the_wrong_width_is_refused(client: Client) -> None:
    with pytest.raises(ValueError, match="primary key of `docs`"):
        client.get(DOCS, [u64(1), u64(2)])


def test_an_unknown_table_is_not_found(client: Client) -> None:
    from slate import Column, Table, ValueType

    ghost = Table("ghost", [Column("id", ValueType.U64)], primary_key=["id"])
    with pytest.raises(NotFound, match="ghost"):
        list(client.query(Query(ghost)))


def test_an_ordinal_past_the_end_of_a_table_is_refused(client: Client) -> None:
    # The one check `ColumnRef` can still fail: an index within the input. The
    # server refuses it by name, which is the diagnosis nothing else offers —
    # a predicate on a column that does not exist otherwise silently matches
    # nothing.
    q = Query(DOCS)
    with pytest.raises(InvalidRequest, match="which has 4 columns"):
        list(client.query(q.where(q.c.at(99).eq(u64(1)))))


def test_a_row_slices_like_the_sequence_it_declares(client: Client) -> None:
    """`Row`, `JoinedRow` and `Group` are `Sequence`s, so slicing is theirs.

    Declared `__getitem__(int)` and suppressed the resulting override error
    until the checker swap, which is a promise narrower than the class they
    inherit from. Slicing always worked — the backing store is a tuple — so the
    overloads changed a declaration and this test is what stops the *capability*
    regressing along with it.
    """
    # 9_200: `DOCS` is shared across the suite and ids are how tests stay out
    # of each other's way. 9_100 and 9_101 belong to `test_deadlines`, and
    # taking 9_100 here made that file fail on a full run and pass alone —
    # which is what a "flake" usually is.
    client.insert(DOCS, [(u64(9_200), "sliceable", i64(3), None)])
    q = Query(DOCS)
    row = next(iter(client.query(q.where(q.c.id.eq(u64(9_200))))))

    assert list(row[:2]) == [row[0], row[1]]
    assert list(row[1:]) == list(row)[1:]
    assert list(row[:]) == list(row)
    # A slice of a Sequence is a Sequence, not a scalar: indexing one column is
    # the other overload and still gives a value.
    assert row[1] == "sliceable"
