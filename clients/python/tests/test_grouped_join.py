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
    asc,
    concat,
    desc,
    i64,
    lit,
    upper,
)

from .conftest import as_int, as_str
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


def test_a_grouped_join_with_no_aggregates_is_the_distinct_keys(
    client: Client,
) -> None:
    """This used to assert a refusal, and the refusal was wrong.

    A grouped join with keys and no aggregates is `SELECT DISTINCT` over the
    joined rows — the kernel has always answered it, and the server refused
    because its guard tested the aggregates alone. What is still refused is
    neither keys nor aggregates, which the next test covers.
    """
    join, authors, _ = _authors_books()
    grouped = GroupedJoinQuery(join)
    grouped.group_by(authors.c.id)

    groups = list(client.aggregate(grouped))
    assert groups, "the fixture should produce groups"
    # Bare keys: nothing beside them. A server that appended a count would
    # still return one group per author and pass a length assertion.
    assert all(len(g) == 0 for g in groups), [list(g) for g in groups]
    # And one group per author that has a book, which is what makes the keys
    # distinct rather than merely present.
    ids = [g.key[0] for g in groups]
    assert len(ids) == len(set(ids))


def test_a_grouped_join_with_neither_keys_nor_aggregates_is_refused(
    client: Client,
) -> None:
    """One group over every joined row, computing nothing: no answer to give."""
    join, _authors, _ = _authors_books()
    grouped = GroupedJoinQuery(join)

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


def test_ordering_a_chains_groups_by_its_computed_value(client: Client) -> None:
    """The third client for the case Go and TypeScript got on 2026-09-26.

    `ledger/2026-09-26-ordering-a-chains-groups-by-what-it-computed.md` closed
    "`ORDER BY` a chain's computed value is untested from a client" on two of
    three, and recorded the missing third as its own caveat. This is it.

    A *chain* rather than a two-input join, because the grouped chain path
    narrows each step's projection (`narrowed_chain` in the kernel) and
    resolves the grouping in the joined space, where a computed slot sits past
    every table — the one shape where a sort key could plausibly resolve
    against the wrong thing.

    Descending, deliberately. The kernel's own group order is ascending by
    encoded key, so an ascending assertion passes unchanged against a server
    that dropped the sort; the ascending run beside it is what shows the
    descending one was the sort talking and not the fixture.
    """

    def decades(direction: str) -> list[int]:
        join = JoinQuery()
        authors = join.add(AUTHORS)
        books = join.add(BOOKS, on=[(authors.c.id, "author_id")])
        join.add(SALES, on=[(books.c.id, "book_id")])
        # The decade a book came out in, which no table stores.
        join.compute((as_scalar(books.c.year) / i64(10)) * i64(10))

        grouped = GroupedJoinQuery(join)
        grouped.group_by(join.computed(0))
        grouped.aggregate(Agg.count())
        order = desc if direction == "desc" else asc
        grouped.sort(order(grouped.key(0)))

        out = []
        for group in client.aggregate(grouped):
            kind, decade = tag_group(group)["key"][0]
            assert kind == "i64", f"a decade computed from an i64 year is an i64, got {kind}"
            out.append(decade)
        return out

    descending = decades("desc")
    assert len(descending) >= 2, "this needs at least two decades to order"
    assert descending == sorted(descending, reverse=True), descending
    assert decades("asc") == sorted(descending), "the two runs must be reverses"


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


def test_both_kinds_of_computed_value_come_back_on_a_join(client: Client) -> None:
    """Sending is not reading back, and only one of the two could be read.

    The three tests above all *group by* a computed value, so the value reaches
    the client as a group key and the row path is never exercised. On the row
    path `JoinedRow.from_proto` read `wire.inputs` and ignored `wire.computed`
    — the join's computed values arrived and were dropped, with no error
    anywhere, so a caller who declared one got a joined row that looked
    complete and was not.

    An input's own computed values were never dropped: they live on that
    input's `Row`, which has carried `computed` since finding 4. So this asks
    for both at once and checks each against the stored columns it came from,
    which is what tells a value that survived from a value that happened to be
    the right shape.
    """
    join = JoinQuery()
    authors = join.add(AUTHORS)
    books = join.add(BOOKS, on=[(authors.c.id, "author_id")])
    # One per input, reading only that input...
    authors.compute(upper(authors.c.name))
    books.compute((as_scalar(books.c.year) / i64(10)) * i64(10))
    # ...and one on the join, reading both.
    join.compute(concat(authors.c.name, lit("/"), books.c.title))

    rows = list(client.join(join))
    assert rows, "the fixture should produce joined rows"
    for row in rows:
        left, right = row[0], row[1]
        assert left is not None and right is not None, "an inner join matched both"

        # The join's, on the joined row, because they may read every input.
        assert len(row.computed_values) == 1, row
        assert row.computed(0) == (f"{as_str(left.get('name'))}/{as_str(right.get('title'))}")

        # Each input's own, on that input's row, because they read only it.
        assert left.computed_values == (str(left.get("name")).upper(),), left
        assert right.computed_values == (as_int(right.get("year")) // 10 * 10,), right

    # And asking for one the join did not compute says so, rather than
    # returning an input's value or an empty tuple's worth of nothing.
    with pytest.raises(IndexError) as missing:
        rows[0].computed(1)
    assert "input's own computed value" in str(missing.value), missing.value


# --- a computed value belonging to the chain ------------------------------


def test_a_chains_computed_value_reads_every_input(client: Client) -> None:
    """`Chain::compute` over three tables, which is the case `Join::compute`
    cannot reach.

    A chain is a `JoinQuery` with more than two inputs and the same `compute`
    field, so nothing new is declared here — which is exactly why it was worth
    writing down. The two paths through the server are genuinely different:
    `Join::compute` is filled in by `JoinCursor::next` and `Chain::compute` by
    a pass over the accumulated rows in `chain::run`, and no client suite had
    ever asked the second one for anything.

    The expression reads all three tables, so an implementation that evaluated
    it against a prefix of the accumulated row — or against the last step
    alone — comes back null rather than wrong, which the assertions catch.
    """
    join = JoinQuery()
    authors = join.add(AUTHORS)
    books = join.add(BOOKS, on=[(authors.c.id, "author_id")])
    sales = join.add(SALES, on=[(books.c.id, "book_id")])
    join.compute(concat(authors.c.name, lit("/"), books.c.title, lit("/"), sales.c.book_id))

    rows = list(client.join(join))
    assert rows, "the chain fixture should produce rows"
    for row in rows:
        left, middle, right = row[0], row[1], row[2]
        assert left is not None and middle is not None and right is not None
        assert len(row.computed_values) == 1, row
        assert row.computed(0) == (
            f"{as_str(left.get('name'))}/{as_str(middle.get('title'))}"
            f"/{as_int(right.get('book_id'))}"
        )


def test_grouping_a_chain_by_a_value_the_chain_computes(client: Client) -> None:
    """And grouping by one, which resolves in the chain's joined space.

    The grouped path narrows each step's projection, so the computed value's
    ordinal has to be built from the *narrowed* widths and the group key has to
    survive that narrowing. A key naming the value by a raw ordinal would land
    on some table's column instead and come back as a plausible grouping of the
    wrong thing — which is why the key is compared against the ungrouped rows
    rather than merely being non-empty.
    """
    join = JoinQuery()
    authors = join.add(AUTHORS)
    books = join.add(BOOKS, on=[(authors.c.id, "author_id")])
    join.add(SALES, on=[(books.c.id, "book_id")])
    # The decade a book came out in, which no table stores.
    join.compute((as_scalar(books.c.year) / i64(10)) * i64(10))

    grouped = GroupedJoinQuery(join)
    grouped.group_by(join.computed(0))
    grouped.aggregate(Agg.count())

    groups = [tag_group(g) for g in client.aggregate(grouped)]
    assert groups, "the chain should produce groups"

    # The oracle: the same partition folded from the ungrouped chain, whose own
    # computed value this test does not use — it reads `books.year` directly,
    # so the two halves cannot be wrong together.
    rows_join = JoinQuery()
    rows_authors = rows_join.add(AUTHORS)
    rows_books = rows_join.add(BOOKS, on=[(rows_authors.c.id, "author_id")])
    rows_join.add(SALES, on=[(rows_books.c.id, "book_id")])
    wanted: dict[int, int] = {}
    for row in client.join(rows_join):
        books_row = row[1]
        assert books_row is not None
        decade = as_int(books_row.get("year")) // 10 * 10
        wanted[decade] = wanted.get(decade, 0) + 1

    got = {int(g["key"][0][1]): int(g["values"][0][1]) for g in groups}
    assert got == wanted, f"{got} against {wanted}"
