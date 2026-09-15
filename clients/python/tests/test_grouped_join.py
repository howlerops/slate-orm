"""Grouping a join, and ordering the groups.

Both reached the wire after this client was first written, so both are newer
than everything around them. The cases here are the ones the Rust wire tests
found worth having, restated against a real client:

- a group key on the *right* side of the join, which is the one that only
  resolves if the joined schema spans every input;
- an ordering over groups that genuinely disagrees with the kernel's default,
  because an ordering that happens to agree with it proves nothing;
- the two refusals, since a client that cannot reach them is a client whose
  users will reach them by accident.
"""

from __future__ import annotations

import pytest

from slate import (
    Agg,
    Client,
    GroupedJoinQuery,
    InvalidRequest,
    JoinInput,
    JoinQuery,
    as_scalar,
    concat,
    desc,
    i64,
    lit,
    upper,
)

from .fixture import AUTHORS, BOOKS, SALES
from .oracle import tag_group


def _authors_books() -> tuple[JoinQuery, JoinInput, JoinInput]:
    """Authors inner-joined to their books, with both inputs to name."""
    join = JoinQuery()
    authors = join.add(AUTHORS)
    books = join.add(BOOKS, on=[(authors.c.id, "author_id")])
    return join, authors, books


def test_a_grouped_join_counts_per_group(client: Client) -> None:
    join, authors, _ = _authors_books()
    grouped = GroupedJoinQuery(join)
    grouped.group_by(authors.c.id)
    grouped.aggregate(Agg.count())

    groups = [tag_group(g) for g in client.aggregate(grouped)]
    assert groups, "the fixture should produce groups"
    # Every group counts at least the one row that created it. `tag` gives a
    # (type, value) pair, and the type is half the assertion: a count that came
    # back as an i64 would be a different value on this wire.
    for group in groups:
        kind, count = group["values"][0]
        assert kind == "u64", f"a count is a u64, got {kind}"
        assert count >= 1


def test_a_group_key_on_the_right_side_resolves(client: Client) -> None:
    """The joined schema has to span every input for this to resolve at all.

    A key on input 0 does not prove that — narrowing the space to the first
    input alone passes every such test. This is the one that fails.
    """
    join, _authors, books = _authors_books()
    grouped = GroupedJoinQuery(join)
    grouped.group_by(books.c.author_id)
    grouped.aggregate(Agg.count())

    right = [tag_group(g) for g in client.aggregate(grouped)]

    # The same partition named from the other side, so the answers match as
    # multisets even though the two name different columns.
    left_join, left_authors, _ = _authors_books()
    left = GroupedJoinQuery(left_join)
    left.group_by(left_authors.c.id)
    left.aggregate(Agg.count())
    assert sorted(map(repr, right)) == sorted(
        map(repr, (tag_group(g) for g in client.aggregate(left)))
    )


def test_ordering_and_limiting_is_over_groups(client: Client) -> None:
    join, authors, _ = _authors_books()
    grouped = GroupedJoinQuery(join)
    grouped.group_by(authors.c.id)
    grouped.aggregate(Agg.count())

    every = [tag_group(g) for g in client.aggregate(grouped)]
    assert len(every) >= 2, "this needs at least two groups to order"

    ordered_join, ordered_authors, _ = _authors_books()
    ordered = GroupedJoinQuery(ordered_join)
    ordered.group_by(ordered_authors.c.id)
    ordered.aggregate(Agg.count())
    # Most rows first. The kernel's own order is ascending by encoded key, so
    # this asks for something it does not already do.
    ordered.sort(desc(ordered.agg(0)))
    ordered.limit(1)

    top = [tag_group(g) for g in client.aggregate(ordered)]
    assert len(top) == 1, "the limit is over groups"

    biggest = max(g["values"][0][1] for g in every)
    assert top[0]["values"][0][1] == biggest


def test_an_offset_skips_groups(client: Client) -> None:
    join, authors, _ = _authors_books()
    grouped = GroupedJoinQuery(join)
    grouped.group_by(authors.c.id)
    grouped.aggregate(Agg.count())
    grouped.sort(desc(grouped.agg(0)))

    every = [tag_group(g) for g in client.aggregate(grouped)]

    skipped_join, skipped_authors, _ = _authors_books()
    skipped = GroupedJoinQuery(skipped_join)
    skipped.group_by(skipped_authors.c.id)
    skipped.aggregate(Agg.count())
    skipped.sort(desc(skipped.agg(0)))
    skipped.offset(1)

    rest = [tag_group(g) for g in client.aggregate(skipped)]
    assert rest == every[1:], "an offset drops the leading groups, in order"


def test_grouping_a_chain(client: Client) -> None:
    """A three-table chain, grouped.

    This asserted a refusal until the kernel grew `group_by_chain`. Rewritten
    rather than deleted: a refusal test that outlives the refusal passes
    forever and protects nothing.
    """
    join = JoinQuery()
    authors = join.add(AUTHORS)
    books = join.add(BOOKS, on=[(authors.c.id, "author_id")])
    join.add(SALES, on=[(books.c.id, "book_id")])

    grouped = GroupedJoinQuery(join)
    grouped.group_by(authors.c.id)
    grouped.aggregate(Agg.count())

    groups = [tag_group(g) for g in client.aggregate(grouped)]
    assert groups, "the chain should produce groups"
    for group in groups:
        kind, count = group["values"][0]
        assert kind == "u64"
        assert count >= 1


def test_a_grouped_join_needs_at_least_one_aggregate(client: Client) -> None:
    join, authors, _ = _authors_books()
    grouped = GroupedJoinQuery(join)
    grouped.group_by(authors.c.id)

    with pytest.raises(InvalidRequest):
        list(client.aggregate(grouped))


# --- a computed value belonging to the join -------------------------------


def test_grouping_by_a_value_the_join_computes(client: Client) -> None:
    """`JoinQuery.compute` is the kind an input's own `compute` cannot be.

    An input's computed value is appended to *that input's* row, and a joined
    row is packed by declared table width — so such a value has no slot in the
    joined space at all, and naming one across inputs is refused. A value
    declared on the *join* has one, past every input's columns, and can read
    any input.

    This module's header says the grouped-join cases are the ones the Rust wire
    tests found worth having. This is the newest of them: until `JoinQuery`
    grew `compute`, the query below could not be expressed over gRPC by any
    client, only by the browser binding.
    """
    join, _, books = _authors_books()
    # The decade a book came out in, which neither table stores.
    join.compute((as_scalar(books.c.year) / i64(10)) * i64(10))

    grouped = GroupedJoinQuery(join)
    grouped.group_by(join.computed(0))
    grouped.aggregate(Agg.count())

    groups = [tag_group(g) for g in client.aggregate(grouped)]
    assert groups, "the fixture should produce groups"
    for group in groups:
        kind, decade = group["key"][0]
        assert kind == "i64", f"a decade computed from an i64 year is an i64, got {kind}"
        assert decade % 10 == 0, f"{decade} is not a decade"


def test_a_joins_computed_value_may_read_both_inputs(client: Client) -> None:
    """The case no input's own `compute` can express, which is why this exists.

    A value reading one table could have been declared on that input. One
    reading *both* could not: an input's computed value is evaluated over that
    input's row alone.
    """
    join, authors, books = _authors_books()
    join.compute(concat(authors.c.name, lit("/"), books.c.title))

    grouped = GroupedJoinQuery(join)
    grouped.group_by(join.computed(0))
    grouped.aggregate(Agg.count())

    groups = [tag_group(g) for g in client.aggregate(grouped)]
    assert groups, "the fixture should produce groups"
    for group in groups:
        kind, joined = group["key"][0]
        assert kind == "str"
        assert "/" in joined, f"{joined!r} should span both tables"


def test_an_inputs_computed_value_is_not_nameable_across_a_join(
    client: Client,
) -> None:
    """And the refusal says which kind does work, rather than just refusing.

    The two kinds are a hair apart in spelling — `inputs[0].computed(0)` and
    `join.computed(0)` — and one of them silently meant the next table's first
    column before the server learned to refuse it. So the message naming
    `joined_computed` is the load-bearing part, not the refusal itself.
    """
    join, authors, _ = _authors_books()
    authors.compute(upper(authors.c.name))

    grouped = GroupedJoinQuery(join)
    grouped.group_by(authors.computed(0))
    grouped.aggregate(Agg.count())

    with pytest.raises(InvalidRequest) as refused:
        list(client.aggregate(grouped))
    assert "joined_computed" in str(refused.value), refused.value
