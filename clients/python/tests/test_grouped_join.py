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
    desc,
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
