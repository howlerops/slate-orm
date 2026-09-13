"""Behaviours of the wire that this client relies on, or deliberately does not.

Everything here was measured rather than read. Several were the opposite of
what I assumed, and the assumptions are recorded in `PROTOCOL-FINDINGS.md`.

These tests go through the generated stub rather than the client, because the
point of each is a request this client will not build — either because it
refuses it locally, or because the API has no way to express it.
"""

from __future__ import annotations

from collections.abc import Iterator

import grpc
import pytest

from slate._proto.slate.v1 import records_pb2 as pb
from slate._proto.slate.v1 import records_pb2_grpc as pb_grpc

from .conftest import APP, Serving


def U(n: int) -> pb.Value:
    return pb.Value(uint64_value=n)


def I(n: int) -> pb.Value:  # noqa: E743 - named for the wire type it makes
    return pb.Value(int64_value=n)


def S(text: str) -> pb.Value:
    return pb.Value(string_value=text)


NULL = pb.Value(null_value=pb.NULL_VALUE)

Stub = pb_grpc.RecordsStub


@pytest.fixture
def stub(oracle_server: Serving) -> Iterator[Stub]:
    channel = grpc.insecure_channel(oracle_server.address)
    try:
        yield pb_grpc.RecordsStub(channel)
    finally:
        channel.close()


def _rows(stub: Stub, query: pb.Query) -> int:
    return sum(
        len(message.rows)
        for message in stub.Query(pb.QueryRequest(query=query), metadata=APP.metadata)
    )


def _compare(
    column: int,
    op: pb.CmpOp.ValueType,
    value: pb.Value,
    hint: pb.AccessHint | None = None,
) -> pb.Query:
    query = pb.Query(
        table="docs",
        filter=pb.Expr(
            compare=pb.Compare(column=pb.ColumnRef(input=0, column=column), op=op, value=value)
        ),
    )
    if hint is not None:
        query.hint.CopyFrom(hint)
    return query


# --- integer widths ---------------------------------------------------------


@pytest.mark.parametrize(
    ("label", "column", "op", "matching", "mismatched"),
    [
        ("id (u64) equality", 0, pb.CMP_OP_EQ, U(1), I(1)),
        ("id (u64) range", 0, pb.CMP_OP_GT, U(3), I(3)),
        ("size (i64) equality", 2, pb.CMP_OP_EQ, I(25), U(25)),
        ("size (i64) range", 2, pb.CMP_OP_GT, I(25), U(25)),
    ],
)
def test_a_predicate_coerces_between_the_two_integer_widths(
    stub: Stub,
    label: str,
    column: int,
    op: pb.CmpOp.ValueType,
    matching: pb.Value,
    mismatched: pb.Value,
) -> None:
    """Measured, not documented. This client does not rely on it.

    `Value`'s ordering is type first, so I expected a width mismatch in a
    filter to select nothing. It does not: the comparison coerces. The `.proto`
    says nothing about this either way, which is why `slate.values` still
    refuses an ambiguous integer where no column type is declared — a client
    that depends on undocumented leniency breaks when it is tightened.

    Pinned so that tightening it is visible here rather than in somebody's
    query.
    """
    assert _rows(stub, _compare(column, op, matching)) == _rows(
        stub, _compare(column, op, mismatched)
    )


def test_the_coercion_agrees_across_access_paths(stub: Stub) -> None:
    """The failure that would actually matter: a plan-dependent answer.

    `docs/correctness.md` opens with a covering index returning a column
    nobody asked for — "same query, two answers, decided by the optimiser".
    Coercion that held for a table scan and not for an index would be that
    bug. It does not.
    """
    scan = pb.AccessHint(table_scan=True)
    index = pb.AccessHint(index="by_size")
    answers = {
        _rows(stub, _compare(2, pb.CMP_OP_GT, value, hint))
        for value in (I(25), U(25))
        for hint in (None, scan, index)
    }
    assert len(answers) == 1, f"the access paths disagree: {answers}"


def test_the_write_path_refuses_the_wrong_integer_width(stub: Stub) -> None:
    """The other half, and the reason `Table` has to carry types at all.

    Loud rather than silent, and the message names both types — which makes
    this a development-time problem rather than a production one.
    """
    with pytest.raises(grpc.RpcError) as caught:
        stub.Insert(
            pb.InsertRequest(
                table="docs",
                rows=[pb.Row(values=[I(900), S("wrong-width"), I(1), NULL])],
            ),
            metadata=APP.metadata,
        )
    assert caught.value.code() is grpc.StatusCode.INVALID_ARGUMENT
    assert "expects u64, got i64" in caught.value.details()


# --- primary keys -----------------------------------------------------------


@pytest.mark.parametrize(
    ("label", "key"),
    [("two values for a one-column key", [U(1), S("x")]), ("no values at all", [])],
)
def test_a_primary_key_of_the_wrong_arity_reads_as_absent(
    stub: Stub, label: str, key: list[pb.Value]
) -> None:
    """PROTOCOL FINDING, pinned: a malformed key is not refused.

    `found: false` is deliberately indistinguishable from "a row your policy
    hides", which is right. It should not also be indistinguishable from "this
    request was nonsense". The insert path refuses a row of the wrong width by
    name; `Get` and `Delete` do not.

    This client refuses it locally, so reaching it needs the raw stub.
    """
    response = stub.Get(
        pb.GetRequest(table="docs", primary_key=pb.Row(values=key)), metadata=APP.metadata
    )
    assert response.found is False


# --- freshness --------------------------------------------------------------


def test_freshness_any_false_still_means_any(stub: Stub) -> None:
    """A zeroed `oneof` arm is a request that does not say what it looks like.

    The reasoning for reading `latest: false` as ANY is sound (see
    `convert.rs`); the consequence is a field whose `false` value means
    something other than "not this". `NullValue`'s one-value-enum trick would
    have made it unrepresentable.
    """
    assert (
        _rows(stub, pb.Query(table="docs"))
        == sum(
            len(m.rows)
            for m in stub.Query(
                pb.QueryRequest(query=pb.Query(table="docs"), freshness=pb.Freshness(any=False)),
                metadata=APP.metadata,
            )
        )
    )


# --- refusals this client cannot reach through its own API ------------------


def test_a_join_input_with_a_sort_is_refused(stub: Stub) -> None:
    """`JoinInput` has no `sort()`, so the refusal is unreachable from the API.

    Checked here so that the API's omission stays justified: if the server ever
    started accepting one, `JoinInput` would be withholding something real.
    """
    join = pb.JoinQuery(
        inputs=[
            pb.JoinInput(
                query=pb.Query(
                    table="authors", sort=[pb.SortKey(column=pb.ColumnRef(input=0, column=1))]
                )
            ),
            pb.JoinInput(
                query=pb.Query(table="books"),
                on=[
                    pb.JoinOn(
                        earlier=pb.ColumnRef(input=0, column=1),
                        own=pb.ColumnRef(input=1, column=2),
                    )
                ],
            ),
        ]
    )
    with pytest.raises(grpc.RpcError) as caught:
        list(stub.Join(pb.JoinRequest(join=join), metadata=APP.metadata))
    assert "would not order the result" in caught.value.details()


def test_a_value_with_no_kind_is_refused(stub: Stub) -> None:
    """The rule the `.proto` opens with, checked once.

    This client cannot build one — `to_value` always sets a kind — so it is
    checked through the stub. Reading it as a null would turn a predicate into
    a different predicate.
    """
    query = _compare(0, pb.CMP_OP_EQ, pb.Value())
    with pytest.raises(grpc.RpcError) as caught:
        _rows(stub, query)
    assert "no kind set" in caught.value.details()


def test_a_column_reference_with_no_kind_is_refused(stub: Stub) -> None:
    """Not defaulted to column 0, which is the whole reason `of` is a `oneof`."""
    query = pb.Query(
        table="docs",
        filter=pb.Expr(
            compare=pb.Compare(
                column=pb.ColumnRef(input=0), op=pb.CMP_OP_EQ, value=U(1)
            )
        ),
    )
    with pytest.raises(grpc.RpcError) as caught:
        _rows(stub, query)
    assert "column reference with no kind set" in caught.value.details()


# --- affected ---------------------------------------------------------------


def test_affected_is_echoed_for_an_insert_and_discovered_for_a_delete(
    server: Serving,
) -> None:
    """PROTOCOL FINDING, pinned: `affected` says nothing about an insert.

    An insert refuses the whole batch rather than applying a prefix, so the
    count is always the number of rows sent. A delete's count is real.
    """
    from slate import i64, u64

    from .conftest import connect
    from .fixture import DOCS

    with connect(server) as client:
        rows = [(u64(700 + n), "affected", i64(n), None) for n in range(3)]
        assert client.insert(DOCS, rows).affected == len(rows)
        # An upsert over a row that already exists reports 1 as well, so it
        # does not distinguish an insert from a replacement either.
        assert client.insert(DOCS, [rows[0]], upsert=True).affected == 1
        # A delete counts what was there.
        assert client.delete(DOCS, [[u64(700)], [u64(7_999)]]).affected == 1
