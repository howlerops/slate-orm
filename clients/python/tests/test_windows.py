"""Window functions from Python, through a real server.

A window is not an aggregate and the difference is the cardinality: `GROUP BY`
returns one row per group, a window returns one value per *input* row. So the
two things this file has to establish are that the values arrive, and that they
arrive **in their own list** — `Row.window_values`, beside `values` and
`computed_values`, rather than folded into either.

The numbers are checked against `aggregate`, which is the same client talking
to a different RPC that folds rows away. Two operators sharing only the
accumulator arithmetic, so agreeing is evidence.

# Why this module seeds its own rows

It read the server's seeded `docs` first, and passed. Then the *whole* suite
ran and four of these tests failed, because `docs` is shared and half a dozen
other modules write into it: by the time pytest reaches this file the table
holds 687 rows, not 7. That is the repository's own "running one file of a
suite is not running the suite", met head on — a fixture built out of rows
another module owns is a fixture that changes when that module does.

So: eight rows of this module's own, and every query filtered to them. The
filter goes on the **query** rather than into an assertion, because a window is
computed over what the filter admits — dropping the other rows afterwards would
leave the ranks and running totals counting them.

The rows also buy something the seeded ones could not: `size` goes 10, 10, 20,
30 within each kind, and those ties are what separate a correct `RANK` from a
`ROW_NUMBER` wearing its name, and a `RANGE` running total from a `ROWS` one.
Against tie-free data both mistakes pass.
"""

from __future__ import annotations

import pytest

from slate import (
    Agg,
    AggregateQuery,
    Client,
    Query,
    SlateError,
    Window,
    asc,
    desc,
    i64,
    u64,
)
from slate.values import PyValue

from .conftest import Serving, as_int, connect
from .fixture import DOCS

#: Ids of this module's own, clear of every other module's range in `docs`.
FIRST = 1_100_000

#: Two kinds of four rows each. Within a kind the sizes are 10, 10, 20, 30, and
#: the pair of tens is the whole reason the fixture is written out rather than
#: generated: peers are where the interesting half of SQL's frame rules live.
ROWS: list[tuple[int, str, int]] = [
    (0, "win-a", 10),
    (1, "win-a", 10),
    (2, "win-a", 20),
    (3, "win-a", 30),
    (4, "win-b", 10),
    (5, "win-b", 10),
    (6, "win-b", 20),
    (7, "win-b", 30),
]

KINDS = ("win-a", "win-b")


@pytest.fixture(scope="module", autouse=True)
def _seeded(server: Serving) -> None:
    """The eight rows, once for the module.

    `upsert` because nothing here writes and the server is session-scoped, so a
    re-seed has to be a no-op rather than a duplicate-key refusal.
    """
    connect(server).insert(
        DOCS,
        [(u64(FIRST + n), kind, i64(size), None) for n, kind, size in ROWS],
        upsert=True,
    )


def mine(q: Query) -> Query:
    """Restrict a query to this module's rows.

    On the query and not on the results: a window is computed over the rows the
    filter admits, so filtering afterwards would leave every rank and running
    total counting the rest of the suite's writes.
    """
    return q.where(q.c.id.ge(u64(FIRST)))


def _by_id(client: Client, query: Query) -> dict[int, list[PyValue]]:
    """Every row's window values, keyed by the offset from `FIRST`.

    The row-shape assertions live here because they hold for every query in
    this file: a `docs` row has four stored columns and no more, whatever the
    query computed, and a server folding a window into the columns or into
    `computed_values` fails here rather than in one test that happened to look.
    """
    out: dict[int, list[PyValue]] = {}
    for row in client.query(query):
        assert len(row.values) == 4, row
        assert row.computed_values == ()
        out[as_int(row.get("id")) - FIRST] = list(row.window_values)
    assert len(out) == len(ROWS), out
    return out


def test_a_row_number_numbers_each_partition_from_one(client: Client) -> None:
    q = Query(DOCS)
    got = _by_id(
        client,
        mine(q).window(
            Window.row_number().over(partition=[q.c.kind], order=[asc(q.c.id)])
        ),
    )

    # Each partition numbered 1..=4, not the whole result numbered and split:
    # ids 4..7 start over rather than continuing 5, 6, 7, 8.
    assert {n: as_int(values[0]) for n, values in got.items()} == {
        0: 1,
        1: 2,
        2: 3,
        3: 4,
        4: 1,
        5: 2,
        6: 3,
        7: 4,
    }
    assert all(len(values) == 1 for values in got.values()), got


def test_rank_leaves_the_gap_that_dense_rank_closes(client: Client) -> None:
    """The one test the seeded rows could not support, for want of a tie.

    10, 10, 20, 30 ranks 1, 1, 3, 4 and dense-ranks 1, 1, 2, 3. The two
    disagree at exactly one place, which is the place the tie is; over
    distinct sizes they agree everywhere and either could be the other.
    """
    q = Query(DOCS)
    got = _by_id(
        client,
        mine(q).window(
            Window.rank().over(partition=[q.c.kind], order=[asc(q.c.size)]),
            Window.dense_rank().over(partition=[q.c.kind], order=[asc(q.c.size)]),
        ),
    )

    want = {0: (1, 1), 1: (1, 1), 2: (3, 2), 3: (4, 3)}
    for n, (rank, dense) in want.items():
        assert (as_int(got[n][0]), as_int(got[n][1])) == (rank, dense), n
        # The second partition is the same four rows again, which is also the
        # claim that the ranking restarts rather than running 1..8.
        assert (as_int(got[n + 4][0]), as_int(got[n + 4][1])) == (rank, dense), n


def test_a_partition_aggregate_agrees_with_the_same_group_by(client: Client) -> None:
    """The oracle, through a different RPC of the same client."""
    q = Query(DOCS)
    got = _by_id(
        client,
        mine(q).window(
            Window.aggregate_over(Agg.sum(q.c.size)).over(partition=[q.c.kind])
        ),
    )

    a = AggregateQuery(DOCS)
    a.where(a.c.id.ge(u64(FIRST)))
    a.group_by(a.c.kind)
    a.aggregate(Agg.sum(a.c.size))
    grouped = {str(g.key[0]): as_int(g.aggregate(0)) for g in client.aggregate(a)}
    assert set(grouped) == set(KINDS)

    for n, kind, _ in ROWS:
        assert as_int(got[n][0]) == grouped[kind], n


def test_a_running_total_gives_peers_the_same_value(client: Client) -> None:
    """SQL's default frame is `RANGE`, and peers see each other.

    Ordered, the frame runs from the partition's start to the end of the
    *current row's peer group*, so the two tens both see 2. Under `ROWS` —
    a one-character difference in the kernel's loop, and the mistake this
    exists to catch — they would see 1 and 2.
    """
    q = Query(DOCS)
    got = _by_id(
        client,
        mine(q).window(
            Window.aggregate_over(Agg.count()).over(
                partition=[q.c.kind], order=[asc(q.c.size)]
            )
        ),
    )
    assert {n: as_int(values[0]) for n, values in got.items()} == {
        0: 2,
        1: 2,
        2: 3,
        3: 4,
        4: 2,
        5: 2,
        6: 3,
        7: 4,
    }


def test_a_sort_can_name_a_window(client: Client) -> None:
    """The one reference kind a sort key may carry, and the only place it may.

    Descending, and with a tiebreak, so the assertion is about the window's
    value rather than about whatever order the scan happened to give.
    """
    q = Query(DOCS)
    query = (
        mine(q)
        .window(
            Window.aggregate_over(Agg.count()).over(
                partition=[q.c.kind], order=[asc(q.c.size)]
            )
        )
        .sort(desc(q.windowed(0)), asc(q.c.id))
    )
    counts = [as_int(row.windowed(0)) for row in client.query(query)]

    assert counts == [4, 4, 3, 3, 2, 2, 2, 2], counts


def test_lag_steps_through_the_partition(client: Client) -> None:
    q = Query(DOCS)
    got = _by_id(
        client,
        mine(q).window(
            Window.lag(q.c.size).over(partition=[q.c.kind], order=[asc(q.c.id)])
        ),
    )
    # The first row of a partition has no predecessor and gets null; every
    # other row gets the size of the one before it, per partition — so row 4,
    # first of `win-b`, is null rather than row 3's 30.
    assert [got[n][0] for n in range(8)] == [None, 10, 10, 20, None, 10, 10, 20]


def test_a_filter_cannot_name_a_window(client: Client) -> None:
    """Refused by the server, and this asserts the client does not paper over it.

    SQL's own rule: a window is computed after `WHERE`, so there is nothing for
    a filter to name. The builder could have refused this locally and does not
    — one statement of the rule, on the server, where a future client gets it
    for free — so the refusal has to survive the trip back.

    `u64(1)` rather than `1`, and the reason is worth knowing: a window value
    has no declared type, exactly as a computed value has none, so the client
    refuses a bare integer before the request is built. That refusal is a
    different one from the one under test, and writing the literal untyped
    would have made this test pass for the wrong reason.
    """
    q = Query(DOCS)
    with pytest.raises(SlateError) as caught:
        list(
            client.query(
                q.window(Window.row_number().over(order=[asc(q.c.id)])).where(
                    q.windowed(0).eq(u64(1))
                )
            )
        )
    assert "after the filter" in str(caught.value)


def test_an_unordered_rank_is_refused(client: Client) -> None:
    """`RANK` with nothing to rank by would be 1 on every row.

    Refused rather than answered, which is the kernel's decision and is not one
    Postgres makes — worth a test here because a client that swallowed it would
    turn a refusal into a column of ones.
    """
    with pytest.raises(SlateError):
        list(client.query(Query(DOCS).window(Window.rank().over())))


def test_a_running_distinct_count_is_refused(client: Client) -> None:
    """Quadratic in the partition, so refused; unordered it is allowed."""
    q = Query(DOCS)
    with pytest.raises(SlateError):
        list(
            client.query(
                mine(q).window(
                    Window.aggregate_over(Agg.count_distinct(q.c.kind)).over(
                        order=[asc(q.c.id)]
                    )
                )
            )
        )
    # The same thing over a whole partition is one value set and is served.
    other = Query(DOCS)
    got = _by_id(
        client,
        mine(other).window(
            Window.aggregate_over(Agg.count_distinct(other.c.kind)).over()
        ),
    )
    assert {as_int(values[0]) for values in got.values()} == {len(KINDS)}
