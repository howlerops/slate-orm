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

from slate import Atomicity, Batch, Client, Query, i64, u64
from slate._proto.slate.v1 import records_pb2 as pb
from slate.values import PyValue

from .conftest import APP, Serving
from .fixture import DOCS, SHELVES


class Counting(grpc.UnaryUnaryClientInterceptor, grpc.UnaryStreamClientInterceptor):
    """Counts calls per method and weighs them, forwarding them untouched.

    Both interfaces, because a read is server-streaming and a write is unary:
    an interceptor registered for only the first would count writes and report
    zero for every query, which reads as "queries are free" rather than as a
    hole in the instrument.

    # Why it weighs as well as counts

    Five caveats across three entries said this work "says nothing about
    latency or bytes", and every one of them deferred both to
    `examples/deployed`, which needs MinIO. That conflates two questions. A
    *latency* needs a network and this container has none. A *byte* does not:
    `request.ByteSize()` is the serialized length of the message the client is
    about to send, it is exact, and it has no spread — the same request
    weighs the same on every run and every machine, because protobuf
    serialization is deterministic.

    So the bytes are here, beside the counts, measured by the same instrument
    on the same workloads. The latency half stays where it was and says why.

    Request bytes only, and the name says `bytes` rather than `on the wire`:
    this is the protobuf payload, not the HTTP/2 frame, not the headers, not
    the identity metadata, and not the response. Those are real and this does
    not measure them. What it measures is the part that scales with the
    caller's data, which is the part the batching and paging claims are about.
    """

    def __init__(self) -> None:
        self.calls: collections.Counter[str] = collections.Counter()
        self.bytes: collections.Counter[str] = collections.Counter()

    def _count(self, method: str, request: Any) -> None:
        # The method arrives as `/slate.v1.Records/Write`; the last segment is
        # what a reader of a failure message wants.
        name = method.rsplit("/", 1)[-1]
        self.calls[name] += 1
        self.bytes[name] += request.ByteSize()

    def intercept_unary_unary(
        self, continuation: Any, client_call_details: Any, request: Any
    ) -> Any:
        # Named `client_call_details` because the base class is: `grpc`'s
        # interceptor protocols take it as a keyword, so a shorter name is a
        # Liskov violation the type checker refuses rather than a style choice.
        self._count(client_call_details.method, request)
        return continuation(client_call_details, request)

    def intercept_unary_stream(
        self, continuation: Any, client_call_details: Any, request: Any
    ) -> Any:
        self._count(client_call_details.method, request)
        return continuation(client_call_details, request)

    def total(self) -> int:
        return sum(self.calls.values())

    def weight(self) -> int:
        """Serialized request bytes across every method."""
        return sum(self.bytes.values())


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
    counter.bytes.clear()

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
    counter.bytes.clear()

    batch = Batch(Atomicity.INDEPENDENT)
    for n in range(ROWS, ROWS * 2):
        batch.insert(DOCS, [_row(n)])
    client.batch(batch)

    assert counter.calls["Batch"] == 1, f"one Batch for {ROWS} rows; got {counter.calls}"
    assert counter.calls["Insert"] == 0, f"a batch must not also Insert; got {counter.calls}"
    assert counter.total() == 1, f"one call in total; got {counter.calls}"


def test_batching_saves_round_trips_and_costs_bytes(
    counted: tuple[Client, Counting],
) -> None:
    """The half the counts could not see, and it goes the other way.

    "A batch is a round trip" is true and this file already shows it: twenty
    writes, one call. The unstated companion — that batching is therefore
    cheaper — is false on the request. **A batch of twenty costs more bytes
    than twenty singles, not fewer**, because each statement gains a tag and a
    length prefix from being nested in a `repeated` field and the batch adds an
    envelope of its own.

    Measured, at five sizes, twice each, and exactly linear:

    | rows | batch | singles | delta |
    | --- | --- | --- | --- |
    | 1 | 71 | 65 | +6 |
    | 2 | 140 | 130 | +10 |
    | 5 | 347 | 325 | +22 |
    | 10 | 692 | 650 | +42 |
    | 20 | 1382 | 1300 | +82 |

    `delta = 4n + 2`: four bytes per statement, two for the envelope. The
    *totals* depend on the rows — `_row` writes a fixed `note` and a key whose
    varint widens past 2^14 — so the assertion below is on the delta and this
    table is what these twenty rows happen to weigh. An earlier run of the
    same measurement at a different key range read 1402 against 1320: the
    totals moved by 20, the delta did not move at all. That is the reason the
    formula is the assertion and the totals are only narration.

    The two numbers together are the honest version of the claim: **twenty
    writes go from twenty requests to one, and from 1300 bytes to 1382.** On
    any real link that is an enormous win, because a round trip costs a
    latency and 82 bytes costs nothing — but it is a trade and the entries
    describing batching as "cheaper on the wire" did not say so.

    No spread, and none is reported: `ByteSize()` is deterministic for a given
    request, so the only thing that moves these numbers is the rows.
    """
    client, counter = counted

    for n in range(ROWS):
        client.insert(DOCS, [_row(n)], upsert=True)
    singly = counter.weight()

    counter.calls.clear()
    counter.bytes.clear()
    batch = Batch(Atomicity.INDEPENDENT)
    for n in range(ROWS):
        batch.upsert(DOCS, [_row(n)])
    client.batch(batch)
    batched = counter.weight()

    # The same rows both ways, so the payload is identical and the difference
    # is framing. Written as one assertion on the *formula* rather than on
    # 1402: a literal would pin the row contents too, and a wider `note`
    # column would then read as a regression in batching.
    assert batched - singly == 4 * ROWS + 2, (
        f"a batch should cost 4 bytes per statement plus a 2-byte envelope; "
        f"{ROWS} rows weighed {batched} batched against {singly} singly"
    )
    # There is deliberately no second assertion that `batched > singly`. It
    # was written, and the mutation weakening it to `batched >= 0` survived —
    # correctly, because `4 * ROWS + 2` is positive for any `ROWS`, so the
    # formula above already forces the direction. An assertion that cannot
    # fail while its neighbour holds is not a second check, it is a sentence
    # in the wrong place; the direction is the finding and the docstring is
    # where the finding belongs.
    assert counter.calls["Batch"] == 1, f"still one call; got {counter.calls}"


def test_paging_by_cursor_costs_more_bytes_than_paging_by_offset(
    counted: tuple[Client, Counting],
) -> None:
    """The other phrase counting could not settle, and it goes the other way too.

    `2026-09-29-the-days-own-caveats-read-and-a-phrase-that-did-not-survive-counting.md`
    left this exactly: *"an offset page's request carries an integer where a
    cursor's carries a key, and nothing weighs them."* Weighed:

    | | four pages, total request bytes |
    | --- | --- |
    | by cursor | 215 |
    | by offset | 198 |

    A cursor page is heavier than an offset page once there is a cursor to
    send: **+17 bytes over four pages, about 9%.** Per page it is a few bytes,
    and how few depends on the key — a cursor carries the last row's primary
    key as a varint, so it widens with the value, where an offset carries a
    small integer. An earlier run over a different key range read 218 against
    198. Hence the assertion below is a range and not a literal: what is being
    pinned is *a cursor costs more than an offset and is not free*, which a
    cursor dropped from the request would break by making them equal.

    Which is the same shape as the batch finding and the same correction to the
    same phrase. What keyset paging saves is *store* reads — the README's 495
    key-value pairs by offset against 5 by cursor — and that saving is on the
    server. On the request it is a small loss. "Cheaper on the wire" is wrong
    about the wire and right about the store, and those are different places.

    Sibling of `test_paging_by_offset_costs_the_same_calls_as_paging_by_cursor`
    above, which found the counts identical; this is why identical counts were
    not the end of the question.
    """
    client, counter = counted

    seeding = Batch(Atomicity.INDEPENDENT)
    for n in range(PAGES * PAGE_SIZE):
        seeding.insert(DOCS, [_row(n)])
    client.batch(seeding)

    counter.calls.clear()
    counter.bytes.clear()
    cursor = None
    for _ in range(PAGES):
        q = Query(DOCS).limit(PAGE_SIZE)
        page = client.page(q.where(q.c.kind.eq("counted")).after(cursor))
        cursor = page.cursor
        assert cursor is not None, "the fixture ran out of rows before the pages did"
    by_cursor = counter.weight()

    counter.calls.clear()
    counter.bytes.clear()
    for page_n in range(PAGES):
        q = Query(DOCS).limit(PAGE_SIZE).offset(page_n * PAGE_SIZE)
        list(client.query(q.where(q.c.kind.eq("counted"))))
    by_offset = counter.weight()

    assert counter.calls["Query"] == PAGES, f"four offset pages; got {counter.calls}"
    # A range rather than the literal 20, for the reason the batch test gives:
    # the difference is a key and a flag, and the key is a `u64` whose varint
    # grows with the value. Pinned tightly enough that a cursor dropped from
    # the request — which would make the two equal — fails.
    assert 12 <= by_cursor - by_offset <= 40, (
        f"a cursor page carries a key where an offset page carries an integer, "
        f"so four cursor pages should be ~20 bytes heavier; "
        f"got {by_cursor} against {by_offset}"
    )


def test_a_batchs_framing_cost_is_four_bytes_a_statement_and_two_for_the_envelope() -> None:
    """Where `4n + 2` comes from, decoded rather than read off the rules.

    The batch test above asserts the formula and the entry that found it could
    only explain it — a tag byte and a length prefix per nested statement — by
    reading the protobuf encoding rules. A different framing producing the same
    slope would have been indistinguishable. This takes the messages apart:

    | message | bytes |
    | --- | --- |
    | `InsertRequest` | 25 |
    | `BatchOperation(insert=…)` | 27 |
    | `BatchRequest`, no operations | 2 |
    | `BatchRequest`, one operation | 31 |
    | `BatchRequest`, two operations | 60 |

    Two for the `oneof` that wraps the insert in a `BatchOperation`, two for
    the `repeated` field that holds it in the `BatchRequest` — four per
    statement. And the "envelope" is not framing at all: it is the two bytes of
    the `atomicity` enum, a field a single `Insert` has no equivalent of.

    No server: these are messages, and building them is the whole measurement.
    """
    row = pb.Row(
        values=[
            pb.Value(uint64_value=70_001),
            pb.Value(string_value="x"),
            pb.Value(int64_value=1),
        ]
    )
    insert = pb.InsertRequest(table="docs", rows=[row], upsert=True)
    operation = pb.BatchOperation(insert=insert)
    empty = pb.BatchRequest(atomicity=pb.ATOMICITY_INDEPENDENT)
    one = pb.BatchRequest(atomicity=pb.ATOMICITY_INDEPENDENT, operations=[operation])
    two = pb.BatchRequest(atomicity=pb.ATOMICITY_INDEPENDENT, operations=[operation] * 2)

    # The two wrappers, named separately: the whole point is that the four is
    # two things and not one, so a change that moved a byte from one to the
    # other would still satisfy a single assertion on the total.
    assert operation.ByteSize() - insert.ByteSize() == 2, "the oneof's tag and length"
    assert one.ByteSize() - empty.ByteSize() - operation.ByteSize() == 2, (
        "the repeated field's tag and length"
    )
    # The envelope, and *what it is*. Asserting only that it weighs two leaves
    # "two bytes of framing" and "two bytes of atomicity" indistinguishable,
    # and the mutation weakening it to `>= 0` survived because two satisfies
    # both. A `BatchRequest` with no atomicity and no operations weighs
    # nothing, which is the falsifiable form: the envelope is a field the
    # caller set, not a cost the wire imposes.
    assert pb.BatchRequest().ByteSize() == 0, "an empty BatchRequest is not framing"
    assert empty.ByteSize() == 2, "the envelope is the atomicity enum, not framing"
    # And the slope, from the messages rather than from a server: adding a
    # second statement costs the first one's size plus the same four.
    assert two.ByteSize() - one.ByteSize() == insert.ByteSize() + 4


def test_loading_a_relation_for_three_parents_saves_bytes_as_well_as_calls(
    counted: tuple[Client, Counting],
) -> None:
    """Batching saves bytes here, and that is the opposite of the write path.

    The write batch costs `4n + 2` because each statement carries its own
    table, rows and schema claim: nothing is shared, and nesting adds framing.
    A relation load shares everything — one table, one relationship name, one
    schema claim — and varies only the parent keys. So folding three into one
    saves the two copies of the header.

    Measured: **one `Related` for three parents is 53 bytes; three `Related`
    calls for one parent each are 147.** A 64% saving, against a 6% cost on the
    write path, from the same word "batching".

    That is the honest shape of the claim these entries have been circling:
    batching saves bytes when the requests share a header and costs bytes when
    they do not. Neither number alone says that; the pair does.
    """
    client, counter = counted
    session = client.session()

    counter.calls.clear()
    counter.bytes.clear()
    session.related(SHELVES, [u64(10), u64(11), u64(12)], through="shelf_library", on=SHELVES)
    together = counter.weight()

    counter.calls.clear()
    counter.bytes.clear()
    for parent in (10, 11, 12):
        session.related(SHELVES, [u64(parent)], through="shelf_library", on=SHELVES)
    apart = counter.weight()

    assert counter.calls["Related"] == 3, f"three calls, one per parent; got {counter}"
    # Strictly less, and by a margin no framing change could close: the saving
    # is two whole copies of the request minus two keys, so it is roughly
    # proportional to the header. Asserted as a ratio rather than a literal for
    # the reason the sibling tests give — the totals move with the fixture.
    assert together * 2 < apart, (
        f"one call for three parents should cost well under half of three calls: "
        f"{together} against {apart}"
    )


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
    counter.bytes.clear()

    cursor, seen = None, []
    for _ in range(PAGES):
        q = Query(DOCS).limit(PAGE_SIZE)
        page = client.page(q.where(q.c.kind.eq("counted")).after(cursor))
        seen += [row[0] for row in page.rows]
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
    # The *keys*, not a count of them. A count cannot tell four pages from four
    # copies of the first page, and that is not hypothetical: a mutation pinning
    # every offset page to 0 survived the count assertion this replaces. The
    # cursor twin has the same hole — `after(cursor)` mutated to `after(None)`
    # returns twenty rows in four calls — so both tests walk the keys.
    assert seen == list(range(FIRST_KEY, FIRST_KEY + PAGES * PAGE_SIZE)), (
        f"{PAGES} pages of {PAGE_SIZE} should walk the keys once, in order; got {seen}"
    )


def test_paging_by_offset_costs_the_same_calls_as_paging_by_cursor(
    counted: tuple[Client, Counting],
) -> None:
    """The saving keyset paging offers is not in round trips, and this shows it.

    Three entries describe keyset paging as "cheaper on the wire", grouped with
    the batching claim as one unmeasured piece of work. Counting it makes the
    phrase falsifiable and it does not survive: **four offset pages cost four
    `Query` calls, exactly as four keyset pages do.** A page is one request
    either way, because the client asks for one page either way.

    What keyset paging saves is *store* reads, which is the README's
    495-key-value-pairs-by-offset against 5-by-cursor — a kernel measurement of
    what the server does to answer, not of what the caller sends. The two are
    easy to conflate and the phrase "cheaper on the wire" conflates them, which
    is why this test exists beside the one above rather than instead of it.

    This is the control that makes the previous test mean something, in the same
    way the one-at-a-time write is the control for the batch: without it, "four
    pages, four calls" reads as a saving, and it is a saving over nothing.
    """
    client, counter = counted

    seeding = Batch(Atomicity.INDEPENDENT)
    for n in range(PAGES * PAGE_SIZE):
        seeding.insert(DOCS, [_row(n)])
    client.batch(seeding)
    counter.calls.clear()
    counter.bytes.clear()

    seen: list[PyValue] = []
    for page in range(PAGES):
        q = Query(DOCS).limit(PAGE_SIZE).offset(page * PAGE_SIZE)
        rows = list(client.query(q.where(q.c.kind.eq("counted"))))
        seen += [row[0] for row in rows]

    assert counter.calls["Query"] == PAGES, (
        f"{PAGES} offset pages cost {PAGES} calls, the same as by cursor; got {counter.calls}"
    )
    # The *keys*, not a count of them. A count cannot tell four pages from four
    # copies of the first page, and that is not hypothetical: a mutation pinning
    # every offset page to 0 survived the count assertion this replaces. The
    # cursor twin has the same hole — `after(cursor)` mutated to `after(None)`
    # returns twenty rows in four calls — so both tests walk the keys.
    assert seen == list(range(FIRST_KEY, FIRST_KEY + PAGES * PAGE_SIZE)), (
        f"{PAGES} pages of {PAGE_SIZE} should walk the keys once, in order; got {seen}"
    )


class Recording(grpc.UnaryUnaryClientInterceptor, grpc.UnaryStreamClientInterceptor):
    """Keeps every request this client sent, by RPC name.

    `Counting` above answers how many; this answers what was in them. The
    freshness floor is a property of the *request*: a client that dropped it
    returns exactly the right rows on a single-node fixture, so nothing about
    the answer distinguishes one that sends it from one that does not.
    """

    def __init__(self) -> None:
        self.seen: list[tuple[str, Any]] = []

    def _note(self, details: Any, request: Any) -> None:
        self.seen.append((details.method.rsplit("/", 1)[-1], request))

    def intercept_unary_unary(
        self, continuation: Any, client_call_details: Any, request: Any
    ) -> Any:
        self._note(client_call_details, request)
        return continuation(client_call_details, request)

    def intercept_unary_stream(
        self, continuation: Any, client_call_details: Any, request: Any
    ) -> Any:
        self._note(client_call_details, request)
        return continuation(client_call_details, request)


def _queries_carrying_a_floor(server: Serving, *, monotonic: bool, key: int) -> list[bool]:
    """Write, then read, and report whether each `Query` carried a floor."""
    recorder = Recording()
    channel = grpc.intercept_channel(grpc.insecure_channel(server.address), recorder)
    client = Client(server.address, APP, channel=channel, monotonic_reads=monotonic)
    client.wait_for_ready()
    try:
        # The write first: the floor is the watermark, and a session that has
        # read and written nothing has none to send. Without this the
        # assertions below would hold against a client that never sets the
        # field — the hole this same test had in Go and TypeScript, found on
        # 2026-09-29 and fixed there in the same change as this.
        client.insert(DOCS, [(u64(key), "floor", i64(1), None)], upsert=True)
        recorder.seen.clear()
        q = Query(DOCS)
        list(client.query(q.where(q.c.id.eq(u64(key)))))
        return [
            request.HasField("freshness") for method, request in recorder.seen if method == "Query"
        ]
    finally:
        client.delete(DOCS, [[u64(key)]])
        client.close()


def test_a_read_after_a_write_carries_the_freshness_floor(server: Serving) -> None:
    assert _queries_carrying_a_floor(server, monotonic=True, key=FIRST_KEY + 80) == [True]


def test_a_non_monotonic_session_still_reads_its_own_writes(server: Serving) -> None:
    """The behaviour the other two clients adopted on 2026-09-29.

    `monotonic_reads=False` stops the watermark advancing from *reads* — see
    `Session`'s own docstring — and leaves it advancing from this session's own
    commits, so a read after a write still carries a floor.

    This client always did that. Go and TypeScript gated the floor on the flag
    instead, so a session that wrote sent nothing and could miss its own write
    with no error anywhere. Measured across all three with an interceptor,
    written up in
    `ledger/2026-09-29-the-third-client-sends-a-floor-the-other-two-do-not.md`,
    and resolved in favour of this client in
    `ledger/2026-09-29-read-your-writes-is-not-monotonic-reads.md`: the flag is
    named for monotonic reads and read-your-writes is a different guarantee.

    So this test no longer pins a divergence. It pins the agreement, and the
    assertion is unchanged — which is the point of having written it before the
    decision rather than after.
    """
    assert _queries_carrying_a_floor(server, monotonic=False, key=FIRST_KEY + 81) == [True]
