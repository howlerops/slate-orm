"""Decoding the reason token out of a status's details.

The blob below was captured from a running head node rather than built here,
which is the point: a fixture this module encoded itself would agree with this
module's own idea of the wire format, and agreeing with yourself proves
nothing. This one is what `slate-serverd` actually sent, so a decoder that
drifts from the server fails here.

The same bytes are decoded by the Go and TypeScript suites, so the three
clients are checked against one artefact rather than three transcriptions.
"""

from __future__ import annotations

import grpc
import pytest

from slate._details import ERROR_INFO_URL, reason_of
from slate.errors import _DETAILS_KEY, ResourceLimit, RpcCall, from_rpc_error

#: A real `grpc-status-details-bin`, captured from a `delete_where` with
#: `returning` that matched more rows than `max_returned_rows` allowed.
#: `google.rpc.Status{code: 8, message: "...", details: [ErrorInfo{...}]}`.
BLOB = bytes.fromhex(
    "08081290016120707265646963617465207772697465206d617463686564206d6f72"
    "65207468616e203320726f77732c20616e642069747320726f777320776572652061"
    "736b656420666f723b206e6172726f7720746865207072656469636174652c206472"
    "6f7020746865207265717565737420666f722074686520726f77732c206f72207261"
    "69736520746865206c696d69741a520a28747970652e676f6f676c65617069732e63"
    "6f6d2f676f6f676c652e7270632e4572726f72496e666f12260a1950524544494341"
    "54455f57524954455f544f4f5f4c415247451209736c6174652d6f726d"
)


def test_the_token_comes_out_of_a_real_blob() -> None:
    assert reason_of(BLOB) == "PREDICATE_WRITE_TOO_LARGE"


def test_the_url_is_the_one_the_server_packs_under() -> None:
    # Not a restatement of the constant: the captured blob carries the URL as
    # bytes, so this asserts the client's idea of it matches the server's.
    assert ERROR_INFO_URL.encode() in BLOB


@pytest.mark.parametrize(
    "blob",
    [
        pytest.param(b"", id="empty"),
        pytest.param(b"\xff\xff\xff\xff", id="not a protobuf at all"),
        # A well-formed Status with no details at all: code 8, no field 3.
        pytest.param(b"\x08\x08", id="a status carrying no details"),
    ],
)
def test_anything_else_is_the_empty_token(blob: bytes) -> None:
    # Never an exception: a client that raised while building an error object
    # would replace the server's failure with its own, and the caller would
    # stop learning why the call failed at all.
    assert reason_of(blob) == ""


def test_a_truncated_blob_does_not_raise() -> None:
    # Every prefix of a real message, including the ones that parse into
    # something plausible and the ones that do not.
    for cut in range(len(BLOB)):
        assert reason_of(BLOB[:cut]) in ("", "PREDICATE_WRITE_TOO_LARGE")


def _tag(number: int, wire: int) -> bytes:
    return _varint((number << 3) | wire)


def _varint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def _bytes_field(number: int, payload: bytes) -> bytes:
    return _tag(number, 2) + _varint(len(payload)) + payload


def decoy(url: str, reason: str = "DECOY") -> bytes:
    """A well-formed `Status` whose one detail is packed under `url`.

    Encoded by hand, in the opposite direction from the decoder under test, so
    that agreeing with it says something. The payload is shaped exactly like an
    `ErrorInfo` — field 1, a string — so a decoder that skipped the type check
    would hand back `reason` rather than `""`. That is the whole point: the
    check is what stops one message's field 1 being read as another's.
    """
    packed = _bytes_field(1, reason.encode())
    any_message = _bytes_field(1, url.encode()) + _bytes_field(2, packed)
    return _tag(1, 0) + _varint(8) + _bytes_field(3, any_message)


def test_a_detail_of_another_type_is_skipped_not_misread() -> None:
    blob = decoy("type.googleapis.com/google.rpc.RetryInfo")
    assert reason_of(blob) == ""


def test_the_decoy_would_be_read_without_the_type_check() -> None:
    """The negative control for the test above.

    Without it, `test_a_detail_of_another_type_is_skipped_not_misread` passes
    for two different reasons — the URL check working, or the decoy being
    malformed and decoding to nothing either way — and only one of them is the
    property. This pins the second: packed under the URL the server really
    uses, the very same bytes come back as a token.
    """
    assert reason_of(decoy(ERROR_INFO_URL)) == "DECOY"


class _FakeCall(Exception):
    """A `grpc.RpcError` as a call delivers one, with the blob in its trailers.

    Hand-rolled rather than mocked: `from_rpc_error` reads four things off the
    exception and this supplies exactly those, so a change to what it reads
    shows up as an AttributeError here rather than as a silently empty field.
    """

    def __init__(self, blob: bytes) -> None:
        super().__init__("resource exhausted")
        self._blob = blob

    def code(self) -> grpc.StatusCode:
        return grpc.StatusCode.RESOURCE_EXHAUSTED

    def details(self) -> str:
        return "a predicate write matched more rows than it may return"

    def trailing_metadata(self) -> tuple[tuple[str, object], ...]:
        # A text trailer beside the binary one, because `_trailers` and
        # `_reason` walk the same sequence and must not trip over each other.
        return (("content-type", "application/grpc"), (_DETAILS_KEY, self._blob))


def test_from_rpc_error_carries_the_token_onto_the_exception() -> None:
    """The wiring test, and a different test from the ones above on purpose.

    Those call `reason_of` directly, so all of them keep passing if
    `from_rpc_error` simply stops asking for a token — which is exactly the
    state this change fixed. The same mutation in the Go client survived every
    decoder test and was caught only by the conformance runner, which is a slow
    and indirect way to learn that one client stopped filling one field.
    """
    error = from_rpc_error(_FakeCall(BLOB))
    assert error.reason == "PREDICATE_WRITE_TOO_LARGE"
    # Unchanged by carrying a token: the class, and the text trailers.
    assert isinstance(error, ResourceLimit)
    assert error.trailers == {"content-type": "application/grpc"}


def test_a_failure_with_no_details_has_the_empty_token() -> None:
    error = from_rpc_error(_FakeCall(b""))
    assert error.reason == ""


class _CodeOnly(Exception):
    """A failure carrying `code` and not `details`.

    The one behaviour difference between the `RpcCall` Protocol and the two
    independent `hasattr` calls it replaced. gRPC does not produce this — the
    two methods come together off `grpc.Call` — so the test pins the choice
    rather than a requirement: both or neither.
    """

    def code(self) -> grpc.StatusCode:
        return grpc.StatusCode.RESOURCE_EXHAUSTED


def test_a_half_shaped_failure_is_taken_as_neither() -> None:
    # Suppressed because the excluded case *is* the test: `_CodeOnly` is
    # neither an `RpcError` nor an `RpcCall` — it has `code` and not `details`
    # — and what is being pinned is what `from_rpc_error` does when handed one.
    error = from_rpc_error(_CodeOnly("half a call"))  # ty: ignore[invalid-argument-type]
    assert error.code is grpc.StatusCode.UNKNOWN, (
        "code and details are taken together; one without the other is not a call"
    )
    assert error.message == "half a call"


def test_the_protocol_matches_what_a_real_failure_carries() -> None:
    # `runtime_checkable` checks attribute presence only, which is what makes
    # it a drop-in for `hasattr` — and what makes the hand-rolled fake below,
    # which is not a `grpc.Call`, still count as one.
    assert isinstance(_FakeCall(b""), RpcCall)
    assert not isinstance(_CodeOnly("x"), RpcCall)
