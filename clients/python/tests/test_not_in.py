"""`NOT IN`, as the negation of `IN`, through a real server.

# Why this file exists

`ledger/2026-09-18-not-in-was-refused-on-a-claim-that-does-not-hold.md` taught
the SQL front end to write `NOT IN` and then said of the clients:

    **No client can express it.** `notIn` is a front-end operator; the wire
    carries `Expr`, and the three SDKs build `Expr::In` with no negation
    helper. A client that wants this builds the `Not` itself, which is
    possible and undocumented.

Its two sentences contradict each other and the second is the true one. This
client has had `not_` and `~` the whole time, so `~q.c.kind.in_([...])` is the
expression. `ledger/2026-09-29-three-could-be-anothers-counted.md` withdrew the
first half and found what the caveat had not: **no test in any of the three
clients used the negation helper at all.** Three doors and nobody walking
through any of them, which is the same shape as the wrong caveat — somebody
read the front end and generalised, and nothing ran to say otherwise.

# The oracle

`NOT IN` is the complement of `IN` over the rows that exist: the two must be
disjoint and must together cover every row. That is stronger than comparing
`NOT IN` to a hand-written list, because it fails if either arm drifts, and it
is the property a reader relies on.

It is **not** SQL's `NOT IN`, and the difference is why this fixture has no
nulls. SQL's is three-valued: a null in the list makes every answer unknown and
admits nothing. `Expr::In` has the same rule — `docs/correctness.md` works
through it — so the complement holds here only because the column is not
nullable. A nullable one would need its own case and does not have one.

Both spellings are exercised, `not_(...)` and `~`, because they are two public
names for one thing and a test of one says nothing about the other.
"""

from __future__ import annotations

from slate import Client, Query, not_

from .conftest import as_int
from .fixture import DOCS


def _ids(client: Client, expr) -> set[int]:
    q = Query(DOCS)
    return {as_int(row[0]) for row in client.query(q.where(expr(q)))}


def test_not_in_is_the_complement_of_in(oracle_client: Client) -> None:
    rows = list(oracle_client.query(Query(DOCS)))
    assert len(rows) > 2, "too few rows for a complement to mean anything"
    every = {as_int(row[0]) for row in rows}

    # A proper, non-empty subset of the kinds, taken from the fixture rather
    # than written down: a list naming every kind makes the complement empty
    # and one naming none makes it everything, and either passes a `not` that
    # returned its argument.
    kinds = sorted({str(row[1]) for row in rows})
    assert len(kinds) > 1, f"one kind ({kinds}) cannot be split"
    listed = kinds[: len(kinds) // 2] or kinds[:1]

    inside = _ids(oracle_client, lambda q: q.c.kind.in_(listed))
    outside = _ids(oracle_client, lambda q: not_(q.c.kind.in_(listed)))

    assert inside, f"no row has a kind in {listed}"
    assert outside, f"every row has a kind in {listed}, so the complement is empty"
    assert not (inside & outside), (
        f"IN and NOT IN both admit {sorted(inside & outside)}, so the negation did nothing"
    )
    assert inside | outside == every, (
        f"IN and NOT IN together admit {sorted(inside | outside)}, not the {sorted(every)} seeded"
    )


def test_the_tilde_spells_the_same_negation(oracle_client: Client) -> None:
    rows = list(oracle_client.query(Query(DOCS)))
    kinds = sorted({str(row[1]) for row in rows})
    listed = kinds[: len(kinds) // 2] or kinds[:1]

    spelled = _ids(oracle_client, lambda q: not_(q.c.kind.in_(listed)))
    operator = _ids(oracle_client, lambda q: ~q.c.kind.in_(listed))
    assert spelled == operator, f"not_() gave {sorted(spelled)}, ~ gave {sorted(operator)}"
    # And neither is the whole table, which is what a `~` that lost its
    # negation on the way to the wire would return.
    assert operator != {as_int(row[0]) for row in rows}


def test_negation_is_not_ignored_on_a_single_comparison(oracle_client: Client) -> None:
    # The narrower half. The complement test above is satisfied by a server
    # that answered the complement for `IN` as well — far-fetched, but not
    # excluded by that test alone. One comparison and its negation is.
    rows = list(oracle_client.query(Query(DOCS)))
    kind = str(rows[0][1])

    same = _ids(oracle_client, lambda q: q.c.kind.eq(kind))
    other = _ids(oracle_client, lambda q: not_(q.c.kind.eq(kind)))

    assert same, f"no row of kind {kind!r}"
    assert other, f"every row is of kind {kind!r}, so the negation is empty"
    assert not (same & other), f"a comparison and its negation both admit {sorted(same & other)}"
