"""How many round trips a client makes, counted at the channel.

# The claim this file settles

Two entries say a batch is one round trip rather than N, and both say the
measurement is missing from the place it matters:

  > **No measurement from a client.** The 9.4x to 10.0x in the previous entry was
  > measured through the Rust wire test; none of the three clients has a
  > benchmark, so the claim that batching helps *them* is inherited rather than
  > shown.
  > — `ledger/2026-09-16-batch-in-three-clients-and-a-token-that-only-survives-batched.md`

  > Nothing measures the *cost* on the wire. The README's "page 99 read 495
  > key-value pairs by offset and 5 by cursor" is a kernel measurement and is
  > still the only one; the same claim through a head node is untested.
  > — `ledger/2026-09-16-keyset-pagination-on-the-wire-and-the-page-that-failed-second.md`

A `grpc.UnaryUnaryClientInterceptor` on the channel counts what the client
sends. That is the round trip, as the caller experiences it, from inside the
client rather than inferred from the server or from the shape of the code.

# Why counting rather than timing

The inherited number was a *ratio of durations*, and a duration on a loopback
socket against an in-memory store is mostly scheduling. This session already
published one timing claim that reversed on a different machine
(`ledger/2026-09-28-a-measurement-that-reversed-under-ci.md`), and the lesson
there was that a count has no spread. Twenty writes are twenty requests or they
are one; no runner load changes that.

What this therefore does **not** settle is the part of those caveats about
per-request client work — "the clients add per-request work the Rust test does
not (schema claims, value encoding), and that work is not saved by batching".
Encoding twenty rows costs the same twenty encodings either way. Counting
requests cannot see that, and says so rather than implying otherwise.

# Why the interceptor rather than the server's counters

`slate-serverd` exports `slate_requests_total{method=…}` and the daemon's own
suite scrapes it. Rejected here for two reasons. The suite's server is the
testserver, which serves no `/metrics`; and a server-side count answers "how
many requests arrived", which is the same number only if the client sent what
it thinks it sent. The caveat is about the client, so the instrument belongs in
the client.
"""

from __future__ import annotations

import collections
from collections.abc import Iterator
from typing import Any

import grpc
import pytest

from slate import Atomicity, Batch, Client, Query
from slate.values import PyValue

from .conftest import APP, Serving
from .fixture import DOCS


class Counting(grpc.UnaryUnaryClientInterceptor, grpc.UnaryStreamClientInterceptor):
    """Counts calls per method, and forwards them untouched.

    Both interfaces, because a read is server-streaming and a write is unary:
    an interceptor registered for only the first would count writes and report
    zero for every query, which reads as "queries are free" rather than as a
    hole in the instrument.
    """

    def __init__(self) -> None:
        self.calls: collections.Counter[str] = collections.Counter()

    def _count(self, method: str) -> None:
        # The method arrives as `/slate.v1.Records/Write`; the last segment is
        # what a reader of a failure message wants.
        self.calls[method.rsplit("/", 1)[-1]] += 1

    def intercept_unary_unary(
        self, continuation: Any, client_call_details: Any, request: Any
    ) -> Any:
        # Named `client_call_details` because the base class is: `grpc`'s
        # interceptor protocols take it as a keyword, so a shorter name is a
        # Liskov violation the type checker refuses rather than a style choice.
        self._count(client_call_details.method)
        return continuation(client_call_details, request)

    def intercept_unary_stream(
        self, continuation: Any, client_call_details: Any, request: Any
    ) -> Any:
        self._count(client_call_details.method)
        return continuation(client_call_details, request)

    def total(self) -> int:
        return sum(self.calls.values())


@pytest.fixture
def counted(server: Serving) -> Iterator[tuple[Client, Counting]]:
    """A client whose channel counts every call it makes.

    Function-scoped and its own channel, so one test's calls cannot land in
    another's count. `Client` takes `channel=` precisely so a caller can bring
    a configured one; nothing in the client had to change for this.
    """
    counter = Counting()
    channel = grpc.intercept_channel(grpc.insecure_channel(server.address), counter)
    client = Client(server.address, APP, channel=channel)
    client.wait_for_ready()
    try:
        yield client, counter
    finally:
        # Every row this file writes is deleted again, because `server` is
        # session-scoped and shared. Learned the hard way: the first version
        # left forty rows in `docs` and five tests in `test_streaming.py` —
        # which count rows rather than naming them — went red. They pass alone
        # and fail in the suite, which is the exact shape `CLAUDE.md` warns
        # about under "running one file of a suite is not running the suite".
        #
        # Through a plain client, so the deletes do not land in a count. The
        # keys are the whole range this file can write, not the ones a given
        # test used: a test that fails partway through has still written some.
        janitor = Client(server.address, APP)
        janitor.wait_for_ready()
        try:
            janitor.delete(DOCS, [[key] for key in range(FIRST_KEY, FIRST_KEY + ROWS * 2)])
        finally:
            janitor.close()
        channel.close()


FIRST_KEY = 10_000
"""Above anything the fixture seeds, so these rows collide with no other test.

Not sufficient on its own — the rows are deleted in the fixture's teardown for
the reason written there — but it keeps a *running* test in this file from
changing what a concurrent one sees.
"""

PAGES = 4
PAGE_SIZE = 5
"""Four pages of five, which the seeded `docs` rows comfortably exceed.

Separate constants from `ROWS` because the paging case reads what the fixture
seeded and the write cases add their own; tying them together would make a
change to one silently resize the other.
"""

ROWS = 20
"""Enough that a per-row loop and a single call cannot be confused.

Twenty rather than three because the numbers have to be far apart for the
failure message to be worth reading: `21 != 2` says something a reader can act
on, where `4 != 2` could be an off-by-one anywhere.
"""


def _row(n: int) -> list[PyValue]:
    return [FIRST_KEY + n, "counted", 1, "round-trip fixture"]


def test_writing_n_rows_one_at_a_time_is_n_round_trips(
    counted: tuple[Client, Counting],
) -> None:
    """The control. Without it the batch case proves only that a batch works."""
    client, counter = counted
    counter.calls.clear()

    for n in range(ROWS):
        client.insert(DOCS, [_row(n)])

    assert counter.calls["Insert"] == ROWS, f"one Insert per row; got {counter.calls}"


def test_writing_n_rows_in_a_batch_is_one_round_trip(
    counted: tuple[Client, Counting],
) -> None:
    """The claim, counted from the client rather than inherited.

    One `Batch` call for twenty rows, against the twenty `Write` calls above.
    This is what "a batch is a round trip" means and is the half the previous
    entries could not show.
    """
    client, counter = counted
    counter.calls.clear()

    batch = Batch(Atomicity.INDEPENDENT)
    for n in range(ROWS, ROWS * 2):
        batch.insert(DOCS, [_row(n)])
    client.batch(batch)

    assert counter.calls["Batch"] == 1, f"one Batch for {ROWS} rows; got {counter.calls}"
    assert counter.calls["Insert"] == 0, f"a batch must not also Insert; got {counter.calls}"
    assert counter.total() == 1, f"one call in total; got {counter.calls}"


def test_paging_by_cursor_is_one_round_trip_per_page(
    counted: tuple[Client, Counting],
) -> None:
    """Keyset paging on the wire: pages cost calls, and the count is the pages.

    The README's key-value figure is a kernel measurement of what a page *reads*
    from the store. This is the other number the caveat asks for — what a page
    costs a client — and it is the one a caller sizing a page against a network
    actually needs. Four pages of five over the seeded rows: four calls, not one
    per row and not one for the lot.
    """
    client, counter = counted

    # Its own rows, in a batch, before the counter is cleared. The seeded
    # `docs` are fewer than `PAGES * PAGE_SIZE`, so paging over them ran out
    # partway and the test measured fewer pages than it claimed — caught by the
    # `cursor is not None` assertion below, which is why that assertion is
    # there rather than an `if cursor is None: break` that would have hidden it.
    seeding = Batch(Atomicity.INDEPENDENT)
    for n in range(PAGES * PAGE_SIZE):
        seeding.insert(DOCS, [_row(n)])
    client.batch(seeding)
    counter.calls.clear()

    cursor, seen = None, 0
    for _ in range(PAGES):
        q = Query(DOCS).limit(PAGE_SIZE)
        page = client.page(q.where(q.c.kind.eq("counted")).after(cursor))
        seen += len(page.rows)
        cursor = page.cursor
        assert cursor is not None, (
            "the fixture ran out of rows before the pages did, so this measured "
            "fewer pages than it says"
        )

    # Both sides of this are literals, and that is the point. The first version
    # asserted against a `pages` counter the loop itself incremented, so it held
    # for any number of pages including one — a mutation shrinking the loop
    # survived, because shrinking the loop shrank both sides of the equation.
    # An assertion whose expected value is computed by the code under test is
    # not an assertion.
    #
    # `Query`, not a `Page` RPC: a keyset page is an ordinary query carrying a
    # cursor and a limit, which is the wire shape the interceptor reports.
    assert counter.calls["Query"] == PAGES, (
        f"{PAGES} pages should cost {PAGES} calls; got {counter.calls}"
    )
    assert seen == PAGES * PAGE_SIZE, (
        f"{PAGES} full pages of {PAGE_SIZE}; got {seen} rows"
    )
