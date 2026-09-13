"""The reference model, checked at the wire form rather than at the answer.

These are the only tests here that do not talk to the server, and they are the
ones that pin the property the whole design is for: a `ColumnRef` names a
producer and an index, the position comes from the builder, and there is no way
to write an input number.

Checking the built protobuf rather than the returned rows matters, because the
failure this guards against is a request the server *accepts*. `books.title`
resolved to input 0 is a perfectly good reference to some other table's column,
and no assertion about rows in a passing fixture will notice until the two
tables happen to differ.
"""

from __future__ import annotations

import pytest

from slate import (
    Agg,
    AggregateQuery,
    Column,
    JoinQuery,
    Query,
    Table,
    ValueType,
    i64,
)
from slate.values import ValueTypeError

from .fixture import AUTHORS, BOOKS, DOCS


def test_a_single_table_query_addresses_input_zero() -> None:
    q = Query(DOCS)
    ref = q.c.size.to_proto()
    assert ref.input == 0
    assert ref.WhichOneof("of") == "column"
    assert ref.column == 2


def test_a_join_input_addresses_its_own_position() -> None:
    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS, on=[(a.c.id, "author_id")])
    assert a.c.name.to_proto().input == 0
    assert b.c.title.to_proto().input == 1
    # And the ordinals are each table's own, not a running total: `title` is
    # ordinal 3 of `books`, not 3 + the width of `authors`.
    assert b.c.title.to_proto().column == 3


def test_a_table_produces_no_references_at_all() -> None:
    """The one line that would throw the whole model away.

    A `Table` that produced references would have to choose an input, and the
    only choice available at declaration time is 0 — which is right for the
    first input of a join and silently wrong for every other.
    """
    with pytest.raises(AttributeError):
        BOOKS.title  # type: ignore[attr-defined]  # noqa: B018 - the lookup is the assertion


def test_a_self_join_gives_the_two_inputs_different_positions() -> None:
    j = JoinQuery()
    left = j.add(BOOKS)
    right = j.add(BOOKS, on=[(left.c.author_id, "author_id")])
    assert left.c.id.to_proto().input == 0
    assert right.c.id.to_proto().input == 1
    # An input is a position, which a self-join distinguishes and a qualified
    # name cannot.
    condition = right.to_proto().on[0]
    assert condition.earlier.input == 0
    assert condition.own.input == 1


def test_a_join_condition_naming_another_input_on_its_own_side_is_refused() -> None:
    """Refused here, because the wire would accept it.

    `JoinOn.own` is documented as "a column of the input this condition belongs
    to". A reference from a different input is in range and means something
    else.
    """
    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS)
    with pytest.raises(ValueError, match="belongs to a different input"):
        b.on(a.c.id, a.c.name)


def test_a_join_condition_naming_a_later_input_as_earlier_is_refused() -> None:
    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS)
    with pytest.raises(ValueError, match="not an input read before it"):
        a.on(b.c.id, "id")


def test_a_computed_reference_carries_its_kind_not_an_offset() -> None:
    q = Query(DOCS)
    q.compute(q.c.size * i64(2))
    ref = q.computed(0).to_proto()
    assert ref.WhichOneof("of") == "computed"
    assert ref.computed == 0
    # Not 4, which is where the kernel puts it. The server does that addition.
    assert ref.input == 0


def test_group_keys_and_aggregates_are_their_own_kinds() -> None:
    a = AggregateQuery(BOOKS)
    key = a.key(0).to_proto()
    agg = a.agg(1).to_proto()
    assert key.WhichOneof("of") == "group_key"
    assert agg.WhichOneof("of") == "aggregate"
    # Both must be input 0: they name a grouped result rather than an input.
    assert key.input == 0
    assert agg.input == 0


def test_count_star_carries_no_column_and_count_column_does() -> None:
    a = AggregateQuery(BOOKS)
    assert not Agg.count().to_proto().HasField("column")
    assert Agg.count(a.c.year).to_proto().HasField("column")


# --- values -----------------------------------------------------------------


def test_an_integer_is_encoded_as_the_column_declares_it() -> None:
    q = Query(DOCS)
    # `id` is u64 and `size` is i64. The wire has both and the kernel orders
    # values type first, so getting this wrong is an empty result or a type
    # mismatch — never a near miss.
    assert q.c.id.eq(1).to_proto().compare.value.WhichOneof("kind") == "uint64_value"
    assert q.c.size.eq(1).to_proto().compare.value.WhichOneof("kind") == "int64_value"


def test_an_integer_with_no_declared_type_is_refused() -> None:
    q = Query(DOCS)
    q.compute(q.c.size * i64(2))
    with pytest.raises(ValueTypeError, match="i64"):
        q.computed(0).eq(4)


def test_an_explicit_wrapper_wins_over_the_declaration() -> None:
    """So a caller can address a column this client has declared wrongly."""
    q = Query(DOCS)
    assert q.c.id.eq(i64(1)).to_proto().compare.value.WhichOneof("kind") == "int64_value"


def test_a_bool_is_not_an_integer() -> None:
    """`bool` is a subclass of `int` in Python, and would encode as 0 or 1.

    The server cannot catch that: `Int64Value(1)` is a perfectly good integer.
    """
    flags = Table("flags", [Column("on", ValueType.I64)], primary_key=["on"])
    q = Query(flags)
    assert q.c.on.eq(True).to_proto().compare.value.WhichOneof("kind") == "bool_value"


def test_none_is_an_explicit_null() -> None:
    q = Query(DOCS)
    assert q.c.note.eq(None).to_proto().compare.value.WhichOneof("kind") == "null_value"


# --- predicate assembly -----------------------------------------------------


def test_an_empty_disjunction_is_false_not_true() -> None:
    """`any_of([])` means "none of them", and `TRUE` would mean "every row".

    A caller building `any_of(ids)` from an empty list is the case, and the
    difference between the two answers is the whole table.
    """
    from slate import any_of

    assert any_of([]).to_proto().literal is False


def test_an_empty_conjunction_is_true() -> None:
    from slate import all_of

    assert all_of([]).to_proto().literal is True


def test_a_projection_of_nothing_is_distinct_from_no_projection() -> None:
    """Absent means every column; empty means none, which is what a count wants."""
    assert not Query(DOCS).to_proto().HasField("projection")
    empty = Query(DOCS).select_none().to_proto().projection
    assert not empty.all_columns
    assert list(empty.columns) == []


def test_an_unknown_column_name_raises_where_it_is_written() -> None:
    q = Query(DOCS)
    with pytest.raises(KeyError, match="no column `nope`"):
        q.c.nope  # noqa: B018 - the lookup is the assertion


def test_a_table_refuses_a_primary_key_it_does_not_have() -> None:
    with pytest.raises(ValueError, match="no such column"):
        Table("t", [Column("a", ValueType.U64)], primary_key=["b"])
